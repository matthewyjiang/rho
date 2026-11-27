#!/usr/bin/env bash
# Publish built Arch or Fedora packages into a checked-out mjiang-extras tree
# and push to its main branch.
#
# Usage:
#   scripts/publish_to_extras.sh --kind arch|fedora --extras-dir DIR [--source-repo OWNER/NAME] PACKAGE...
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/publish_to_extras.sh --kind arch|fedora --extras-dir DIR [--source-repo OWNER/NAME] PACKAGE...

arch:   runs mjiang-extras/scripts/publish-packages.sh, commits the refreshed
        pacman repo database, and pushes, rebasing over concurrent pushes.
fedora: hands the RPMs to mjiang-extras/scripts/push-rpms.sh, which owns the
        publish, commit, and push retry loop. It hard-resets DIR to origin/main
        on every attempt, so local changes in DIR are discarded.

DIR must already be a checkout of mjiang-extras with push credentials
configured for git. PACKAGE files must live outside DIR.
EOF
}

kind=""
extras_dir=""
source_repo=""
packages=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --kind)
      kind="${2:?missing value for --kind}"
      shift 2
      ;;
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

if [[ -z "$kind" || -z "$extras_dir" || ${#packages[@]} -eq 0 ]]; then
  usage >&2
  exit 2
fi

case "$kind" in
  arch) publish_script="scripts/publish-packages.sh" ;;
  fedora) publish_script="scripts/push-rpms.sh" ;;
  *)
    echo "error: --kind must be arch or fedora, got: $kind" >&2
    exit 2
    ;;
esac

if [[ ! -d "$extras_dir" ]]; then
  echo "error: extras dir not found: $extras_dir" >&2
  exit 1
fi

extras_dir="$(cd "$extras_dir" && pwd)"
if [[ ! -f "$extras_dir/$publish_script" ]]; then
  echo "error: missing $extras_dir/$publish_script" >&2
  exit 1
fi

resolved_packages=()
for pkg in "${packages[@]}"; do
  if [[ ! -f "$pkg" ]]; then
    echo "error: package not found: $pkg" >&2
    exit 1
  fi
  resolved="$(cd "$(dirname -- "$pkg")" && pwd)/$(basename -- "$pkg")"
  # push-rpms.sh hard-resets the checkout, which would delete a package
  # inside it before publish-rpms.sh copies it.
  if [[ "$resolved" == "$extras_dir"/* ]]; then
    echo "error: package must live outside the extras checkout: $pkg" >&2
    exit 1
  fi
  resolved_packages+=("$resolved")
done

commit_message="chore($kind): update packages"
if [[ -n "$source_repo" ]]; then
  commit_message="chore($kind): update packages from $source_repo"
fi

cd "$extras_dir"
git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"

if [[ "$kind" == "fedora" ]]; then
  # Every Fedora writer rewrites fedora/x86_64/repodata, so a rebase would
  # conflict; push-rpms.sh republishes onto a fresh origin/main instead.
  exec bash "$publish_script" --repo-dir . --message "$commit_message" "${resolved_packages[@]}"
fi

bash "$publish_script" --repo-dir . "${resolved_packages[@]}"
git add x86_64/
if git diff --cached --quiet; then
  echo "No Arch package repo changes to commit"
  exit 0
fi
git commit -m "$commit_message"
# The Fedora publish job pushes to the same main. Its commits touch only
# fedora/, so rebasing over them is clean; a real conflict in x86_64/ fails
# loudly. Five attempts with linear backoff, matching mjiang-extras' own
# writers, which each finish in seconds.
for attempt in 1 2 3 4 5; do
  git pull --rebase origin main || { git rebase --abort; exit 1; }
  if git push origin HEAD:main; then
    exit 0
  fi
  echo "Push attempt $attempt lost a race; retrying"
  sleep $((attempt * 5))
done
echo "error: could not push after 5 attempts" >&2
exit 1
