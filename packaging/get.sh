#!/usr/bin/env bash
# terminal-windows installer for macOS and Linux.
#
#   curl -fsSL https://github.com/dlfkdLR/terminal-windows/releases/latest/download/get.sh | bash
#
# Downloads the latest release for this machine, verifies its SHA-256 against
# the release's SHA256SUMS, and installs one binary into ~/.local/bin.
# Needs no administrator rights and does not edit shell configuration.
# Options (environment): TW_PREFIX=/abs/path (default ~/.local), TW_VERSION=v0.1.0, TW_REPO=owner/name,
# TW_SKIP_DEPS=1 (do not install QEMU and swtpm with Homebrew)
set -euo pipefail

REPO="${TW_REPO:-dlfkdLR/terminal-windows}"
PREFIX="${TW_PREFIX:-$HOME/.local}"
VERSION="${TW_VERSION:-latest}"

say() { printf '%s\n' "$*" >&2; }
die() { say "error: $*"; exit 1; }

case "$(uname -s)" in
  Darwin) os=apple-darwin ;;
  Linux) os=unknown-linux-gnu ;;
  *) die "unsupported OS $(uname -s). On Windows use install.ps1 from the release page." ;;
esac
case "$(uname -m)" in
  arm64|aarch64) arch=aarch64 ;;
  x86_64|amd64) arch=x86_64 ;;
  *) die "unsupported CPU $(uname -m)" ;;
esac
target="$arch-$os"
asset="terminal-windows-$target.tar.gz"

if [ "$VERSION" = latest ]; then
  base="https://github.com/$REPO/releases/latest/download"
else
  base="https://github.com/$REPO/releases/download/$VERSION"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
say "Downloading $asset ..."
curl -fsSL --retry 5 --retry-delay 3 "$base/$asset" -o "$tmp/$asset" || die "download failed: $base/$asset"
curl -fsSL --retry 5 --retry-delay 3 "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" || die "cannot download SHA256SUMS"

expected="$(awk -v f="$asset" '$2 == f || $2 == "*"f {print $1}' "$tmp/SHA256SUMS")"
[ -n "$expected" ] || die "$asset is not listed in SHA256SUMS"
if command -v shasum >/dev/null 2>&1; then
  actual="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
else
  actual="$(sha256sum "$tmp/$asset" | awk '{print $1}')"
fi
[ "$expected" = "$actual" ] || die "SHA-256 mismatch for $asset (expected $expected, got $actual)"
say "SHA-256 verified."

tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$PREFIX/bin"
dest="$PREFIX/bin/terminal-windows"
if [ -e "$dest" ] && ! "$dest" --version 2>/dev/null | grep -q '^terminal-windows '; then
  die "$dest exists and is not terminal-windows; refusing to overwrite it"
fi
install -m 755 "$tmp/terminal-windows" "$dest"
say "Installed $("$dest" --version) to $dest"

case ":$PATH:" in
  *":$PREFIX/bin:"*) ;;
  *) say "Add $PREFIX/bin to your PATH to run 'terminal-windows'." ;;
esac
# QEMU and swtpm run the Windows VM. On macOS with Homebrew they are installed now, so the first
# `terminal-windows` run does not have to wait for it. Set TW_SKIP_DEPS=1 to skip this.
if [ "$os" = apple-darwin ] && [ "${TW_SKIP_DEPS:-0}" != 1 ]; then
  missing=""
  command -v qemu-system-aarch64 >/dev/null 2>&1 || command -v qemu-system-x86_64 >/dev/null 2>&1 || missing=" qemu"
  command -v swtpm >/dev/null 2>&1 || missing="$missing swtpm"
  missing="${missing# }"
  if [ -n "$missing" ]; then
    if command -v brew >/dev/null 2>&1; then
      say "Installing $missing with Homebrew ..."
      # shellcheck disable=SC2086
      brew install $missing || say "warning: 'brew install $missing' failed; 'terminal-windows' will retry it."
    else
      say "Next: install $missing (Homebrew: https://brew.sh, then: brew install $missing)"
    fi
  fi
elif [ "$os" = unknown-linux-gnu ]; then
  say "Linux: install qemu-system and swtpm with your package manager."
fi
say "Then run: terminal-windows"
