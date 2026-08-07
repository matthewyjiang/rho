#!/usr/bin/env bash
# Check or regenerate the Interactive TUI docs proof plate.
#
# Default mode compares a live matrix-mode PTY capture to the checked-in SVGs.
# Pass --write to overwrite dark (README + site) and light (site) asset paths
# after a layout or chrome change.
#
# Requires a Unix PTY and a debug rho build (RHO_TUI_TEST_MODE=matrix).

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

mode="check"
jobs="${CARGO_BUILD_JOBS:-12}"
bin_path=""
dark_assets=(
  "docs/assets/rho-ui-demo.svg"
  "docs/public/assets/rho-ui-demo.svg"
)
# Light is site-only (HomePage). README keeps the dark plate under docs/assets.
light_assets=(
  "docs/public/assets/rho-ui-demo-light.svg"
)

usage() {
  cat <<'EOF'
Usage: scripts/check_docs_ui_demo.sh [--check|--write] [--bin PATH] [--jobs N]

  --check   Fail when checked-in SVGs drift from a live PTY capture (default).
  --write   Regenerate dark and light docs asset paths from a live PTY capture.
  --bin     Path to a debug rho binary (skips the default rebuild of target/debug/rho).
  --jobs    Cargo job cap (default: CARGO_BUILD_JOBS or 12).
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --check)
      mode="check"
      shift
      ;;
    --write)
      mode="write"
      shift
      ;;
    --bin)
      bin_path="${2:-}"
      shift 2
      ;;
    --jobs)
      jobs="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$(uname -s)" in
  Linux|Darwin) ;;
  *)
    echo "skip: docs TUI proof plate requires a Unix PTY ($(uname -s))"
    exit 0
    ;;
esac

if [[ -z "$bin_path" ]]; then
  # Always rebuild so rust-cache / earlier cargo steps cannot leave a stale
  # target/debug/rho that captures a different screen than the checked-out source.
  echo "==> Build debug rho for matrix mode"
  cargo build -j "$jobs" -p rho-coding-agent --bin rho --locked
  bin_path="$root/target/debug/rho"
fi

if [[ ! -x "$bin_path" && ! -f "$bin_path" ]]; then
  echo "error: rho binary not found: $bin_path" >&2
  exit 1
fi

demo_args=(
  run -j "$jobs" -p rho-tui-pty --bin rho-pty-demo --locked --
  --bin "$bin_path"
)

if [[ "$mode" == "write" ]]; then
  echo "==> Write docs TUI proof plate from live PTY capture"
  write_args=("${demo_args[@]}")
  for asset in "${dark_assets[@]}"; do
    write_args+=(--output "$asset")
  done
  for asset in "${light_assets[@]}"; do
    write_args+=(--light-output "$asset")
  done
  cargo "${write_args[@]}"
  exit 0
fi

echo "==> Check docs TUI proof plate against live PTY capture"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/rho-ui-demo.XXXXXX")"
cleanup() { rm -rf "$tmp"; }
trap cleanup EXIT

live_dark="$tmp/rho-ui-demo.svg"
live_light="$tmp/rho-ui-demo-light.svg"
live_screen="$tmp/screen.txt"

check_args=(
  "${demo_args[@]}"
  --output "$live_dark"
  --light-output "$live_light"
  --screen-text "$live_screen"
)
cargo "${check_args[@]}"

failed=0
compare_asset() {
  local expected="$1"
  local actual="$2"
  if cmp -s "$expected" "$actual"; then
    echo "OK $expected"
    return 0
  fi
  failed=1
  echo "SVG drift at $expected" >&2
  echo "regenerate with:" >&2
  echo "  bash scripts/check_docs_ui_demo.sh --write" >&2
  echo "--- diff: $expected vs live capture ---" >&2
  diff -u "$expected" "$actual" >&2 || true
}

for asset in "${dark_assets[@]}"; do
  compare_asset "$asset" "$live_dark"
done
for asset in "${light_assets[@]}"; do
  compare_asset "$asset" "$live_light"
done

if [[ "$failed" -ne 0 ]]; then
  echo "--- live screen text ---" >&2
  cat "$live_screen" >&2 || true
  echo "error: one or more SVG outputs drifted from the live PTY capture" >&2
  exit 1
fi
