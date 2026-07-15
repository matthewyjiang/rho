# SDK performance acceptance

This document defines the release-candidate performance budget. Measurements
must use release builds, record the machine and Rust toolchain, run enough
samples to report a median and a high percentile, and compare the same commit's
SDK-backed and retained baseline fixture where applicable.

## Required benchmarks

| Scenario | Measurement | 1.0 acceptance budget |
| --- | --- | --- |
| Runtime plus session startup | Builder through ready idle session with a scripted provider | Median no more than 2 ms and no more than 20% above the pre-SDK fixture |
| Simple completion overhead | Scripted one-turn provider excluding simulated provider delay | Median no more than 10% or 100 microseconds above the pre-SDK fixture, whichever is larger |
| Event delivery | 10,000 bounded provider deltas consumed by one run | At least 250,000 events/second median; p99 enqueue-to-consume latency below 5 ms |
| History snapshot | Clone and serialize 1,000 representative messages | Median below 10 ms and peak retained allocation below 3 times serialized size |
| Compaction orchestration | Partition, scripted summary, and atomic commit for a 1,000-message history | Median no more than 15% above the pre-SDK compaction fixture |
| Slow consumer | Producer against a full bounded event channel | Memory remains bounded and cancellation completes within 250 ms after the consumer is dropped |

Provider network latency, upstream rate limiting, authentication, OS keychain
prompts, terminal drawing, and SQLite disk latency are reported separately and
must not be attributed to SDK orchestration overhead.

## Regression policy

A result within both the absolute and relative budget is acceptable. A result
outside either budget is material and blocks 1.0 unless the pull request:

1. identifies the measured cause;
2. explains the user-visible benefit that requires the regression;
3. updates the budget with maintainer approval;
4. includes the raw before/after benchmark artifact; and
5. records the intentional change in coordinated release notes.

Noise is not a waiver. Re-run on an otherwise idle machine, increase sample
counts, and compare distributions. Do not hide regressions by changing fixture
content, event capacity, optimization settings, or benchmark boundaries between
the baseline and candidate.

## Release evidence

Run the reproducible suite from the repository root:

```bash
./scripts/run_sdk_release_benchmarks.sh
```

The script uses the release benchmark profile, 20 samples by default, the
in-target `pre-sdk-retained-fixture-v1` baseline, and writes
[`benchmarks/results/sdk-release-current.json`](../../benchmarks/results/sdk-release-current.json).
Override `RHO_BENCH_SAMPLES` or `RHO_BENCH_OUTPUT` without changing benchmark
boundaries.

The current repository evidence was recorded on an AMD Ryzen 5 5600X with
Rust 1.96.1. All nine machine-evaluated budget checks passed: 370 ns median
startup (1.057x baseline), 3.98 us simple completion, 2.09 million events per
second with 130 us reported p99 delivery latency, 301 us snapshot median with a
2.762x measured peak allocation ratio, 1.028x baseline compaction, and 4.76 us
p99 slow-consumer cancellation. These figures are machine-specific, so release
candidates must rerun the suite and commit their own raw artifact.

The release candidate must attach:

- benchmark command and commit IDs;
- CPU, memory, operating system, Rust version, and build profile;
- raw criterion or equivalent machine-readable results;
- median and p95/p99 values where applicable;
- baseline and SDK-backed deltas; and
- an explanation and approval for every material regression.

The committed artifact satisfies these evidence requirements for the measured
source tree. A later release candidate must regenerate it after all code and
toolchain changes.
