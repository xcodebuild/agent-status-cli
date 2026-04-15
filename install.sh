#!/usr/bin/env sh
set -eu

REPO="${AGENT_STATUS_REPO:-xcodebuild/agent-status-cli}"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${1:-${AGENT_STATUS_VERSION:-latest}}"

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "install.sh: missing required command: $1" >&2
    exit 1
  fi
}

fail() {
  echo "install.sh: $*" >&2
  exit 1
}

resolve_target() {
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Darwin) platform="apple-darwin" ;;
    Linux) platform="unknown-linux-gnu" ;;
    *) fail "unsupported operating system: $os" ;;
  esac

  case "$arch" in
    x86_64|amd64) cpu="x86_64" ;;
    arm64|aarch64) cpu="aarch64" ;;
    *) fail "unsupported architecture: $arch" ;;
  esac

  target="${cpu}-${platform}"

  case "$target" in
    x86_64-unknown-linux-gnu|x86_64-apple-darwin|aarch64-apple-darwin)
      printf '%s' "$target"
      ;;
    *)
      fail "no release asset is published for $target"
      ;;
  esac
}

resolve_version() {
  if [ "$VERSION" != "latest" ]; then
    printf '%s' "$VERSION"
    return
  fi

  need_cmd curl
  latest_tag="$(
    curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
      | grep -m 1 '"tag_name"' \
      | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/'
  )"

  [ -n "$latest_tag" ] || fail "could not resolve the latest release tag"
  printf '%s' "$latest_tag"
}

extract_zip() {
  archive="$1"
  destination="$2"

  if command -v unzip >/dev/null 2>&1; then
    unzip -q "$archive" -d "$destination"
    return
  fi

  if command -v bsdtar >/dev/null 2>&1; then
    bsdtar -xf "$archive" -C "$destination"
    return
  fi

  fail "need unzip or bsdtar to extract release assets"
}

install_bin() {
  source_path="$1"
  target_path="$2"
  install -m 0755 "$source_path" "$target_path"
}

main() {
  need_cmd curl
  need_cmd install
  target="$(resolve_target)"
  version="$(resolve_version)"
  asset="agent-status-cli-${target}.zip"
  download_url="https://github.com/$REPO/releases/download/$version/$asset"

  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT INT TERM HUP

  echo "Downloading $download_url"
  curl -fL "$download_url" -o "$tmpdir/$asset"

  extract_zip "$tmpdir/$asset" "$tmpdir/unpack"
  mkdir -p "$INSTALL_DIR"

  for bin_name in agent-status-cli asc-codex asc-claude; do
    bin_path="$(find "$tmpdir/unpack" -type f -name "$bin_name" -print -quit)"
    [ -n "$bin_path" ] || fail "missing binary in archive: $bin_name"
    install_bin "$bin_path" "$INSTALL_DIR/$bin_name"
  done

  echo "Installed agent-status-cli, asc-codex, and asc-claude to $INSTALL_DIR"
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
      echo "Add $INSTALL_DIR to PATH to use the commands directly."
      ;;
  esac
}

main "$@"
