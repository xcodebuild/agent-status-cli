#!/usr/bin/env sh
set -eu

INSTALL_DIR="${INSTALL_DIR:-}"
CARGO_BIN="${CARGO_BIN:-cargo}"
TARGET_TRIPLE="${TARGET_TRIPLE:-}"
BUILD_PROFILE="${BUILD_PROFILE:-release}"

usage() {
  cat <<'EOF'
Usage:
  ./install-dev.sh [--install-dir DIR] [--target TARGET] [--profile PROFILE]

Environment overrides:
  INSTALL_DIR     Install destination directory
  TARGET_TRIPLE   Cargo target triple
  BUILD_PROFILE   Cargo profile name (default: release)
  CARGO_BIN       Cargo executable to run (default: cargo)
EOF
}

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "install-dev.sh: missing required command: $1" >&2
    exit 1
  fi
}

fail() {
  echo "install-dev.sh: $*" >&2
  exit 1
}

path_contains_dir() {
  dir="$1"

  case ":$PATH:" in
    *":$dir:"*) return 0 ;;
    *) return 1 ;;
  esac
}

pick_install_dir() {
  if [ -n "$INSTALL_DIR" ]; then
    printf '%s' "$INSTALL_DIR"
    return
  fi

  for dir in /opt/homebrew/bin /usr/local/bin "$HOME/.local/bin" "$HOME/bin"; do
    if path_contains_dir "$dir" && [ -d "$dir" ] && [ -w "$dir" ]; then
      printf '%s' "$dir"
      return
    fi
  done

  old_ifs="${IFS}"
  IFS=:
  set -- $PATH
  IFS="${old_ifs}"

  for dir in "$@"; do
    [ -n "$dir" ] || continue
    [ -d "$dir" ] || continue
    [ -w "$dir" ] || continue
    printf '%s' "$dir"
    return
  done

  printf '%s' "$HOME/.local/bin"
}

repo_root() {
  CDPATH= cd -- "$(dirname -- "$0")" && pwd
}

profile_dir() {
  case "$BUILD_PROFILE" in
    release|debug)
      printf '%s' "$BUILD_PROFILE"
      ;;
    *)
      printf '%s' "$BUILD_PROFILE"
      ;;
  esac
}

build_bins() {
  root="$1"

  set -- "$CARGO_BIN" build --locked --bin agent-status-cli --bin asc-codex --bin asc-claude --bin asc-opencode

  if [ "$BUILD_PROFILE" = "release" ]; then
    set -- "$@" --release
  elif [ "$BUILD_PROFILE" != "debug" ]; then
    set -- "$@" --profile "$BUILD_PROFILE"
  fi

  if [ -n "$TARGET_TRIPLE" ]; then
    set -- "$@" --target "$TARGET_TRIPLE"
  fi

  (
    cd "$root"
    "$@"
  )
}

parse_args() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --install-dir)
        [ "$#" -ge 2 ] || fail "missing value for --install-dir"
        INSTALL_DIR="$2"
        shift 2
        ;;
      --target)
        [ "$#" -ge 2 ] || fail "missing value for --target"
        TARGET_TRIPLE="$2"
        shift 2
        ;;
      --profile)
        [ "$#" -ge 2 ] || fail "missing value for --profile"
        BUILD_PROFILE="$2"
        shift 2
        ;;
      --help|-h)
        usage
        exit 0
        ;;
      *)
        fail "unknown argument: $1"
        ;;
    esac
  done
}

artifact_dir() {
  root="$1"
  profile="$(profile_dir)"

  if [ -n "$TARGET_TRIPLE" ]; then
    printf '%s/target/%s/%s' "$root" "$TARGET_TRIPLE" "$profile"
  else
    printf '%s/target/%s' "$root" "$profile"
  fi
}

install_bin() {
  source_path="$1"
  target_path="$2"
  install -m 0755 "$source_path" "$target_path"
}

main() {
  parse_args "$@"
  need_cmd "$CARGO_BIN"
  need_cmd install

  root="$(repo_root)"
  install_dir="$(pick_install_dir)"

  build_bins "$root"

  build_dir="$(artifact_dir "$root")"
  mkdir -p "$install_dir"

  for bin_name in agent-status-cli asc-codex asc-claude asc-opencode; do
    bin_path="$build_dir/$bin_name"
    [ -f "$bin_path" ] || fail "missing built binary: $bin_path"
    install_bin "$bin_path" "$install_dir/$bin_name"
  done

  echo "Built profile '$BUILD_PROFILE' from $root"
  echo "Installed agent-status-cli, asc-codex, asc-claude, and asc-opencode to $install_dir"
  case ":$PATH:" in
    *":$install_dir:"*) ;;
    *)
      echo "Add $install_dir to PATH to use the commands directly."
      ;;
  esac
}

main "$@"
