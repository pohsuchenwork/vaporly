#!/usr/bin/env bash
# Vaporly installer. Downloads the latest release and installs it.
#   macOS: installs /Applications/Vaporly.app from the release tarball.
#   Linux: installs the AppImage to ~/.local/bin.
#   Windows: use install.ps1 instead.
#
# Usage:
#   curl -fsSL https://github.com/pohsuchenwork/vaporly/releases/latest/download/install.sh | bash
#
# Note: while the repository is private these download URLs need a GitHub login.
# They become anonymous once the repo is public. Until then, download the app
# from the Releases page by hand.
set -euo pipefail

REPO="pohsuchenwork/vaporly"
APP_NAME="Vaporly"

log() { printf '\n>> %s\n' "$1"; }
die() { printf '\nError: %s\n' "$1" >&2; exit 1; }

OS="$(uname -s)"
ARCH_RAW="$(uname -m)"

install_macos() {
  case "$ARCH_RAW" in
    arm64 | aarch64) ARCH="aarch64" ;;
    x86_64) ARCH="x86_64" ;;
    *) die "unsupported macOS architecture: $ARCH_RAW" ;;
  esac

  local url tmp tarball
  url="https://github.com/${REPO}/releases/latest/download/${APP_NAME}_${ARCH}.app.tar.gz"
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  tarball="${tmp}/${APP_NAME}.app.tar.gz"

  log "Downloading ${APP_NAME} for ${ARCH}..."
  curl -fSL --progress-bar "$url" -o "$tarball" ||
    die "download failed. If the repo is private, grab the app from the Releases page instead."

  log "Closing ${APP_NAME} if it is running..."
  osascript -e "quit app \"${APP_NAME}\"" >/dev/null 2>&1 || true
  sleep 1

  log "Installing to /Applications..."
  rm -rf "/Applications/${APP_NAME}.app"
  tar -xzf "$tarball" -C "/Applications"
  [ -d "/Applications/${APP_NAME}.app" ] || die "install did not produce /Applications/${APP_NAME}.app"

  # A curl download carries no quarantine flag, so Gatekeeper opens the app
  # without the "unidentified developer" block. Clear any stray flag anyway.
  xattr -dr com.apple.quarantine "/Applications/${APP_NAME}.app" 2>/dev/null || true

  log "Done. Launching ${APP_NAME}..."
  open "/Applications/${APP_NAME}.app" || true

  cat <<'NEXT'

Almost there. On first launch, grant these in System Settings > Privacy and Security:
  1. Microphone        so Vaporly can hear you
  2. Accessibility     so Vaporly can paste into other apps
  3. Input Monitoring  so the global hotkey works

Then hold the Fn key, say a sentence, and release. Your text lands where the cursor is.
NEXT
}

install_linux() {
  # Match the AppImage arch tag Tauri publishes: x86_64 -> amd64, arm64 -> aarch64.
  case "$ARCH_RAW" in
    x86_64) ARCH="amd64" ;;
    aarch64 | arm64) ARCH="aarch64" ;;
    *) die "unsupported Linux architecture: $ARCH_RAW" ;;
  esac

  command -v curl >/dev/null || die "curl is required"
  local api asset dest
  api="https://api.github.com/repos/${REPO}/releases/latest"
  log "Finding the latest AppImage for ${ARCH}..."
  asset="$(curl -fsSL "$api" |
    grep -o '"browser_download_url": *"[^"]*\.AppImage"' |
    cut -d'"' -f4 | grep -i "$ARCH" | head -1)"
  [ -n "$asset" ] || die "no AppImage found for ${ARCH} on the latest release."

  dest="${HOME}/.local/bin/${APP_NAME}.AppImage"
  mkdir -p "${HOME}/.local/bin"
  log "Downloading to ${dest}..."
  curl -fSL --progress-bar "$asset" -o "$dest"
  chmod +x "$dest"

  cat <<NEXT

Installed to ${dest}
Run it with:  ${dest}
(.deb and .rpm packages are also on the release page if you prefer those.)
NEXT
}

case "$OS" in
  Darwin) install_macos ;;
  Linux) install_linux ;;
  *) die "unsupported OS: $OS (on Windows, use install.ps1)" ;;
esac
