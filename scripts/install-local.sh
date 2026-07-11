#!/bin/bash
# Build ccnotify from this checkout and set it up on this machine:
#   1. cargo build --release
#   2. install ccnotify + ccnotify-overlay side by side on PATH
#   3. make sure the install dir is on PATH for future shells
#   4. run `ccnotify setup` (hook script + settings.json entries + shell alias)
#
# Everything setup does is reversible with: ccnotify uninstall
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"
install_dir="${CCNOTIFY_INSTALL_DIR:-$HOME/.local/bin}"

if ! command -v cargo >/dev/null 2>&1; then
  # rustup installs land here but only new shells pick them up
  [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
fi
command -v cargo >/dev/null 2>&1 || {
  echo "cargo not found — install Rust first: https://rustup.rs"; exit 1;
}

echo "==> Building release binaries (the first release build takes a few minutes)"
cargo build --release --manifest-path "$root/Cargo.toml"

echo "==> Installing to $install_dir"
mkdir -p "$install_dir"
install -m 755 "$root/target/release/ccnotify" "$install_dir/ccnotify"
install -m 755 "$root/target/release/ccnotify-overlay" "$install_dir/ccnotify-overlay"

# The `claude` alias that `ccnotify setup` writes runs plain `ccnotify`,
# so the install dir has to be on PATH in future shells.
case ":$PATH:" in
  *":$install_dir:"*) ;;
  *)
    rc=""
    case "${SHELL:-}" in
      */zsh)  rc="$HOME/.zshrc" ;;
      */bash) rc="$HOME/.bashrc" ;;
      */fish) rc="$HOME/.config/fish/config.fish" ;;
    esac
    if [ -n "$rc" ] && ! grep -qs "# ccnotify-path" "$rc"; then
      if [ "${rc##*/}" = "config.fish" ]; then
        printf '\n# ccnotify-path\nfish_add_path %s\n' "$install_dir" >> "$rc"
      else
        printf '\n# ccnotify-path\nexport PATH="%s:$PATH"\n' "$install_dir" >> "$rc"
      fi
      echo "==> Added $install_dir to PATH in $rc"
    elif [ -z "$rc" ]; then
      echo "NOTE: add $install_dir to your PATH manually (unrecognized shell)."
    fi
    ;;
esac

echo "==> Running ccnotify setup"
"$install_dir/ccnotify" setup

echo
echo "Done. Open a NEW terminal (so the alias and PATH load) and run: claude"
echo "To try several sessions at once, run claude in different project folders."
