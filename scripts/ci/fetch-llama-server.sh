#!/usr/bin/env bash
# Stage the bundled llama.cpp engine payload for a given Rust target triple.
#
#   scripts/ci/fetch-llama-server.sh <rust-target-triple>
#   scripts/ci/fetch-llama-server.sh aarch64-apple-darwin
#
# Downloads the PINNED official llama.cpp release asset for the target,
# verifies its SHA256, and flattens the minimal server payload (llama-server +
# its shared libraries + LICENSE) into src-tauri/resources/llama/, where the
# existing `resources/**/*` bundle glob picks it up on every OS. The app
# installs this payload into app-data on first run and spawns it from there
# (see src-tauri/src/managers/llm_engine.rs).
#
# Missing official asset for a target => warn and exit 0: the build proceeds
# without a payload and the engine degrades to the guided "NotInstalled" state
# (Ollama remains selectable), never a broken build.
#
# Upgrading the pin: bump LLAMA_TAG, refresh every SHA256 below from the
# release assets, and let the llm-engine-smoke CI job gate the change.
set -euo pipefail

LLAMA_TAG="b9912"

# sha256 per asset, computed 2026-07-08 from the official release downloads.
sha_for() {
  case "$1" in
    llama-${LLAMA_TAG}-bin-macos-arm64.tar.gz)  echo "19c24fb4e859eeb3063b3aceaef40d789f7a57b4274e815600f38d8a581724e3" ;;
    llama-${LLAMA_TAG}-bin-macos-x64.tar.gz)    echo "f150aab73721a873518be78d481200776ab8d509c768a7fff9c37878cdf4fb0f" ;;
    llama-${LLAMA_TAG}-bin-ubuntu-x64.tar.gz)   echo "9cc442e9e66d70ee780604cc241917fdd419a24f7af1d61697f8aa53f6eec7dd" ;;
    llama-${LLAMA_TAG}-bin-ubuntu-arm64.tar.gz) echo "7ca306dd5307d9fb75889594cc8b6c1a2db5b2aabe326dc8b01eac362dc84222" ;;
    llama-${LLAMA_TAG}-bin-win-cpu-x64.zip)     echo "82518f23efc049a183343287ac8e1b9d6d4c9586a51520b2604a2b13e7fab768" ;;
    llama-${LLAMA_TAG}-bin-win-cpu-arm64.zip)   echo "5c2e39b8ab8c12b54beec017ac1c45d2542fddaa7ff60e7185a069736f2c3558" ;;
    *) echo "" ;;
  esac
}

# Windows deliberately uses the CPU build: runtime ISA dispatch (per-arch
# ggml-cpu-*.dll) with zero GPU-driver support burden. macOS arm64 ships Metal
# in the standard build; the engine manager decides -ngl at spawn time.
asset_for_triple() {
  case "$1" in
    aarch64-apple-darwin)       echo "llama-${LLAMA_TAG}-bin-macos-arm64.tar.gz" ;;
    x86_64-apple-darwin)        echo "llama-${LLAMA_TAG}-bin-macos-x64.tar.gz" ;;
    x86_64-unknown-linux-gnu)   echo "llama-${LLAMA_TAG}-bin-ubuntu-x64.tar.gz" ;;
    aarch64-unknown-linux-gnu)  echo "llama-${LLAMA_TAG}-bin-ubuntu-arm64.tar.gz" ;;
    x86_64-pc-windows-msvc)     echo "llama-${LLAMA_TAG}-bin-win-cpu-x64.zip" ;;
    aarch64-pc-windows-msvc)    echo "llama-${LLAMA_TAG}-bin-win-cpu-arm64.zip" ;;
    *) echo "" ;;
  esac
}

TRIPLE="${1:-}"
if [ -z "$TRIPLE" ]; then
  TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
  echo "No triple given; using host: $TRIPLE"
fi

ASSET="$(asset_for_triple "$TRIPLE")"
if [ -z "$ASSET" ]; then
  echo "WARN: no llama.cpp release asset mapped for target '$TRIPLE', skipping engine payload." >&2
  exit 0
fi

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DEST="$REPO_ROOT/src-tauri/resources/llama"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

URL="https://github.com/ggml-org/llama.cpp/releases/download/${LLAMA_TAG}/${ASSET}"
echo "Fetching $URL"
curl -fsSL --retry 3 -o "$WORK/$ASSET" "$URL"

WANT_SHA="$(sha_for "$ASSET")"
GOT_SHA="$( (command -v sha256sum >/dev/null && sha256sum "$WORK/$ASSET" || shasum -a 256 "$WORK/$ASSET") | awk '{print $1}')"
if [ "$GOT_SHA" != "$WANT_SHA" ]; then
  echo "ERROR: SHA256 mismatch for $ASSET" >&2
  echo "  want: $WANT_SHA" >&2
  echo "  got:  $GOT_SHA" >&2
  exit 1
fi
echo "SHA256 verified."

EXTRACT="$WORK/extract"
mkdir -p "$EXTRACT"
case "$ASSET" in
  *.tar.gz) tar xzf "$WORK/$ASSET" -C "$EXTRACT" ;;
  *.zip)    unzip -q "$WORK/$ASSET" -d "$EXTRACT" ;;
esac

# Normalize: find llama-server wherever the archive put it (top level dir on
# macOS/Linux, flat root on Windows; layouts have drifted across tags).
SERVER="$(find "$EXTRACT" -type f \( -name 'llama-server' -o -name 'llama-server.exe' \) | head -1)"
if [ -z "$SERVER" ]; then
  echo "ERROR: llama-server not found inside $ASSET (layout drift?), refusing to stage a broken payload." >&2
  exit 1
fi
SRC_DIR="$(dirname "$SERVER")"

rm -rf "$DEST"
mkdir -p "$DEST"
cp "$SERVER" "$DEST/"
# Shared libraries only, the other 20+ example binaries stay behind. Include
# symlinks (-type l) and copy them as symlinks (-P): the dylib/so soname chain
# (libfoo.0.dylib -> libfoo.0.0.N.dylib) is what the binary actually loads;
# dropping the links produces a payload that fails dyld at spawn.
find "$SRC_DIR" -maxdepth 1 \( -type f -o -type l \) \
  \( -name '*.dylib' -o -name '*.so' -o -name '*.so.*' -o -name '*.dll' \) \
  -exec cp -P {} "$DEST/" \;
[ -f "$SRC_DIR/LICENSE" ] && cp "$SRC_DIR/LICENSE" "$DEST/LICENSE.llama.cpp"
chmod +x "$DEST"/llama-server* 2>/dev/null || true
printf '%s\n' "$LLAMA_TAG" > "$DEST/engine-version.txt"

# rpath sanity: the server must resolve its libs beside itself.
case "$TRIPLE" in
  *apple-darwin*)
    if command -v otool >/dev/null; then
      if ! otool -l "$DEST/llama-server" | grep -q '@loader_path'; then
        echo "Patching missing @loader_path rpath"
        install_name_tool -add_rpath @loader_path "$DEST/llama-server"
      fi
    fi
    ;;
  *linux*)
    if command -v readelf >/dev/null; then
      if ! readelf -d "$DEST/llama-server" | grep -qE '\$ORIGIN'; then
        if command -v patchelf >/dev/null; then
          echo "Patching missing \$ORIGIN RUNPATH"
          patchelf --set-rpath '$ORIGIN' "$DEST/llama-server"
        else
          echo "WARN: no \$ORIGIN RUNPATH and patchelf unavailable, server may not find its libs." >&2
        fi
      fi
    fi
    ;;
esac

COUNT="$(find "$DEST" -type f | wc -l | tr -d ' ')"
SIZE="$(du -sh "$DEST" | cut -f1)"
echo "Staged llama.cpp ${LLAMA_TAG} for ${TRIPLE}: ${COUNT} files, ${SIZE} -> ${DEST}"
