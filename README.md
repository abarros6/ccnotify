# ccnotify

**Actionable, always-on-top overlay notifications for [Claude Code](https://claude.com/claude-code).**

Step away from the terminal while Claude Code works. A small floating pill
follows you across windows and apps, shows what each session is doing
(working / needs input / idle), and lets you respond **from the overlay** —
no switching back to the terminal:

- **Permission requests** show the actual content, not just an alert: the
  literal Bash command, a red/green diff for file edits, a content preview
  for writes, the URL for fetches. Allow or deny in place, with an optional
  deny reason.
- **End of turn / idle** shows Claude's last message with a reply box — your
  reply is typed straight into that session's terminal.
- **Live output view** — peek at what the session is printing without
  raising the terminal window.
- **One overlay per session**, labeled with an alias (defaults to the folder
  name) and a consistent per-project color. Run as many concurrent sessions
  as you like; there is no daemon and no coordination between them.
- **Lightweight by design**: Rust + Tauri using the OS's native webview (no
  bundled Chromium). A few MB per binary, ~near-zero CPU at idle.

| Platform | Status |
| --- | --- |
| macOS | ✅ tested |
| Linux | ⚠️ should build (needs WebKitGTK dev packages), not yet tested |
| Windows | ❌ not yet (pty + console handling still to do) |

## Requirements

- [Claude Code](https://claude.com/claude-code) installed and working
  (`claude` on your `PATH`)
- `git`, `curl`, and a C toolchain (on macOS: `xcode-select --install`)
- [rustup](https://rustup.rs) — the build auto-installs the pinned Rust
  version from `rust-toolchain.toml`, so any rustup works
- Linux only: `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev`

## Install from a clone

```sh
# 1. Rust toolchain (skip if you already have rustup)
curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"

# 2. Clone and install
git clone https://github.com/anthonybarros/ccnotify.git
cd ccnotify
./scripts/install-local.sh
```

`install-local.sh` does four things (each is idempotent — rerunning is safe):

1. `cargo build --release` (the first build takes a few minutes)
2. copies `ccnotify` and `ccnotify-overlay` side by side into `~/.local/bin`
   (override with `CCNOTIFY_INSTALL_DIR=/some/dir ./scripts/install-local.sh`)
3. adds `~/.local/bin` to your `PATH` in your shell rc if it isn't already
4. runs `ccnotify setup`, which:
   - installs the hook forwarder to `~/.claude/hooks/ccnotify-forward.sh`
   - adds `PreToolUse` / `Notification` / `Stop` / `UserPromptSubmit` hook
     entries to your global `~/.claude/settings.json`
   - adds `alias claude="ccnotify claude"` to your shell rc (zsh/bash/fish)

Then **open a new terminal** (so the alias and PATH load) and run Claude Code
as you always have:

```sh
claude
```

A colored pill appears near the top-right of your screen. That's it.

The hooks are safe to keep installed globally: when Claude Code starts
*without* the wrapper (scripts, CI, `command claude`), the forwarder sees no
`$CCNOTIFY_PORT` and exits instantly as a no-op.

### Manual install (if you prefer to see every step)

```sh
git clone https://github.com/anthonybarros/ccnotify.git
cd ccnotify
cargo build --release

# The wrapper finds the overlay binary NEXT TO ITSELF — keep them together.
mkdir -p ~/.local/bin
install -m 755 target/release/ccnotify ~/.local/bin/
install -m 755 target/release/ccnotify-overlay ~/.local/bin/

# Wire up hooks + shell alias (prints exactly what it changed)
~/.local/bin/ccnotify setup
```

### Uninstall

```sh
ccnotify uninstall                     # removes hook entries, hook script, and the alias
rm -f ~/.local/bin/ccnotify ~/.local/bin/ccnotify-overlay
```

## Using it

```sh
claude                           # normal usage — aliased to: ccnotify claude
ccnotify --alias backend claude  # override the overlay label for this session
ccnotify setup                   # (re)install hooks + shell alias
ccnotify uninstall               # remove hooks + shell alias
```

Per-project alias without a flag — drop a `.ccnotify.json` in the project root:

```json
{ "alias": "backend" }
```

### The overlay

The compact pill shows a colored identity dot, the session alias, and the
ambient status: blue **working…**, amber **needs your input** (hard to miss
peripherally), green **idle — turn finished**. A native OS notification also
fires as a secondary nudge. Drag the pill by its `⋮` grip; its position is
remembered per alias, and new sessions stagger from the top-right corner so
overlays don't stack.

**Click the pill to expand it in place:**

- On a **permission request**: the per-tool detail (command / diff /
  preview) with **Allow** / **Deny** buttons and an optional deny reason.
- When **idle**: Claude's last message and a reply box (**⌘↩** to send).
- Header controls: **▤** toggles a live, escape-stripped view of the
  session's recent terminal output (refreshes every 2s, follows the tail
  unless you scroll up) · **▾** collapses · **✕** quits the Claude session
  (click twice — it arms red first so a stray click can't kill a session).

If nobody answers a permission request within ~9.5 minutes, the hook
returns no decision and Claude Code falls back to its normal terminal
prompt — nothing is silently denied.

Note: the `PreToolUse` hook fires for **every** tool call, including ones
Claude Code would normally auto-allow, so the overlay currently asks more
often than the raw terminal would. A per-tool auto-allow config is on the
roadmap.

## How it works (short version)

```
you type `claude` (shell-aliased to the wrapper)
  -> wrapper spawns the real claude in a pty it owns, sets $CCNOTIFY_PORT
  -> Claude Code hooks fire on PreToolUse / Notification / Stop
  -> hook script POSTs the event to the wrapper's 127.0.0.1-only HTTP server
  -> permission events block until you click Allow/Deny on the overlay;
     the decision is returned as the hook's response
  -> text replies are written directly into the pty's stdin
```

Each session is fully self-contained: its own loopback port, its own
random shared-secret token (nothing else on the machine can post fake
events or read output), its own overlay process. Full detail in
[docs/architecture.md](docs/architecture.md).

## Verifying your install

Run the automated end-to-end check (uses a fake `claude`, touches no config):

```sh
cargo build          # smoke test uses the debug binary
./scripts/smoke-test.sh
```

You should see `--- SMOKE TEST PASSED ---`. Then test for real: run
`claude`, ask it to run a shell command, and answer the permission request
from the overlay.

## Documentation

- [docs/architecture.md](docs/architecture.md) — components, event flow,
  HTTP API, security model, resource budget
- [docs/development.md](docs/development.md) — building, testing, project
  layout, releasing
- [docs/troubleshooting.md](docs/troubleshooting.md) — common problems and
  fixes
- [spec.md](spec.md) — the original design spec

## Status / roadmap

- [x] Permission round-trip (blocking PreToolUse hook → overlay → decision)
- [x] Overlay UI: states, per-tool formatters, drag + per-alias position
- [x] Idle/Stop handling with replies into the pty
- [x] Live terminal-output view and session quit from the overlay
- [x] `ccnotify setup` / `uninstall`
- [ ] Per-tool auto-allow config (e.g. always allow `Read`, always ask `Bash`)
- [ ] Linux validation; Windows support
- [ ] Signed/notarized macOS builds
- [ ] Stretch: customizable companion appearance

## License

[MIT](LICENSE)
