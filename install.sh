#!/bin/sh
set -eu

BIN_NAME="codex"
SOURCE_BIN="${1:-target/release/codex}"
INSTALL_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
TARGET_BIN="$INSTALL_DIR/$BIN_NAME"

mkdir -p "$INSTALL_DIR"
cp "$SOURCE_BIN" "$TARGET_BIN"
chmod 755 "$TARGET_BIN"

cat <<EOF
installed Codex HUD wrapper to $TARGET_BIN
put $INSTALL_DIR before the real Codex directory in PATH so this wrapper is selected first
keep the real codex binary later in PATH; the wrapper must still be able to find it
EOF
