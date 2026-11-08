#!/bin/sh
# dtk installer for macOS and Linux (and WSL). Downloads the release for this machine, checks
# its SHA-256, installs the binary. Windows: use install.ps1.
#
#   curl -fsSL https://raw.githubusercontent.com/amaye15/dtk/main/install.sh | sh
#
# Options (environment variables):
#   DTK_VERSION        a tag such as v0.4.0 (default: the latest release)
#   DTK_INSTALL_DIR    where to put the binary (default: $HOME/.local/bin)
#   DTK_SKIP_CHECKSUM  set to 1 to skip the SHA-256 check (not recommended)
set -eu

REPO="amaye15/dtk"
VERSION="${DTK_VERSION:-latest}"
DEST="${DTK_INSTALL_DIR:-$HOME/.local/bin}"

info() { printf 'dtk install: %s\n' "$1"; }
fail() { printf 'dtk install: %s\n' "$1" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "$1 is required but was not found"; }
need curl
need tar

case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux)  os="unknown-linux-musl" ;;
  *) fail "unsupported system $(uname -s). On Windows use install.ps1; or build with: cargo install dtk" ;;
esac
case "$(uname -m)" in
  arm64|aarch64) arch="aarch64" ;;
  x86_64|amd64)  arch="x86_64" ;;
  *) fail "unsupported CPU $(uname -m); build from source with: cargo install dtk" ;;
esac
target="$arch-$os"

if [ "$VERSION" = "latest" ]; then
  # The redirect of /releases/latest names the newest tag and does not use the API quota.
  final="$(curl -fsSL -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" 2>/dev/null || true)"
  VERSION="${final##*/}"
  case "$VERSION" in v[0-9]*) ;; *)
    VERSION="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)"
    [ -n "$VERSION" ] || fail "could not find the latest release of $REPO (GitHub may be rate-limiting; set DTK_VERSION=vX.Y.Z to pin)" ;;
  esac
fi

name="dtk-$VERSION-$target"
archive="$name.tar.gz"
base="https://github.com/$REPO/releases/download/$VERSION"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

info "downloading $archive"
curl -fsSL -o "$tmp/$archive" "$base/$archive" || fail "no release file for $target at $VERSION"

if [ "${DTK_SKIP_CHECKSUM:-0}" = "1" ]; then
  info "DTK_SKIP_CHECKSUM=1 is set: SKIPPING the checksum check (not recommended)"
else
  # checksums.txt lists every release file (as rtk's does); older releases only have a .sha256 file.
  if curl -fsSL -o "$tmp/checksums.txt" "$base/checksums.txt" 2>/dev/null && grep -q "  $archive\$" "$tmp/checksums.txt"; then
    grep "  $archive\$" "$tmp/checksums.txt" > "$tmp/expected"
  elif curl -fsSL -o "$tmp/expected" "$base/$archive.sha256" 2>/dev/null; then
    :
  else
    fail "could not download a checksum for $archive; refusing to install an unverified file (DTK_SKIP_CHECKSUM=1 bypasses this)"
  fi
  want="$(awk '{print $1}' "$tmp/expected" | head -n 1)"
  if command -v sha256sum >/dev/null 2>&1; then
    got="$(sha256sum "$tmp/$archive" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    got="$(shasum -a 256 "$tmp/$archive" | awk '{print $1}')"
  else
    fail "neither sha256sum nor shasum is available, so the checksum cannot be checked"
  fi
  [ -n "$want" ] && [ "$want" = "$got" ] || fail "checksum mismatch (expected $want, got $got); not installing"
  info "checksum ok"
fi

mkdir -p "$tmp/x"
tar -xzf "$tmp/$archive" -C "$tmp/x"
# 0.4.0 and later keep the binary at the top of the archive; older archives had a folder.
bin="$(find "$tmp/x" -type f -name dtk | head -n 1)"
[ -n "$bin" ] || fail "the archive does not contain dtk"
mkdir -p "$DEST"
install -m 755 "$bin" "$DEST/dtk"
info "installed $("$DEST/dtk" --version) to $DEST/dtk"

case ":$PATH:" in
  *":$DEST:"*) ;;
  *) info "$DEST is not on your PATH. Add it, for example: echo 'export PATH=\"$DEST:\$PATH\"' >> ~/.zshrc" ;;
esac

# Register dtk, so that nothing is left to do. Skip with DTK_NO_INIT=1. dtk init connects Claude
# Code and every other agent that it finds (Codex CLI, Gemini CLI, Copilot CLI, OpenCode, Kilo
# Code, Pi, Hermes, Mistral Vibe), keeps the settings that are there, and keeps a backup. With
# no agent on the machine it changes nothing and says so.
if [ "${DTK_NO_INIT:-}" = 1 ]; then
  info "DTK_NO_INIT=1 is set: dtk is not registered. When you want it: dtk init --global"
else
  info "registering dtk with the agents on this machine (backups are kept; undo with: dtk init --uninstall)"
  if "$DEST/dtk" init --global; then
    info "done. Start a new session in each agent; check the setup with: dtk verify"
  else
    info "could not register dtk with every agent. Read the message above, or run: $DEST/dtk init --global"
  fi
fi
