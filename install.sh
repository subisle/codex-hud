#!/bin/sh
set -eu

BIN_NAME="codex"
SOURCE_BIN="${1:-target/release/codex}"
INSTALL_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
TARGET_BIN="$INSTALL_DIR/$BIN_NAME"

mkdir -p "$INSTALL_DIR"
cp "$SOURCE_BIN" "$TARGET_BIN"
chmod 755 "$TARGET_BIN"

RESOLVED_CODEX="$(command -v "$BIN_NAME" 2>/dev/null || true)"

cat <<EOF
installed Codex HUD wrapper to $TARGET_BIN
put $INSTALL_DIR before the real Codex directory in PATH so this wrapper is selected first
keep the real codex binary later in PATH; the wrapper must still be able to find it
EOF

if [ "$RESOLVED_CODEX" != "$TARGET_BIN" ]; then
  cat <<EOF
warning: current shell resolves 'codex' to ${RESOLVED_CODEX:-<not found>}
run this once before testing:
  export PATH="$INSTALL_DIR:\$PATH"
then verify:
  which -a codex
EOF
else
  printf "current shell resolves 'codex' to the HUD wrapper\n"
fi
