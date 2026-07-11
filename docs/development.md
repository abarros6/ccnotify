# Development

## Project layout

| path | what it is |
| --- | --- |
| `wrapper/` | the `ccnotify` CLI: pty wrapper, loopback HTTP server, `setup`/`uninstall` |
| `overlay/` | the Tauri overlay app (`ccnotify-overlay`): Rust shell + `ui/index.html` |
| `common/` | shared types: session identity, palette, event/state payloads |
| `hooks/notify-forward.sh` | hook forwarder; embedded into the CLI via `include_str!` and installed by setup |
| `scripts/install-local.sh` | build + install + `ccnotify setup` in one step |
| `scripts/smoke-test.sh` | end-to-end test of the permission round-trip with a fake `claude` |
| `.github/workflows/release.yml` | tag-triggered release build matrix |

One Cargo workspace; the Rust version is pinned in `rust-toolchain.toml`
(rustup picks it up automatically) and `Cargo.lock` is committed.

## Building

```sh
cargo build            # debug — what the smoke test uses
cargo build --release  # optimized; LTO makes the overlay link slow (~3–6 min)
cargo test
```

The two binaries land in `target/{debug,release}/`. The wrapper locates
`ccnotify-overlay` **in its own directory**, so when testing an installed
copy, always ship them together. Running from `target/debug` works as-is.

Linux needs the webview dev packages first:

```sh
sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev
```

## Testing

**Smoke test** (no real Claude, no config changes):

```sh
cargo build && ./scripts/smoke-test.sh
```

It PATH-shims a fake `claude` that plays both Claude and the hook script,
then verifies: env inheritance into the pty child, the blocking
`PreToolUse` round-trip, the decision payload format, reply injection
into the pty's stdin, and token auth (403 on bad token).

**Manual testing against a real session** without touching your global
config: skip the shell alias and run the wrapper directly —

```sh
./target/debug/ccnotify claude
```

(The hooks in `~/.claude/settings.json` still need to exist for events to
flow; `ccnotify setup` installs them and `ccnotify uninstall` removes
them.)

**Driving a session from scripts**: every endpoint is plain HTTP. Grab
the port/token from a child process env (`ps -wwE -p <pid>` on macOS) and
e.g. approve a pending permission:

```sh
curl -X POST -H "X-CCNotify-Token: $TOKEN" \
  --data '{"id":1,"decision":"allow"}' \
  "http://127.0.0.1:$PORT/overlay/decision"
```

## Working on the overlay UI

The whole UI is one static file, `overlay/ui/index.html` (no npm, no
bundler). It's compiled into the binary by Tauri's codegen, so after
editing it you must rebuild `ccnotify-overlay`. The JS talks to Rust via
`window.__TAURI__.core.invoke` (`withGlobalTauri` is enabled); the Rust
commands live in `overlay/src/main.rs` and do all HTTP to the wrapper.

To add a formatter for a new tool, extend `formatTool()` in
`index.html` — the payload is the raw `tool_input` object from the
`PreToolUse` hook.

## Releasing

Releases are built by `.github/workflows/release.yml` on version tags:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The matrix builds macOS (arm64 + x86_64), Linux x86_64, and a Windows
artifact (which will not be functional until Windows support lands), and
attaches `ccnotify-<target>.tar.gz` archives containing both binaries to
the GitHub Release. `install.sh` at the repo root downloads the latest
release asset for the current OS/arch — keep its asset naming in sync
with the workflow.

macOS binaries are unsigned for now; see the Gatekeeper note in
[troubleshooting](troubleshooting.md).
