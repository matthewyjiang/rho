#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

output=${RHO_BENCH_OUTPUT:-benchmarks/results/sdk-release-current.json}
if [[ "$output" != /* ]]; then
  output="$repo_root/$output"
fi
samples=${RHO_BENCH_SAMPLES:-20}
mkdir -p "$(dirname "$output")"

export RHO_BENCH_OUTPUT="$output"
export RHO_BENCH_SAMPLES="$samples"
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-1}

cargo bench -p rho-sdk --bench release_benchmarks
python3 - "$output" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
evidence = json.loads(path.read_text())
failed = [name for name, passed in evidence["budget_checks"].items() if not passed]
print(f"benchmark evidence: {path}")
print(f"budget checks: {len(evidence['budget_checks']) - len(failed)} passed, {len(failed)} failed")
if failed:
    print("failed checks: " + ", ".join(failed))
    raise SystemExit(2)
PY
