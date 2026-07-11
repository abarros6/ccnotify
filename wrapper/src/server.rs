//! Loopback HTTP server: receives hook events from `notify-forward`,
//! serves state to the overlay (long-poll), and accepts decisions/replies.

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::net::TcpListener;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use ccnotify_common::{Decision, OverlayState, PendingPermission, Reply, Status};
use tiny_http::{Header, Method, Response, Server};

/// How long a blocked PreToolUse request waits for an overlay decision
/// before giving up and letting Claude Code fall back to its own
/// terminal prompt. Must stay under the hook `timeout` in settings.json.
const PERMISSION_WAIT: Duration = Duration::from_secs(570);
const LONG_POLL_WAIT: Duration = Duration::from_secs(25);

pub struct Shared {
    inner: Mutex<Inner>,
    changed: Condvar,
}

struct Inner {
    state: OverlayState,
    pending_tx: HashMap<u64, mpsc::Sender<Decision>>,
    next_id: u64,
}

impl Shared {
    pub fn new(state: OverlayState) -> Self {
        Self {
            inner: Mutex::new(Inner {
                state,
                pending_tx: HashMap::new(),
                next_id: 1,
            }),
            changed: Condvar::new(),
        }
    }

    fn mutate(&self, f: impl FnOnce(&mut OverlayState)) {
        let mut inner = self.inner.lock().unwrap();
        f(&mut inner.state);
        inner.state.version += 1;
        self.changed.notify_all();
    }
}

pub type PtyWriter = Arc<Mutex<Box<dyn Write + Send>>>;
pub type OutputBuf = Arc<Mutex<VecDeque<u8>>>;
pub type ChildKiller = Arc<Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>>;

/// Everything a request handler might need for one session.
pub struct Ctx {
    pub token: String,
    pub shared: Arc<Shared>,
    pub pty_writer: PtyWriter,
    pub output: OutputBuf,
    pub killer: ChildKiller,
}

pub fn serve(listener: TcpListener, ctx: Ctx) {
    let server = match Server::from_listener(listener, None) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ccnotify: http server failed to start: {e}");
            return;
        }
    };
    let ctx = Arc::new(ctx);
    for request in server.incoming_requests() {
        let ctx = ctx.clone();
        // One thread per request: permission events intentionally block.
        std::thread::spawn(move || handle(request, &ctx));
    }
}

fn cors(mut resp: Response<std::io::Cursor<Vec<u8>>>) -> Response<std::io::Cursor<Vec<u8>>> {
    for (k, v) in [
        ("Access-Control-Allow-Origin", "*"),
        (
            "Access-Control-Allow-Headers",
            "Content-Type, X-CCNotify-Token",
        ),
        ("Access-Control-Allow-Methods", "GET, POST, OPTIONS"),
    ] {
        resp.add_header(Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap());
    }
    resp
}

fn json_response(status: u16, body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    let mut resp = Response::from_string(body).with_status_code(status);
    resp.add_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
    cors(resp)
}

fn handle(mut request: tiny_http::Request, ctx: &Ctx) {
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();

    if *request.method() == Method::Options {
        let _ = request.respond(cors(Response::from_string("")));
        return;
    }

    if !authorized(&request, &url, &ctx.token) {
        let _ = request.respond(json_response(403, "{\"error\":\"bad token\"}".into()));
        return;
    }

    let mut body = String::new();
    let _ = request.as_reader().read_to_string(&mut body);

    // Quit needs to respond before it kills the session.
    if *request.method() == Method::Post && path == "/overlay/quit" {
        let _ = request.respond(json_response(200, "{}".into()));
        let _ = ctx.killer.lock().unwrap().kill();
        return;
    }

    let response = match (request.method().clone(), path.as_str()) {
        (Method::Post, "/event") => handle_event(&body, &ctx.shared),
        (Method::Get, "/overlay/state") => handle_state(&url, &ctx.shared),
        (Method::Get, "/overlay/output") => handle_output(&ctx.output),
        (Method::Post, "/overlay/decision") => handle_decision(&body, &ctx.shared),
        (Method::Post, "/overlay/reply") => handle_reply(&body, &ctx.shared, &ctx.pty_writer),
        _ => json_response(404, "{\"error\":\"not found\"}".into()),
    };
    let _ = request.respond(response);
}

fn authorized(request: &tiny_http::Request, url: &str, token: &str) -> bool {
    let header_ok = request.headers().iter().any(|h| {
        h.field.as_str().as_str().eq_ignore_ascii_case("x-ccnotify-token")
            && h.value.as_str() == token
    });
    header_ok || query_param(url, "token").as_deref() == Some(token)
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let qs = url.split_once('?')?.1;
    qs.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

/// Dispatch a hook event by its hook_event_name.
fn handle_event(body: &str, shared: &Shared) -> Response<std::io::Cursor<Vec<u8>>> {
    let event: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return json_response(400, "{\"error\":\"bad json\"}".into()),
    };
    let name = event
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match name {
        "PreToolUse" => handle_pre_tool_use(&event, shared),
        "Notification" => {
            let message = event
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Claude is waiting for your input")
                .to_string();
            notify_os(&shared.inner.lock().unwrap().state.alias, &message);
            shared.mutate(|s| {
                s.status = Status::NeedsInput;
                // Don't clobber a richer pending-permission view.
                if s.pending.is_none() {
                    s.message = Some(message);
                    s.can_reply = true;
                }
            });
            json_response(200, String::new())
        }
        "Stop" | "SubagentStop" => {
            if name == "Stop" {
                // Newer Claude Code versions put the text straight in the
                // payload; the transcript file may not even exist yet.
                let message = event
                    .get("last_assistant_message")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        event
                            .get("transcript_path")
                            .and_then(|v| v.as_str())
                            .and_then(last_assistant_message)
                    });
                notify_os(
                    &shared.inner.lock().unwrap().state.alias,
                    "Turn finished — Claude is idle",
                );
                shared.mutate(|s| {
                    s.status = Status::Idle;
                    s.message = message;
                    s.can_reply = true;
                    s.pending = None;
                });
            }
            json_response(200, String::new())
        }
        "UserPromptSubmit" => {
            shared.mutate(|s| {
                s.status = Status::Working;
                s.message = None;
                s.can_reply = false;
            });
            json_response(200, String::new())
        }
        _ => json_response(200, String::new()),
    }
}

/// Block until the overlay answers (or we time out), then reply in the
/// PreToolUse hook decision format Claude Code expects on stdout.
fn handle_pre_tool_use(
    event: &serde_json::Value,
    shared: &Shared,
) -> Response<std::io::Cursor<Vec<u8>>> {
    let tool_name = event
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let tool_input = event.get("tool_input").cloned().unwrap_or(serde_json::json!({}));

    let (tx, rx) = mpsc::channel::<Decision>();
    let id;
    {
        let mut inner = shared.inner.lock().unwrap();
        id = inner.next_id;
        inner.next_id += 1;
        inner.pending_tx.insert(id, tx);
        inner.state.pending = Some(PendingPermission {
            id,
            tool_name: tool_name.clone(),
            tool_input,
        });
        inner.state.status = Status::NeedsInput;
        inner.state.can_reply = false;
        inner.state.version += 1;
        shared.changed.notify_all();
        notify_os(
            &inner.state.alias,
            &format!("Permission needed: {tool_name}"),
        );
    }

    let decision = rx.recv_timeout(PERMISSION_WAIT).ok();

    {
        let mut inner = shared.inner.lock().unwrap();
        inner.pending_tx.remove(&id);
        if inner.state.pending.as_ref().map(|p| p.id) == Some(id) {
            inner.state.pending = None;
            inner.state.status = Status::Working;
            inner.state.version += 1;
            shared.changed.notify_all();
        }
    }

    match decision {
        Some(d) => {
            let allow = d.decision == "allow";
            let reason = d.reason.unwrap_or_else(|| {
                if allow {
                    "Approved from ccnotify overlay".into()
                } else {
                    "Denied from ccnotify overlay".into()
                }
            });
            let out = serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": if allow { "allow" } else { "deny" },
                    "permissionDecisionReason": reason,
                }
            });
            json_response(200, out.to_string())
        }
        // Timed out: return nothing so Claude Code falls back to its own
        // terminal permission prompt instead of a silent auto-deny.
        None => json_response(200, String::new()),
    }
}

fn handle_state(url: &str, shared: &Shared) -> Response<std::io::Cursor<Vec<u8>>> {
    let since: u64 = query_param(url, "version")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut inner = shared.inner.lock().unwrap();
    if inner.state.version == since {
        let (guard, _timeout) = shared
            .changed
            .wait_timeout_while(inner, LONG_POLL_WAIT, |i| i.state.version == since)
            .unwrap();
        inner = guard;
    }
    json_response(200, serde_json::to_string(&inner.state).unwrap())
}

/// Recent terminal output, escape-stripped, for the overlay's output view.
fn handle_output(output: &OutputBuf) -> Response<std::io::Cursor<Vec<u8>>> {
    let bytes: Vec<u8> = {
        let buf = output.lock().unwrap();
        buf.iter().copied().collect()
    };
    let text = tidy_terminal_output(&strip_escapes(&bytes));
    json_response(200, serde_json::json!({ "text": text }).to_string())
}

fn handle_decision(body: &str, shared: &Shared) -> Response<std::io::Cursor<Vec<u8>>> {
    let decision: Decision = match serde_json::from_str(body) {
        Ok(d) => d,
        Err(_) => return json_response(400, "{\"error\":\"bad json\"}".into()),
    };
    let tx = shared.inner.lock().unwrap().pending_tx.remove(&decision.id);
    match tx {
        Some(tx) => {
            let _ = tx.send(decision);
            json_response(200, "{}".into())
        }
        None => json_response(410, "{\"error\":\"no such pending request\"}".into()),
    }
}

/// Write the overlay's free-text reply straight into claude's stdin.
fn handle_reply(
    body: &str,
    shared: &Shared,
    pty_writer: &PtyWriter,
) -> Response<std::io::Cursor<Vec<u8>>> {
    let reply: Reply = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(_) => return json_response(400, "{\"error\":\"bad json\"}".into()),
    };
    {
        let mut w = pty_writer.lock().unwrap();
        if w.write_all(reply.text.as_bytes())
            .and_then(|_| w.write_all(b"\r"))
            .and_then(|_| w.flush())
            .is_err()
        {
            return json_response(500, "{\"error\":\"pty write failed\"}".into());
        }
    }
    shared.mutate(|s| {
        s.status = Status::Working;
        s.message = None;
        s.can_reply = false;
    });
    json_response(200, "{}".into())
}

/// Drop ANSI/VT escape sequences (CSI, OSC, DCS, single-char) from raw
/// pty bytes. CRs become newlines since TUIs use them for overwrites.
fn strip_escapes(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(s.len() / 2);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.next() {
                // CSI: parameters, then one final byte in @..~
                Some('[') => {
                    for n in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&n) {
                            break;
                        }
                    }
                }
                // OSC: until BEL or ST (ESC \)
                Some(']') => {
                    while let Some(n) = chars.next() {
                        if n == '\u{7}' {
                            break;
                        }
                        if n == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                // DCS / SOS / PM / APC: until ST
                Some('P') | Some('X') | Some('^') | Some('_') => {
                    while let Some(n) = chars.next() {
                        if n == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {} // two-char escape; both consumed
            }
        } else if c == '\r' {
            // CRLF collapses to one newline; a bare CR (TUI overwrite)
            // still becomes a line break.
            if chars.peek() != Some(&'\n') {
                out.push('\n');
            }
        } else if c == '\n' || c == '\t' || !c.is_control() {
            out.push(c);
        }
    }
    out
}

/// Squeeze redraw noise: trim line ends, collapse blank runs, keep tail.
fn tidy_terminal_output(text: &str) -> String {
    const MAX_LINES: usize = 120;
    let mut lines: Vec<&str> = Vec::new();
    let mut last_blank = true;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            if !last_blank {
                lines.push("");
            }
            last_blank = true;
        } else {
            lines.push(trimmed);
            last_blank = false;
        }
    }
    let start = lines.len().saturating_sub(MAX_LINES);
    lines[start..].join("\n")
}

/// Best-effort: pull the last assistant text out of the transcript JSONL.
fn last_assistant_message(path: &str) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    for line in raw.lines().rev() {
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let content = v.get("message")?.get("content")?;
        let mut text = String::new();
        if let Some(items) = content.as_array() {
            for item in items {
                if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                        if !text.is_empty() {
                            text.push_str("\n\n");
                        }
                        text.push_str(t);
                    }
                }
            }
        } else if let Some(t) = content.as_str() {
            text.push_str(t);
        }
        if !text.is_empty() {
            const MAX: usize = 4000;
            if text.len() > MAX {
                let cut = text
                    .char_indices()
                    .map(|(i, _)| i)
                    .take_while(|&i| i <= MAX)
                    .last()
                    .unwrap_or(0);
                text.truncate(cut);
                text.push('…');
            }
            return Some(text);
        }
    }
    None
}

/// Secondary nudge via native OS notification (macOS only for now).
fn notify_os(alias: &str, message: &str) {
    #[cfg(target_os = "macos")]
    {
        let escape = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            "display notification \"{}\" with title \"ccnotify — {}\"",
            escape(message),
            escape(alias)
        );
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (alias, message);
    }
}
