# Architecture

ccnotify has no daemon and no shared state between sessions. Every `claude`
you launch gets its own wrapper process, its own loopback HTTP server on a
random port, its own secret token, and its own overlay window. Killing one
session cannot affect another.

## Components

```
┌─────────────────────────── one session ────────────────────────────┐
│                                                                     │
│  terminal ◄──────────► ccnotify (wrapper)                           │
│                          │  ├── pty ◄──► real `claude` process      │
│                          │  ├── HTTP server on 127.0.0.1:<random>   │
│                          │  └── spawns/kills ccnotify-overlay       │
│                          │                                          │
│  Claude Code hooks ──────┘        ccnotify-overlay (Tauri window)   │
│  (notify-forward.sh POSTs         polls state, posts decisions      │
│   events to the port it           and replies                       │
│   inherited via env)                                                │
└─────────────────────────────────────────────────────────────────────┘
```

### `wrapper/` — the `ccnotify` binary

- Spawns `claude <args>` inside a pseudo-terminal it owns
  (`portable-pty`), relaying the real terminal's stdin/stdout through
  transparently: raw mode on the controlling tty, `SIGWINCH` resize
  forwarding, exit-code passthrough.
- Before spawning, binds `127.0.0.1:0` to reserve a random port and
  generates a 16-byte random token. Both are placed in the child's
  environment (`CCNOTIFY_PORT`, `CCNOTIFY_TOKEN`, plus `CCNOTIFY_ALIAS`
  and `CCNOTIFY_COLOR`), so every hook Claude Code runs inherits them.
- Tees everything the pty prints into a 64 KB ring buffer that backs the
  overlay's output view.
- Spawns `ccnotify-overlay` (looked up **next to its own binary**) and
  kills it when the session ends.

### `hooks/notify-forward.sh`

Installed once to `~/.claude/hooks/ccnotify-forward.sh` by `ccnotify
setup`, and referenced from the global `~/.claude/settings.json` for four
events: `PreToolUse` (matcher `*`, timeout 600 s), `Notification`, `Stop`,
and `UserPromptSubmit`.

Its entire job: if `$CCNOTIFY_PORT` is unset, exit 0 (Claude was started
without the wrapper — the hook is a no-op). Otherwise POST the hook's
stdin JSON to `http://127.0.0.1:$CCNOTIFY_PORT/event` with the token
header and print the response body to stdout, which is how a permission
decision gets back to Claude Code.

### `overlay/` — the Tauri window

A frameless, always-on-top, all-workspaces window using the OS webview.
On macOS it runs with the Accessory activation policy so it never appears
in the Dock or ⌘-Tab. The webview does **no** cross-origin networking:
its JS calls Tauri commands, and the Rust side talks to the wrapper over
a plain `TcpStream` HTTP/1.1 client (`overlay/src/http.rs`). This
sidesteps CORS and App Transport Security entirely.

State flows by long-polling `GET /overlay/state?version=N` — the wrapper
holds the request up to 25 s until the state version changes, so updates
are instant while idle cost stays near zero.

If the overlay cannot reach its wrapper for ~12 s (six consecutive poll
failures), it exits — a wrapper that died uncleanly cannot leave a zombie
overlay behind.

### `common/` — shared crate

Session identity, state/decision/reply payload types, and the
alias→color mapping: FNV-1a hash of the alias into a fixed 10-color
palette, so the same project always gets the same color with nothing
stored.

## Event flow

| Hook event | Wrapper behavior |
| --- | --- |
| `PreToolUse` | First checked against the session's own `permissions.allow`/`.deny`/`defaultMode` (merged from `.claude/settings.local.json`, `.claude/settings.json`, and `~/.claude/settings.json` — see `wrapper/src/permissions.rs`). A match decides instantly with no overlay involvement at all, mirroring what the raw terminal would do. Otherwise: registers a pending permission, flips state to **needs input**, fires an OS notification, and **blocks the HTTP response** until the overlay posts a decision. The decision is returned as `hookSpecificOutput.permissionDecision` (`allow`/`deny` + reason). After 570 s with no answer it returns an empty body, letting Claude Code fall back to its own terminal prompt. |
| `Notification` | State → **needs input** with the notification message (unless a richer pending permission is already showing). Responds 200 immediately. |
| `Stop` | State → **idle**, capturing `last_assistant_message` from the payload (transcript-file parsing is the fallback for older Claude Code versions). Responds 200 immediately. |
| `UserPromptSubmit` | State → **working**. Responds 200 immediately. |

Free-text replies from the overlay are written into the pty's stdin
followed by `\r` — from Claude Code's point of view the user typed them.

## HTTP API (loopback only)

All endpoints require the session token, via `X-CCNotify-Token` header or
`?token=` query parameter.

| Endpoint | Purpose |
| --- | --- |
| `POST /event` | Hook events in; PreToolUse blocks until decided |
| `GET /overlay/state?version=N` | Long-poll state snapshot |
| `GET /overlay/output` | Escape-stripped tail of recent terminal output |
| `POST /overlay/decision` | `{id, decision: "allow"\|"deny", reason?}` |
| `POST /overlay/reply` | `{text}` → written into the pty's stdin |
| `POST /overlay/open` | Focus the app hosting this session's terminal (from `__CFBundleIdentifier`/`TERM_PROGRAM` captured at startup; macOS only) |
| `POST /overlay/quit` | Ends the Claude session (kills the pty child) |

## Security model

- The server binds `127.0.0.1` only; nothing is reachable from the network.
- Every request must present the per-session random token, which exists
  only in the wrapper's memory and in the environment of the processes it
  spawned. Other local processes cannot forge events, read output, or
  approve permissions.
- The hook script never trusts anything except its own environment; with
  no `$CCNOTIFY_PORT` it does nothing, so globally-installed hooks are
  inert outside wrapped sessions.

## Resource budget

The spec's target is roughly 20–40 MB and near-zero CPU per overlay at
idle. Measured on macOS with three concurrent sessions: ~1.8 MB per
wrapper, ~43 MB per overlay (debug build; release is lower and most of it
is the shared system WebKit), 0.0% CPU at idle for both. The pill's
"working" indicator is a static icon with a cheap CSS transition, not an
animation loop, specifically to keep idle CPU at zero.
