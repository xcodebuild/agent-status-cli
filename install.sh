#!/usr/bin/env sh
set -eu

REPO="${AGENT_STATUS_REPO:-xcodebuild/agent-status-cli}"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${1:-${AGENT_STATUS_VERSION:-latest}}"
RELEASE_BASES="${AGENT_STATUS_RELEASE_BASES:-https://gh-proxy.com/https://github.com/$REPO}"

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

download_to() {
  url="$1"
  destination="$2"

  if curl -fL --connect-timeout 10 --retry 2 --retry-delay 1 "$url" -o "$destination"; then
    return 0
  fi

  return 1
}

download_first_available() {
  output="$1"
  shift

  for url in "$@"; do
    echo "Trying $url" >&2
    if download_to "$url" "$output"; then
      printf '%s' "$url"
      return 0
    fi
  done

  return 1
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

resolve_latest_version() {
  need_cmd curl

  for base in $RELEASE_BASES; do
    latest_url="$base/releases/latest"
    echo "Resolving latest release from $latest_url" >&2
    latest_tag="$(
      curl -fsSL -o /dev/null -w '%{url_effective}' -L --connect-timeout 10 "$latest_url" \
        | sed -n 's#.*/tag/\([^/?#]*\).*#\1#p'
    )"
    if [ -n "$latest_tag" ]; then
      printf '%s' "$latest_tag"
      return
    fi
  done

  fail "could not resolve the latest release tag"
}

download_release_asset() {
  output="$1"
  version="$2"
  asset="$3"

  if [ "$version" = "latest" ]; then
    echo "Attempting direct latest asset download for $asset" >&2
    set -- $(for base in $RELEASE_BASES; do printf '%s/releases/latest/download/%s\n' "$base" "$asset"; done)
    if download_url="$(download_first_available "$output" "$@")"; then
      printf '%s' "$download_url"
      return 0
    fi

    echo "Direct latest release download failed; trying explicit latest tag asset" >&2
    set -- $(for base in $RELEASE_BASES; do printf '%s/releases/download/latest/%s\n' "$base" "$asset"; done)
    if download_url="$(download_first_available "$output" "$@")"; then
      printf '%s' "$download_url"
      return 0
    fi

    echo "Direct latest asset download failed; resolving the latest release tag" >&2
    version="$(resolve_latest_version)"
  fi

  set -- $(for base in $RELEASE_BASES; do printf '%s/releases/download/%s/%s\n' "$base" "$version" "$asset"; done)
  download_url="$(download_first_available "$output" "$@")" || return 1
  printf '%s' "$download_url"
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
  version="$VERSION"
  asset="agent-status-cli-${target}.zip"

  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT INT TERM HUP

  download_url="$(download_release_asset "$tmpdir/$asset" "$version" "$asset")" \
    || fail "could not download release asset: $asset"
  echo "Downloaded from $download_url"

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
