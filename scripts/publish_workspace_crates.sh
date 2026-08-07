#!/usr/bin/env bash
# Publish workspace crates in dependency order from an exact commit.
#
# crates.io cannot roll back a successful publish. This script fails closed:
# it never continues past the first failed package and never mutates GitHub
# releases. Callers should only publish draft GitHub releases after this
# script succeeds for the same candidate SHA.
#
# Dry-run validation uses scripts/crate_publish_prep.py so path patches apply
# only for internal deps whose exact workspace version is not on crates.io yet.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'EOF'
Usage: scripts/publish_workspace_crates.sh --sha <full-sha> [--sdk] [--tools] [--providers] [--app] [--dry-run]

  --sha <full-sha>  Exact 40-character commit that must be checked out
  --sdk             Publish rho-sdk
  --tools           Publish rho-agent-tools
  --providers       Publish rho-providers
  --app             Publish rho-agent-tools, rho-providers, then rho-coding-agent
  --dry-run         Validate packages without publishing
EOF
}

sha=""
publish_sdk=false
publish_tools=false
publish_providers=false
publish_app=false
dry_run=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --sha)
      sha="${2:-}"
      shift 2
      ;;
    --sdk)
      publish_sdk=true
      shift
      ;;
    --tools)
      publish_tools=true
      shift
      ;;
    --providers)
      publish_providers=true
      shift
      ;;
    --app)
      publish_tools=true
      publish_providers=true
      publish_app=true
      shift
      ;;
    --dry-run)
      dry_run=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$sha" ]]; then
  echo "--sha is required" >&2
  usage >&2
  exit 2
fi

if [[ ! "$sha" =~ ^[0-9a-f]{40}$ ]]; then
  echo "Candidate SHA must be a full lowercase commit SHA" >&2
  exit 1
fi

if [[ "$publish_sdk" != true && "$publish_tools" != true && "$publish_providers" != true && "$publish_app" != true ]]; then
  echo "Select at least one of --sdk, --tools, --providers, or --app" >&2
  exit 2
fi

actual_sha="$(git rev-parse HEAD)"
if [[ "$actual_sha" != "$sha" ]]; then
  echo "Checked out $actual_sha instead of requested $sha" >&2
  exit 1
fi

if [[ -z "${CARGO_REGISTRY_TOKEN:-}" && "$dry_run" != true ]]; then
  echo "CARGO_REGISTRY_TOKEN is required for publication" >&2
  exit 1
fi

publish_flags=(--locked)
if [[ "$dry_run" == true ]]; then
  publish_flags+=(--dry-run)
else
  publish_flags+=(--token "$CARGO_REGISTRY_TOKEN")
fi

metadata="$(cargo metadata --format-version 1 --no-deps)"
crate_version() {
  local name="$1"
  python3 -c 'import json,sys; name=sys.argv[1]; data=json.load(sys.stdin); print(next(p["version"] for p in data["packages"] if p["name"]==name))' "$name" <<<"$metadata"
}

sdk_version="$(crate_version rho-sdk)"
tools_version="$(crate_version rho-agent-tools)"
providers_version="$(crate_version rho-providers)"
app_version="$(crate_version rho-coding-agent)"

crates_io_curl=(
  curl
  -fsS
  --user-agent "rho-release-publisher/1.0 (https://github.com/matthewyjiang/rho)"
)

wait_for_crate() {
  local name="$1"
  local version="$2"
  local attempt
  for attempt in $(seq 1 36); do
    if "${crates_io_curl[@]}" "https://crates.io/api/v1/crates/${name}/${version}" >/dev/null; then
      echo "${name}@${version} is visible on crates.io"
      return 0
    fi
    echo "Waiting for ${name}@${version} to index (attempt ${attempt}/36)"
    sleep 10
  done
  echo "${name}@${version} did not become visible on crates.io" >&2
  return 1
}

crate_already_published() {
  local name="$1"
  local version="$2"
  "${crates_io_curl[@]}" "https://crates.io/api/v1/crates/${name}/${version}" >/dev/null 2>&1
}

if [[ "$publish_sdk" == true ]]; then
  echo "Validating rho-sdk ${sdk_version} at ${sha}"
  cargo publish --dry-run --locked -p rho-sdk
  if [[ "$dry_run" == true ]]; then
    echo "Dry-run complete for rho-sdk"
  elif crate_already_published rho-sdk "$sdk_version"; then
    echo "rho-sdk@${sdk_version} already published; reusing existing crate"
  else
    echo "Publishing rho-sdk ${sdk_version}"
    cargo publish "${publish_flags[@]}" -p rho-sdk
    wait_for_crate rho-sdk "$sdk_version"
  fi
fi

publish_crate() {
  local name="$1"
  local version="$2"
  shift 2
  local validation_flags=("$@")

  # Exact crate versions are immutable on crates.io. Skip packaging work when
  # the version is already public so a later app/providers release cannot fail
  # by re-verifying drifted workspace sources under an already-shipped version.
  if crate_already_published "$name" "$version"; then
    if [[ "$dry_run" == true ]]; then
      echo "${name}@${version} already published; dry-run reuses existing crate"
    else
      echo "${name}@${version} already published; reusing existing crate"
    fi
    return 0
  fi

  echo "Validating ${name} ${version} at ${sha}"
  cargo publish --dry-run --locked -p "$name" "${validation_flags[@]}"
  if [[ "$dry_run" == true ]]; then
    echo "Dry-run complete for ${name}"
    return 0
  fi

  echo "Publishing ${name} ${version}"
  cargo publish "${publish_flags[@]}" -p "$name"
  wait_for_crate "$name" "$version"
}

load_dry_run_validation_flags() {
  # Populate dry_run_validation_flags with selective cargo --config tokens.
  local package="$1"
  local out
  out="$(
    python3 "$root/scripts/crate_publish_prep.py" --print-cargo-config "$package"
  )" || return 1
  dry_run_validation_flags=()
  if [[ -z "$out" ]]; then
    return 0
  fi
  while IFS= read -r line; do
    [[ -n "$line" ]] || continue
    dry_run_validation_flags+=("$line")
  done <<<"$out"
}

if [[ "$publish_tools" == true ]]; then
  tools_validation_flags=()
  if [[ "$dry_run" == true ]]; then
    load_dry_run_validation_flags rho-agent-tools
    tools_validation_flags=("${dry_run_validation_flags[@]}")
  elif [[ "$publish_sdk" != true ]]; then
    wait_for_crate rho-sdk "$sdk_version"
  fi
  publish_crate rho-agent-tools "$tools_version" "${tools_validation_flags[@]}"
fi

if [[ "$publish_providers" == true ]]; then
  providers_validation_flags=()
  if [[ "$dry_run" == true ]]; then
    load_dry_run_validation_flags rho-providers
    providers_validation_flags=("${dry_run_validation_flags[@]}")
  else
    if [[ "$publish_sdk" != true ]]; then
      wait_for_crate rho-sdk "$sdk_version"
    fi
    if [[ "$publish_tools" != true ]]; then
      wait_for_crate rho-agent-tools "$tools_version"
    fi
  fi
  publish_crate rho-providers "$providers_version" "${providers_validation_flags[@]}"
fi

if [[ "$publish_app" == true ]]; then
  app_validation_flags=()
  if [[ "$dry_run" == true ]]; then
    load_dry_run_validation_flags rho-coding-agent
    app_validation_flags=("${dry_run_validation_flags[@]}")
  elif [[ "$publish_sdk" != true ]]; then
    wait_for_crate rho-sdk "$sdk_version"
  fi
  publish_crate rho-coding-agent "$app_version" "${app_validation_flags[@]}"
fi

echo "Workspace crate publication finished for ${sha}"
