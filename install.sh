#!/bin/sh
# ccnotify one-line installer (macOS / Linux)
#   curl -fsSL https://raw.githubusercontent.com/<you>/ccnotify/main/install.sh | sh
set -eu

REPO="${CCNOTIFY_REPO:-anthonybarros/ccnotify}"
INSTALL_DIR="${CCNOTIFY_INSTALL_DIR:-$HOME/.local/bin}"

os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Darwin) target="macos" ;;
  Linux)  target="linux" ;;
  *) echo "Unsupported OS: $os (see README for building from source)"; exit 1 ;;
esac
case "$arch" in
  arm64|aarch64) target="$target-arm64" ;;
  x86_64)        target="$target-x86_64" ;;
  *) echo "Unsupported architecture: $arch"; exit 1 ;;
esac

asset="ccnotify-$target.tar.gz"
url="https://github.com/$REPO/releases/latest/download/$asset"

echo "Downloading $asset from $REPO ..."
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "$url" -o "$tmp/$asset"
tar -xzf "$tmp/$asset" -C "$tmp"

mkdir -p "$INSTALL_DIR"
install -m 755 "$tmp/ccnotify" "$INSTALL_DIR/ccnotify"
install -m 755 "$tmp/ccnotify-overlay" "$INSTALL_DIR/ccnotify-overlay"
echo "Installed ccnotify and ccnotify-overlay to $INSTALL_DIR"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "NOTE: $INSTALL_DIR is not on your PATH — add it to your shell rc." ;;
esac

if [ "$os" = "Darwin" ]; then
  # Unsigned binaries: clear the quarantine bit so Gatekeeper doesn't
  # block first run (see README for details).
  xattr -d com.apple.quarantine "$INSTALL_DIR/ccnotify" 2>/dev/null || true
  xattr -d com.apple.quarantine "$INSTALL_DIR/ccnotify-overlay" 2>/dev/null || true
fi

echo "Running ccnotify setup ..."
"$INSTALL_DIR/ccnotify" setup
