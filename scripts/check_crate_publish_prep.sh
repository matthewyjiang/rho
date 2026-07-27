#!/usr/bin/env bash
# Verify workspace crates can be packaged for crates.io.
#
# Path patches cover coordinated same-PR version bumps before dependencies are
# on crates.io. Release publication still validates each crate against the
# registry in dependency order via scripts/publish_workspace_crates.sh.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

echo "Verifying rho-sdk package"
cargo package --locked -p rho-sdk
echo "Verifying rho-sdk publish preparation"
cargo publish --dry-run --locked -p rho-sdk

echo "Verifying rho-agent-tools publish preparation against workspace rho-sdk"
cargo publish --dry-run --locked -p rho-agent-tools \
  --config 'patch.crates-io.rho-sdk.path="crates/rho-sdk"'

echo "Verifying rho-providers publish preparation against workspace deps"
cargo publish --dry-run --locked -p rho-providers \
  --config 'patch.crates-io.rho-sdk.path="crates/rho-sdk"' \
  --config 'patch.crates-io.rho-agent-tools.path="crates/rho-tools"'

echo "Building rho-coding-agent package contents against workspace deps"
cargo package --locked --no-verify -p rho-coding-agent \
  --config 'patch.crates-io.rho-sdk.path="crates/rho-sdk"' \
  --config 'patch.crates-io.rho-providers.path="crates/rho-providers"' \
  --config 'patch.crates-io.rho-agent-tools.path="crates/rho-tools"'

echo "Crate publish preparation checks passed"
