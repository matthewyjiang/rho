#!/usr/bin/env sh
set -eu

repo_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
fixtures="$repo_root/fixtures/install"
archive="rho-aarch64-apple-darwin.tar.gz"
checksum="$archive.sha256"

function_file="$(mktemp)"
trap 'rm -f "$function_file"' EXIT INT TERM
sed '/^# Installer entry point\./,$d' "$repo_root/scripts/install.sh" > "$function_file"
# shellcheck source=install.sh
. "$function_file"

assert_eq() {
  expected="$1"
  actual="$2"
  description="$3"
  if [ "$actual" != "$expected" ]; then
    echo "error: $description: expected '$expected', got '$actual'" >&2
    exit 1
  fi
}

# GitHub's REST API returns compact JSON, so each fixture must remain one line.
for fixture in "$fixtures"/*.json; do
  lines="$(wc -l < "$fixture" | tr -d ' ')"
  assert_eq 1 "$lines" "$fixture should contain compact single-line JSON"
done

if ! release_has_assets "$archive" "$checksum" < "$fixtures/release-complete.json"; then
  echo "error: failed to find both assets in a compact release response" >&2
  exit 1
fi

fixture_mode=complete
github_api() {
  case "$1:$fixture_mode" in
    */releases/latest:complete)
      cat "$fixtures/release-complete.json"
      ;;
    */releases/latest:fallback)
      cat "$fixtures/release-missing-assets.json"
      ;;
    *'/releases?per_page=100':fallback)
      cat "$fixtures/releases.json"
      ;;
    */releases/tags/*:pinned)
      cat "$fixtures/release-complete.json"
      ;;
    *)
      echo "error: unexpected test API request: $1 ($fixture_mode)" >&2
      return 1
      ;;
  esac
}

VERSION=latest
actual="$(release_tag "$archive")"
assert_eq rho-coding-agent-v1.5.0 "$actual" "latest release selection"

fixture_mode=fallback
actual="$(release_tag "$archive")"
assert_eq rho-coding-agent-v1.4.1 "$actual" "fallback release selection"

fixture_mode=pinned
VERSION=1.5.0
actual="$(release_tag "$archive")"
assert_eq rho-coding-agent-v1.5.0 "$actual" "pinned release selection"

printf '%s\n' "install parser tests passed"
