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

args=(
  run -j "$jobs" -p rho-tui-pty --bin rho-pty-demo --locked --
  --bin "$bin_path"
)

if [[ "$mode" == "check" ]]; then
  args+=(--check)
  echo "==> Check docs TUI proof plate against live PTY capture"
else
  echo "==> Write docs TUI proof plate from live PTY capture"
fi

for asset in "${dark_assets[@]}"; do
  args+=(--output "$asset")
done
for asset in "${light_assets[@]}"; do
  args+=(--light-output "$asset")
done

cargo "${args[@]}"
