//! One wrapper session: spawns the real `claude` in a pty, relays the
//! terminal, runs the local HTTP server, and owns the overlay process.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child as ProcChild, Command, Stdio};
use std::sync::{Arc, Mutex};

use ccnotify_common::{color_for_alias, OverlayState, Status};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use crate::server::{self, Shared};
use crate::term;

/// Ring buffer size for the overlay's terminal-output view.
const OUTPUT_BUF_MAX: usize = 64 * 1024;

pub fn run(alias_override: Option<String>, claude_args: Vec<String>) -> Result<i32, String> {
    let alias = resolve_alias(alias_override);
    let color = color_for_alias(&alias).to_string();
    let token = random_token();

    // Bind first so the port is actually reserved before claude inherits it.
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();

    // Spawn the real claude in a pty it thinks is the terminal.
    let (rows, cols) = term::size();
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("openpty: {e}"))?;

    let mut cmd = CommandBuilder::new("claude");
    for a in &claude_args {
        cmd.arg(a);
    }
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }
    cmd.env("CCNOTIFY_PORT", port.to_string());
    cmd.env("CCNOTIFY_TOKEN", &token);
    cmd.env("CCNOTIFY_ALIAS", &alias);
    cmd.env("CCNOTIFY_COLOR", &color);

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("failed to spawn claude: {e} (is `claude` on PATH?)"))?;
    drop(pair.slave);

    let mut pty_reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| e.to_string())?;
    let pty_writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(
        pair.master.take_writer().map_err(|e| e.to_string())?,
    ));
    let master = Arc::new(Mutex::new(pair.master));

    // Shared state + HTTP server.
    let shared = Arc::new(Shared::new(OverlayState {
        version: 1,
        status: Status::Working,
        alias: alias.clone(),
        color: color.clone(),
        pending: None,
        message: None,
        can_reply: false,
    }));
    let output: server::OutputBuf = Arc::new(Mutex::new(VecDeque::new()));
    let killer: server::ChildKiller = Arc::new(Mutex::new(child.clone_killer()));
    {
        let ctx = server::Ctx {
            token: token.clone(),
            shared: shared.clone(),
            pty_writer: pty_writer.clone(),
            output: output.clone(),
            killer,
        };
        std::thread::spawn(move || server::serve(listener, ctx));
    }

    let overlay = spawn_overlay(port, &token, &alias, &color);
    if overlay.is_none() {
        eprintln!("ccnotify: overlay binary not found, running headless (events still logged)");
    }

    // Terminal relay. Raw mode so keystrokes reach claude unmodified.
    let raw = term::RawGuard::enable();

    // stdin -> pty
    {
        let writer = pty_writer.clone();
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; 4096];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut w = writer.lock().unwrap();
                        if w.write_all(&buf[..n]).is_err() {
                            break;
                        }
                        let _ = w.flush();
                    }
                }
            }
        });
    }

    // pty -> stdout, teeing into the ring buffer for the output view
    let reader_handle = {
        let output = output.clone();
        std::thread::spawn(move || {
            let mut stdout = std::io::stdout();
            let mut buf = [0u8; 8192];
            loop {
                match pty_reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        {
                            let mut out = output.lock().unwrap();
                            out.extend(&buf[..n]);
                            while out.len() > OUTPUT_BUF_MAX {
                                out.pop_front();
                            }
                        }
                        if stdout.write_all(&buf[..n]).is_err() {
                            break;
                        }
                        let _ = stdout.flush();
                    }
                }
            }
        })
    };

    // Window resize relay.
    #[cfg(unix)]
    {
        let master = master.clone();
        let mut signals = signal_hook::iterator::Signals::new([libc::SIGWINCH])
            .map_err(|e| e.to_string())?;
        std::thread::spawn(move || {
            for _ in signals.forever() {
                let (rows, cols) = term::size();
                let _ = master.lock().unwrap().resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
        });
    }

    let status = child.wait().map_err(|e| e.to_string())?;
    // Drain any remaining output before tearing down.
    let _ = reader_handle.join();

    if let Some(mut overlay) = overlay {
        let _ = overlay.kill();
        let _ = overlay.wait();
    }
    if let Some(raw) = raw {
        raw.restore();
    }
    drop(master);

    Ok(status.exit_code() as i32)
}

/// Alias precedence: --alias flag > .ccnotify.json in cwd > cwd basename.
fn resolve_alias(alias_override: Option<String>) -> String {
    if let Some(a) = alias_override {
        return a;
    }
    if let Ok(raw) = std::fs::read_to_string(".ccnotify.json") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(a) = v.get("alias").and_then(|a| a.as_str()) {
                return a.to_string();
            }
        }
    }
    std::env::current_dir()
        .ok()
        .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "claude".to_string())
}

fn random_token() -> String {
    let mut bytes = [0u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .is_err()
    {
        // Fallback entropy; local-only token guarding a loopback port.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        bytes.copy_from_slice(&now.to_le_bytes());
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The overlay binary ships next to the wrapper binary.
fn spawn_overlay(port: u16, token: &str, alias: &str, color: &str) -> Option<ProcChild> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let candidates: Vec<PathBuf> = vec![
        exe_dir.join("ccnotify-overlay"),
        exe_dir.join("ccnotify-overlay.exe"),
    ];
    let bin = candidates.into_iter().find(|p| p.exists())?;
    Command::new(bin)
        .env("CCNOTIFY_PORT", port.to_string())
        .env("CCNOTIFY_TOKEN", token)
        .env("CCNOTIFY_ALIAS", alias)
        .env("CCNOTIFY_COLOR", color)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}
