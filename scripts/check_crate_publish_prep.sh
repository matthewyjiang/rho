#!/usr/bin/env bash
# Verify workspace crates can be packaged for crates.io.
#
# Path patches cover only internal dependencies whose exact workspace version
# is not yet on crates.io (coordinated same-PR cuts). Dependencies that already
# exist on the registry are verified against crates.io so prep fails when a
# crate imports symbols the published dependency does not export.
#
# Policy and fixture coverage live in scripts/crate_publish_prep.py.
# Release publication still publishes in dependency order via
# scripts/publish_workspace_crates.sh.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

python3 scripts/crate_publish_prep.py
