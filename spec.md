# ccnotify — spec

## Goal

Let a person step away from the terminal while Claude Code is working and get an
instant, actionable popup on the same machine whenever Claude Code needs their
input — a permission request, a clarifying question, or the end of a turn.
The reply happens from the popup, without switching back to the terminal.

Design target: functionally close to OpenAI's Codex Pets — a persistent,
always-on-top overlay that survives window/app switches, shows an ambient
status (working / needs input / idle), and lets the person respond by
clicking directly on it rather than hunting for a notification in a tray.
The difference for this tool is depth of content: the overlay needs to
surface the actual command/diff being requested, not just a status icon.

Non-goals for v1: no remote/mobile notifications, no multi-machine sync. This
is a single-machine, single-user tool. Cosmetic customization (choosing or
generating a companion appearance) is a stretch goal, not core — see
milestones.

**Hard requirement carried through every design decision below: this has to
be lightweight.** One overlay per concurrent Claude Code session means the
runtime cost multiplies per session — that ruled out Electron (see Stack)
and should keep ruling out anything with a similar per-window cost as the
project grows.

## Core architecture

A thin wrapper process replaces `claude` in the person's shell. It spawns the
real Claude Code binary inside a pseudo-terminal (pty) it owns, so from the
user's point of view nothing changes — same terminal, same interaction. Because
the wrapper owns the pty, it can write directly into Claude Code's stdin when a
reply comes back from the overlay. No tmux, no shared session-name bookkeeping,
no central daemon.

```
you type `claude` (shell-aliased to the wrapper)
  -> wrapper spawns real claude in a pty, sets $CCNOTIFY_PORT env var
  -> Claude Code inherits $CCNOTIFY_PORT; hook fires on PreToolUse / Notification
  -> hook script reads $CCNOTIFY_PORT, POSTs event JSON to http://127.0.0.1:<port>
  -> wrapper's local HTTP server receives it, tells its overlay to update
  -> permission reply -> HTTP response returned, unblocking the hook call
  -> text reply (idle/clarifying question) -> written straight into the pty's stdin
```

Each wrapper instance is self-contained: its own port, its own overlay
window, its own identity (see below). Running several `claude` sessions at
once just works, with no coordination needed between them.

### Session identity: alias & color

Each session's overlay needs to be visually distinguishable at a glance,
per your call to go with one overlay per session:

- **Alias**: defaults to the basename of the current working directory (so
  a session in `~/projects/api-server` is labeled "api-server" with zero
  configuration). Overridable with a flag (`ccnotify claude --alias
  backend`) or a small per-project config file, for cases where the folder
  name isn't distinctive enough.
- **Color**: deterministically derived by hashing the alias into a small,
  fixed palette (similar to how git/tmux assign consistent colors) — so the
  same project gets the same color every time you launch it, without
  needing to store a color mapping anywhere. Collisions across a large
  palette are rare and low-stakes, since the alias text is the actual
  disambiguator; color is a fast visual cue, not the source of truth.

## Components

### 1. `wrapper/` — the `ccnotify` CLI

- Spawns `claude <args>` in a pty via Rust's `portable-pty` crate,
  forwarding all args through unmodified.
- Picks a free local port, sets it as `$CCNOTIFY_PORT` in the child's env
  before spawning, along with the session's alias/color and a per-instance
  shared-secret token.
- Relays the pty's stdout to the real terminal and the real terminal's
  stdin to the pty, transparently — the person should see and type exactly
  as if they'd run `claude` directly.
- Runs a small HTTP server bound to `127.0.0.1` only, listening on that
  port, guarded by the shared-secret token so nothing else on the machine
  can post fake events to it.
- On receiving a hook event:
  - **Permission event** (`PreToolUse`): hold the HTTP request open, tell
    the overlay to switch to "needs input" with the formatted content, and
    once the person responds, reply with `{"decision": "allow"}` or
    `{"decision": "deny", "reason": "..."}` as the HTTP response body. Bump
    the corresponding hook's `timeout` in the generated config to several
    minutes so it doesn't lapse waiting.
  - **Notification event** (`idle_prompt`, or `Stop`): tell the overlay to
    switch state and show the reply UI, and if the person types a reply,
    write it into the pty's stdin followed by the terminal's newline
    sequence, then respond 200 to close out the hook call (no decision
    needed here).
- Exit cleanly when the child `claude` process exits, closing its overlay
  and forwarding its exit code.

### 2. `hooks/notify-forward` — the shared hook script

Installed once into `~/.claude/hooks/` by the setup command. Its only job:
read `$CCNOTIFY_PORT` and `$CCNOTIFY_TOKEN` from the environment (inherited
from the wrapper), read the hook JSON from stdin, and POST it to
`http://127.0.0.1:$CCNOTIFY_PORT/event` with the token as a header. If
`$CCNOTIFY_PORT` isn't set (i.e. Claude Code wasn't launched through the
wrapper), exit 0 immediately and do nothing — this keeps the tool safe to
have globally configured even when someone runs `claude` directly without
the wrapper for some reason.

Implement as the `command` hook type (a shell script) — simpler and more
portable than the `http` hook type, and one less thing to get wrong per
platform.

### 3. `overlay/` — the persistent floating companion (Tauri)

One instance per session, spawned by that session's wrapper process and
closed when it exits. A small, frameless, always-on-top window that stays
visible across app/window switches for the whole life of the session, the
same way Codex Pets does. Draggable; position persists between runs of the
same alias, and multiple overlays should default to a staggered position so
they don't stack directly on top of each other.

Shows the alias and color prominently (a colored border/dot plus the label
text) so it's clear which project/session each overlay belongs to at a
glance.

**States:**
- **Working** — a quiet, low-cost ambient indicator (a static icon or a
  cheap CSS transition, not a continuous animation loop) while Claude Code
  is actively running.
- **Needs input** — the overlay's color/icon changes to something hard to
  miss peripherally (red/amber, echoing Codex's red-clock pattern) the
  moment a `PreToolUse` or `Notification` hook fires.
- **Idle / done** — a calmer indicator (green check equivalent) once a turn
  finishes and Claude Code is waiting for the next prompt.

**Interaction:** clicking the overlay while in "needs input" expands it in
place into the actual decision UI — no separate window to hunt for.
Content differs by event type and, for permission events, by `tool_name` in
the payload, so this needs a formatter per tool rather than one generic
template:

- **Bash**: monospace block with the literal command, plus the
  `description` field if present. Allow / Deny buttons.
- **Edit / MultiEdit**: render `old_string` / `new_string` as an actual
  unified diff (red/green lines) — not raw JSON. Allow / Deny buttons.
- **Write**: file path as header, preview of new content (truncate if long).
  Allow / Deny buttons.
- **WebFetch / other tools**: show the URL or the most decision-relevant
  input field for that tool. Allow / Deny buttons.
- **Notification (idle_prompt) / Stop**: show whatever text Claude last
  said (from the hook payload / transcript), with a free-text reply field
  and a send button.

A native OS notification can still fire alongside the state change as a
secondary nudge (useful if an overlay is off-screen or covered), but it's
not the primary mechanism.

**Resource budget (hold implementation to this, don't just hope for it)**:
target roughly 20-40MB memory and near-zero CPU per overlay at idle
("working" or "idle" state, not expanded). Verify this with the OS's
actual process monitor during milestone 2 for at least 3-4 concurrent
overlays, not just one — the whole point of the requirement is that it
holds up under realistic concurrent use.

**Stretch goal, not v1**: customizable/generatable companion appearance
(Codex's `/hatch`-style flow). The functional overlay — status, identity,
click-to-respond — should be solid and proven lightweight before any of
that is worth doing.

### 4. `setup/` — the `ccnotify setup` command

Run once. It should:
- Write the hook entries into the person's **global** `~/.claude/settings.json`
  (not per-project) — `PreToolUse` (matcher covering all tools that need
  permission), `Notification` (matchers `idle_prompt`, `permission_prompt`),
  and `Stop`. All pointed at `notify-forward`.
- Detect the person's shell (zsh/bash/fish) and append an alias —
  `alias claude="ccnotify claude"` — to the appropriate rc file, so typing
  `claude` afterward "just works" with no behavior change required from the
  person.
- Print a clear summary of what was changed and how to undo it (`ccnotify
  uninstall` should reverse both edits cleanly).

## Stack

Rust throughout — this is the change from the earlier draft, driven
directly by the lightweight/multi-overlay requirement:

- **Wrapper CLI**: Rust, using `portable-pty` for the pseudo-terminal
  instead of Node's `node-pty`.
- **Overlay**: Tauri, not Electron. Tauri uses the OS's native webview
  (WebKit on macOS, WebView2 on Windows, WebKitGTK on Linux) rather than
  bundling a full Chromium copy per window, so N overlay windows cost a
  small fraction of what N Electron windows would. Binaries are also
  dramatically smaller (roughly single-digit MB vs Electron's 50-150MB).
- One shared Rust workspace (Cargo workspace, analogous to an npm
  monorepo) — the wrapper and the overlay can share the session-identity
  and event-payload types directly instead of duplicating them across a
  Node/Electron split and a separate native module.
- This also removes the earlier Electron-ABI native-rebuild problem
  entirely (see Distribution) — there's no Node runtime or Electron
  bundling step to fight with.

## Distribution & installation

Simpler than the earlier Electron-based plan, precisely because there's no
native Node module to rebuild against a bundling runtime's ABI anymore.

- **CI builds prebuilt binaries.** A GitHub Actions matrix
  (`macos-latest`, `windows-latest`, `ubuntu-latest`) runs a standard
  `cargo build --release` (plus Tauri's bundler for the overlay) on every
  tagged release, producing a per-OS artifact: `.dmg`/`.zip` (macOS), an
  installer `.exe` (Windows), `.AppImage`/`.deb` (Linux). Attach these to
  the GitHub Release.
- **A one-line install script** (`install.sh` for macOS/Linux,
  `install.ps1` for Windows) detects the OS/arch, downloads the matching
  release asset from GitHub, places the binary on `PATH`, and then runs
  `ccnotify setup` automatically. This is the path the README should lead
  with:
  ```
  curl -fsSL https://raw.githubusercontent.com/<you>/ccnotify/main/install.sh | sh
  ```
- **"Build from source" stays documented but secondary** — for
  contributors: `git clone`, then `cargo build --release` (standard Rust
  toolchain, no extra native-rebuild step needed).
- **macOS Gatekeeper**: an unsigned/unnotarized app will get a "can't be
  opened" warning on first run. Document the workaround (right-click →
  Open, or `xattr -d com.apple.quarantine`) clearly in the README. Apple
  notarization is a real option later but costs a paid developer account —
  not a v1 blocker, just something to flag so it isn't a surprise.
- Pin the Rust toolchain version (`rust-toolchain.toml`) and commit
  `Cargo.lock`, so CI and every contributor's local build produce
  consistent binaries.

## Multi-session handling

Decided: one overlay per session, per your preference, distinguished by
the alias/color scheme above rather than a shared queue. Since each
wrapper instance is independent with its own port, token, and overlay,
running several `claude` sessions in different terminal tabs at once
requires zero extra configuration or coordination between them.

## Suggested build order (milestones)

1. **Permission round-trip only, macOS, no overlay UI yet**: wrapper + hook
   script + a bare-bones CLI prompt (even just printing to a log and
   reading stdin) standing in for the overlay, to prove the
   blocking-HTTP-hook mechanism actually works end to end.
2. **Real overlay UI** (Tauri): the persistent always-on-top companion
   window with alias/color identity and working/needs-input/idle states,
   expanding in place into the per-tool formatters for permission events
   when clicked. Verify the resource budget with several concurrent
   overlays before moving on.
3. **Idle/Stop event handling**: text replies written into the pty's stdin.
4. **`ccnotify setup` / `uninstall`**: automate the hook config + shell
   alias changes.
5. **Cross-platform**: Windows and Linux support for the pty spawning,
   notification APIs, and shell alias detection.
6. **Distribution**: CI release matrix with Tauri's bundler, the install
   script, and README instructions for both the quick install and building
   from source.

## Open questions to resolve during implementation

- Exact hook `timeout` value that balances "long enough to actually
  respond" against "don't leave Claude Code hanging indefinitely if the
  person is away" — consider a configurable default with a sane fallback
  (e.g. auto-deny after N minutes with a clear notification that it
  happened).
- How much transcript context to show for `idle_prompt` / `Stop` events —
  the full last assistant message, or a truncated summary.
- Whether to support a config file for per-tool overlay behavior (e.g.
  always auto-allow `Read`, always prompt for `Bash`).
- Exact color palette size/values for the alias-color scheme, and whether
  to let the person override a specific alias's color if they dislike the
  hashed result.
