#!/bin/sh
# dtk installer. Downloads the release for this machine, checks its SHA-256, installs the binary.
#
#   curl -fsSL https://raw.githubusercontent.com/amaye15/dtk/main/install.sh | sh
#
# Options (environment variables):
#   DTK_VERSION      a tag such as v0.3.0 (default: the latest release)
#   DTK_INSTALL_DIR  where to put the binary (default: $HOME/.local/bin)
set -eu

REPO="amaye15/dtk"
VERSION="${DTK_VERSION:-latest}"
DEST="${DTK_INSTALL_DIR:-$HOME/.local/bin}"

fail() { printf 'dtk install: %s\n' "$1" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "$1 is required but was not found"; }
need curl
need tar

case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux)  os="unknown-linux-musl" ;;
  *) fail "unsupported system $(uname -s); build from source with: cargo install dtk" ;;
esac
case "$(uname -m)" in
  arm64|aarch64) arch="aarch64" ;;
  x86_64|amd64)  arch="x86_64" ;;
  *) fail "unsupported CPU $(uname -m); build from source with: cargo install dtk" ;;
esac
target="$arch-$os"

if [ "$VERSION" = "latest" ]; then
  VERSION="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)"
  [ -n "$VERSION" ] || fail "could not find the latest release of $REPO"
fi

name="dtk-$VERSION-$target"
base="https://github.com/$REPO/releases/download/$VERSION"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

printf 'dtk install: downloading %s\n' "$name"
curl -fsSL -o "$tmp/$name.tar.gz" "$base/$name.tar.gz" || fail "no release file for $target at $VERSION"
curl -fsSL -o "$tmp/$name.tar.gz.sha256" "$base/$name.tar.gz.sha256" || fail "no checksum file for $name"

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$tmp" && sha256sum -c "$name.tar.gz.sha256" >/dev/null) || fail "checksum does not match; not installing"
else
  (cd "$tmp" && shasum -a 256 -c "$name.tar.gz.sha256" >/dev/null) || fail "checksum does not match; not installing"
fi
printf 'dtk install: checksum ok\n'

tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
mkdir -p "$DEST"
install -m 755 "$tmp/$name/dtk" "$DEST/dtk"
printf 'dtk install: installed %s to %s\n' "$("$DEST/dtk" --version)" "$DEST/dtk"

case ":$PATH:" in
  *":$DEST:"*) ;;
  *) printf 'dtk install: %s is not on your PATH. Add it, for example: export PATH="%s:$PATH"\n' "$DEST" "$DEST" ;;
esac
printf 'dtk install: next, run: dtk init --global   (then dtk doctor)\n'
