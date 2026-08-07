#!/usr/bin/env bash
# Copy built Arch packages into a checked-out mjiang-extras tree and push.
#
# Usage:
#   scripts/publish_arch_to_extras.sh --extras-dir DIR [--source-repo OWNER/NAME] PACKAGE...
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/publish_arch_to_extras.sh --extras-dir DIR [--source-repo OWNER/NAME] PACKAGE...

Runs mjiang-extras/scripts/publish-packages.sh, commits the refreshed pacman
repo database, and pushes. DIR must already be a checkout of mjiang-extras with
push credentials configured for git.
EOF
}

extras_dir=""
source_repo=""
packages=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --extras-dir)
      extras_dir="${2:?missing value for --extras-dir}"
      shift 2
      ;;
    --source-repo)
      source_repo="${2:?missing value for --source-repo}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --)
      shift
      packages+=("$@")
      break
      ;;
    -*)
      echo "error: unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
    *)
      packages+=("$1")
      shift
      ;;
  esac
done

if [[ -z "$extras_dir" || ${#packages[@]} -eq 0 ]]; then
  usage >&2
  exit 2
fi

if [[ ! -d "$extras_dir" ]]; then
  echo "error: extras dir not found: $extras_dir" >&2
  exit 1
fi

extras_dir="$(cd "$extras_dir" && pwd)"
publish_script="$extras_dir/scripts/publish-packages.sh"
if [[ ! -x "$publish_script" && -f "$publish_script" ]]; then
  chmod +x "$publish_script"
fi
if [[ ! -f "$publish_script" ]]; then
  echo "error: missing $publish_script" >&2
  exit 1
fi

resolved_packages=()
for pkg in "${packages[@]}"; do
  if [[ ! -f "$pkg" ]]; then
    echo "error: package not found: $pkg" >&2
    exit 1
  fi
  resolved_packages+=("$(cd "$(dirname -- "$pkg")" && pwd)/$(basename -- "$pkg")")
done

"$publish_script" --repo-dir "$extras_dir" "${resolved_packages[@]}"

cd "$extras_dir"
git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
git add x86_64/
commit_message="chore(arch): update packages"
if [[ -n "$source_repo" ]]; then
  commit_message="chore(arch): update packages from $source_repo"
fi
if git diff --cached --quiet; then
  echo "No Arch package repo changes to commit"
  exit 0
fi
git commit -m "$commit_message"
git push
