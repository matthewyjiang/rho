#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"
output=${RHO_AUDIT_OUTPUT:-audits/sdk-redaction-current.json}
if [[ "$output" != /* ]]; then
  output="$repo_root/$output"
fi

cargo test -p rho-sdk --test redaction_canary
set +e
python3 scripts/audit_sdk_redaction.py --dynamic-result passed --output "$output"
status=$?
set -e

if [[ $status -ne 0 && ${RHO_AUDIT_ALLOW_FINDINGS:-0} == 1 ]]; then
  echo "redaction findings recorded; release gate remains blocked"
  exit 0
fi
exit "$status"
