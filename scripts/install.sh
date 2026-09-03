#!/usr/bin/env bash
# Install gitgui from GitHub releases.
set -euo pipefail

REPO="${GITGUI_REPO:-antonellof/gitgui}"
VERSION="${GITGUI_VERSION:-latest}"
INSTALL_DIR="${GITGUI_INSTALL_DIR:-$HOME/.local/bin}"

os="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch="$(uname -m)"
case "$arch" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64) arch="arm64" ;;
  *)
    echo "unsupported architecture: $arch" >&2
    exit 1
    ;;
esac
case "$os" in
  darwin) platform="macos" ;;
  linux) platform="linux" ;;
  *)
    echo "unsupported OS: $os" >&2
    exit 1
    ;;
esac

if [ "$VERSION" = "latest" ]; then
  VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\([^"]*\)".*/\1/p' \
    | head -1)"
  if [ -z "$VERSION" ]; then
    echo "could not resolve latest release for ${REPO}" >&2
    exit 1
  fi
fi

asset="gitgui-${VERSION}-${platform}-${arch}"
url="https://github.com/${REPO}/releases/download/v${VERSION}/${asset}.tar.gz"

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
echo "downloading $url"
curl -fsSL "$url" -o "$tmpdir/archive.tar.gz"
tar xzf "$tmpdir/archive.tar.gz" -C "$tmpdir"
bin="$(find "$tmpdir" -maxdepth 1 -type f -name 'gitgui-*' | head -1)"
test -n "$bin"
mkdir -p "$INSTALL_DIR"
install -m 755 "$bin" "$INSTALL_DIR/gitgui"
echo "installed $INSTALL_DIR/gitgui"
if ! command -v gitgui >/dev/null 2>&1; then
  echo "add $INSTALL_DIR to your PATH"
fi
