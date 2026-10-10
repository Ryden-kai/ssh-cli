#!/usr/bin/env bash
# One-shot installer for ssh-cli.
# Builds (release), copies the binary to ~/.local/bin/ssh-cli.exe,
# and (on Windows) also drops a copy in ~/bin/ssh-cli.exe if that
# directory is on PATH and the user wants it.
#
# Usage:
#   ./install.sh              # build + install to ~/.local/bin
#   ./install.sh --no-build   # skip cargo build, just copy existing release binary
#   ./install.sh --mirror-bin # also copy to ~/bin/ssh-cli.exe if it shadows ~/.local/bin on PATH

set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARGET_DIR="${PROJECT_DIR}/target/x86_64-pc-windows-gnu/release"
BIN_NAME="ssh-cli.exe"

DO_BUILD=1
MIRROR_BIN=0
for arg in "$@"; do
  case "$arg" in
    --no-build)  DO_BUILD=0 ;;
    --mirror-bin) MIRROR_BIN=1 ;;
    -h|--help)
      sed -n '2,12p' "$0"
      exit 0
      ;;
    *) echo "unknown arg: $arg" >&2; exit 2 ;;
  esac
done

if [ "$DO_BUILD" -eq 1 ]; then
  echo "==> cargo build --release"
  ( cd "$PROJECT_DIR" && cargo build --release )
fi

if [ ! -f "$TARGET_DIR/$BIN_NAME" ]; then
  echo "error: $TARGET_DIR/$BIN_NAME not found; run a build first or pass --no-build=false" >&2
  exit 1
fi

# Resolve to absolute Windows path for cp compatibility.
SRC=$(cygpath -w "$TARGET_DIR/$BIN_NAME" 2>/dev/null || echo "$TARGET_DIR/$BIN_NAME")

install_to() {
  local dest_dir="$1"
  local dest="$dest_dir/$BIN_NAME"
  mkdir -p "$dest_dir"
  cp -f "$TARGET_DIR/$BIN_NAME" "$dest"
  echo "==> installed: $dest"
}

# Primary target: ~/.local/bin/ssh-cli.exe (cross-platform user-local convention).
LOCAL_BIN="$HOME/.local/bin"
install_to "$LOCAL_BIN"

# Optional: also drop into ~/bin if requested AND it appears on PATH
# (some setups put ~/bin ahead of ~/.local/bin so the primary copy is shadowed).
if [ "$MIRROR_BIN" -eq 1 ]; then
  case ":$PATH:" in
    *":$HOME/bin:"*|*":/c/Users/$USER/bin:"*)
      install_to "$HOME/bin"
      ;;
    *)
      echo "note: ~/bin not on PATH, skipping mirror"
      ;;
  esac
fi

echo
echo "==> verify"
type -a ssh-cli 2>/dev/null || which ssh-cli || echo "(ssh-cli not on PATH yet — add ~/.local/bin to PATH)"
"$LOCAL_BIN/$BIN_NAME" --version
