#!/bin/sh
set -eu

APP_NAME="codex-hud"
BIN_NAME="codex"
VERSION="${CODEX_HUD_VERSION:-}"
REPO="${CODEX_HUD_REPO:-subisle/codex-hud}"
INSTALL_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
TARGET_BIN="$INSTALL_DIR/$BIN_NAME"

usage() {
  cat <<EOF
Usage:
  ./install.sh [path/to/codex]
  curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | sh

Environment:
  CODEX_HUD_VERSION=<release tag>
  CODEX_HUD_REPO=$REPO
  XDG_BIN_HOME=$INSTALL_DIR
EOF
}

log() {
  printf '%s\n' "$*"
}

err() {
  printf 'error: %s\n' "$*" >&2
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    err "missing required command: $1"
    exit 1
  fi
}

install_binary() {
  source_bin="$1"

  if [ ! -f "$source_bin" ]; then
    err "binary not found: $source_bin"
    exit 1
  fi

  mkdir -p "$INSTALL_DIR"
  cp "$source_bin" "$TARGET_BIN"
  chmod 755 "$TARGET_BIN"

  if [ "$(uname -s)" = "Darwin" ]; then
    if command -v codesign >/dev/null 2>&1; then
      codesign --force --sign - "$TARGET_BIN" >/dev/null
    else
      log "warning: codesign not found; macOS may reject $TARGET_BIN"
    fi
  fi

  print_result
}

print_result() {
  resolved_codex="$(command -v "$BIN_NAME" 2>/dev/null || true)"

  cat <<EOF
installed $APP_NAME wrapper to $TARGET_BIN
put $INSTALL_DIR before the real Codex directory in PATH so this wrapper is selected first
keep the real codex binary later in PATH; the wrapper must still be able to find it
EOF

  if [ "$resolved_codex" != "$TARGET_BIN" ]; then
    cat <<EOF
warning: current shell resolves 'codex' to ${resolved_codex:-<not found>}
run this once before testing:
  export PATH="$INSTALL_DIR:\$PATH"
then verify:
  which -a codex
EOF
  else
    log "current shell resolves 'codex' to the HUD wrapper"
  fi
}

download_and_install() {
  os="$(uname -s)"
  arch="$(uname -m)"
  target=""
  archive=""
  release_path="latest/download"

  case "$os:$arch" in
    Darwin:arm64 | Darwin:aarch64)
      target="aarch64-apple-darwin"
      ;;
    *)
      err "prebuilt release is not available for $os $arch yet"
      exit 1
      ;;
  esac

  require_cmd curl
  require_cmd tar

  if [ -n "$VERSION" ]; then
    archive="$APP_NAME-$VERSION-$target.tar.gz"
    release_path="download/$VERSION"
  else
    latest_tag="$(
      curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
        | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' \
        | head -n 1
    )"

    if [ -z "$latest_tag" ]; then
      err "unable to determine latest release tag"
      exit 1
    fi

    VERSION="$latest_tag"
    archive="$APP_NAME-$VERSION-$target.tar.gz"
  fi

  url="https://github.com/$REPO/releases/$release_path/$archive"
  tmp_dir="$(mktemp -d)"
  trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM

  log "downloading $url"
  curl -fsSL "$url" -o "$tmp_dir/$archive"
  tar -xzf "$tmp_dir/$archive" -C "$tmp_dir"
  if [ -n "$VERSION" ]; then
    install_binary "$tmp_dir/$APP_NAME-$VERSION-$target/$BIN_NAME"
  else
    install_binary "$tmp_dir/$APP_NAME-$target/$BIN_NAME"
  fi
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

if [ "$#" -gt 1 ]; then
  usage >&2
  exit 1
fi

if [ "$#" -eq 1 ]; then
  install_binary "$1"
elif [ -f "./$BIN_NAME" ]; then
  install_binary "./$BIN_NAME"
else
  download_and_install
fi
