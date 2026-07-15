# Troubleshooting

## `claude` doesn't go through ccnotify

- The alias only loads in **new** shells. Open a new terminal or
  `source ~/.zshrc` (or your shell's rc).
- Check the alias resolves: `type claude` should print
  `claude is an alias for ccnotify claude`.
- Scripts and other programs that invoke `claude` bypass shell aliases —
  that's by design. Use `ccnotify claude` explicitly where you want the
  overlay.

## No overlay appears

- The wrapper looks for `ccnotify-overlay` **in the same directory as its
  own binary**. `ls "$(dirname "$(which ccnotify)")"` must show both. If
  the overlay is missing the wrapper still runs, printing
  `overlay binary not found, running headless`.
- macOS Gatekeeper may block unsigned binaries downloaded from a release
  (building from source is not affected). Fix:

  ```sh
  xattr -d com.apple.quarantine "$(which ccnotify)" "$(which ccnotify-overlay)"
  ```

## The overlay never leaves "working…" / events don't arrive

- Hooks must be installed: `ccnotify setup` (rerunning is safe). Verify
  with:

  ```sh
  python3 -c "import json;print(json.dumps(json.load(open('$HOME/.claude/settings.json'))['hooks'],indent=1))" | grep -c ccnotify-forward
  ```

  Expect `4` (PreToolUse, Notification, Stop, UserPromptSubmit).
- Hooks are snapshotted when a Claude Code session starts — sessions that
  were already running when you ran `setup` won't fire them. Start a new
  session.
- The hook script needs `curl` on `PATH`.

## A permission request appeared in the terminal instead of the overlay

That's the designed fallback: if nothing answers within ~9.5 minutes (or
the overlay/server is unreachable), the hook returns no decision and
Claude Code falls back to its normal terminal prompt. Nothing is ever
silently auto-denied.

## The overlay asks about tools Claude would normally auto-allow

The `PreToolUse` hook fires for **every** tool call, so before checking
anything else the wrapper matches it against the session's own
`permissions.allow` / `.deny` / `defaultMode` (merged from
`.claude/settings.local.json`, `.claude/settings.json`, and
`~/.claude/settings.json`) and decides instantly if a rule applies — no
overlay popup, same as the raw terminal. If you're still getting asked
about something you expected to be auto-allowed, the rule likely doesn't
match: check the exact string in `permissions.allow` (e.g. `Bash(git
diff:*)` only covers commands starting with `git diff`, not other `git`
subcommands) or that the settings file it's in is actually on the merge
path above. Denying from the overlay tells Claude why (your optional
reason), so it can adjust course.

## A leftover overlay is stuck on screen

Shouldn't happen anymore — an overlay that can't reach its wrapper for
~12 seconds closes itself. If you ever need a hammer:

```sh
pkill -f ccnotify-overlay      # just overlays
pkill -x ccnotify              # wrappers too (ends their claude sessions)
```

## Quitting from the overlay

The ✕ button in the expanded header needs **two clicks** (it arms red
first). This ends the whole Claude session — same as exiting Claude in
the terminal — and the overlay closes with it.

The ⏏ button next to it closes just the overlay window/process — the
Claude session keeps running untouched in its terminal. Since the
wrapper's `PreToolUse` hook still falls back to Claude Code's normal
terminal prompt after ~9.5 minutes with no overlay to answer it, the
session works exactly like a plain `claude` session once the overlay is
gone. There's currently no way to bring that session's overlay back
short of restarting the session.

## Terminal looks broken after a crash

The wrapper puts your terminal in raw mode and restores it on exit. If it
was killed hard (`kill -9`) the restore may not run; fix the terminal
with:

```sh
stty sane
```

## Where things live

| path | what |
| --- | --- |
| `~/.local/bin/ccnotify`, `~/.local/bin/ccnotify-overlay` | binaries |
| `~/.claude/hooks/ccnotify-forward.sh` | hook forwarder |
| `~/.claude/settings.json` | hook entries (under `"hooks"`) |
| `~/.zshrc` / `~/.bashrc` / fish config | `claude` alias, between `# >>> ccnotify >>>` markers |
| `~/.ccnotify/positions.json` | remembered overlay positions per alias |

`ccnotify uninstall` removes the hook entries, the hook script, and the
alias block; delete the binaries and `~/.ccnotify` by hand if you want a
complete wipe.
