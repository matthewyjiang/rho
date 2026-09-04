# Runtime copying and streaming performance audit

This change removes repeated history copies, repeated scans during streamed code
rendering, and an unused raw-reasoning buffer. It also shares cached web responses
and avoids counting context twice when automatic compaction leaves history unchanged.

Session replay accumulates deltas before materializing the active snapshot. Saves
still compare the complete history prefix and retain separate model/snapshot
histories; they are not constant-time. COPY payloads for open code fences are
materialized when requested, rather than after every streamed fragment.

The web-cache changes also prevent storage I/O from publishing into a different
session after a rebind. Its existing payload budget now includes serialized
metadata, without allocating a serialized copy just to count it. The budget does
not claim to include allocator/container overhead or outstanding reader references.

## Measurements

The results and individual timing samples are recorded in `performance-audit.json`.
These are local hot-path measurements, not end-to-end model-response speedups.

Measured September 4, 2026. Representative median results:

| Workload | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| Load 4,000 turns | 10,368.7 ms | 38.48 ms | 269.5× |
| Summarize 4,000 turns | 10,315.1 ms | 38.30 ms | 269.4× |
| Save after 4,000 turns | 7.189 ms | 1.642 ms | 4.38× |
| Stream 4,096 code fragments | 321.52 ms | 6.44 ms | 49.9× |
| 128 tail updates, 4,096 history entries | 0.932 ms | 0.0856 ms | 10.9× |
| Wrap 4,096 styled spans | 2.325 ms | 0.320 ms | 7.26× |
| Codex bookkeeping, 1,024 items | 1.662 ms | 0.395 ms | 4.21× |
| Completion with compaction-policy estimate | 3.571 ms | 1.920 ms | 1.86× |
| Capture and forward 1 MiB of reasoning | 0.474 ms | 0.410 ms | 1.16× |
| Warm web selection, 8 MiB unselected sibling | 160.8 µs | 0.105 µs | 1,537× |
| Policy-disabled completion control | 1.931 ms | 2.080 ms | 0.93× |

The web result selects a tiny item and excludes fetching. It demonstrates that
lookup no longer copies unrelated stored bodies, not a faster network search.

The policy-disabled control did not show a consistent improvement. Its aggregate
median was 7.7% slower, with one candidate SDK process round slower in both policy
modes. All samples are retained; no speedup is claimed for policy-disabled runs.

For the separate 64 MiB reasoning-stream experiment, median whole-child peak RSS
fell from **70,428 KiB to 11,368 KiB**, or about **68.8 MiB to 11.1 MiB**. The raw
artifact contains all three process peaks for each version.

## Method

- Baseline: `06d1ff283a44eb7faa0dae19f9b1aa4817402d84`, with benchmark-only module
  declarations and fixtures added. The Codex benchmark uses the baseline's borrowed
  `record_success` argument; candidate code consumes it.
- Candidate implementation: `79443daee12d91a7432477fcc2b55ab844c63db9`.
- Linux x86-64, AMD Ryzen 5 5600X, Rust 1.92.0.
- Optimized test profile: `CARGO_PROFILE_TEST_OPT_LEVEL=3`,
  `CARGO_PROFILE_TEST_DEBUG=0`. Both versions use the same locked dependencies and
  four build jobs. This is not a release-build or network benchmark.
- Saved executables run on one allowed logical CPU. Three process rounds alternate
  baseline/candidate, candidate/baseline, then baseline/candidate. No builds run
  during measurements.
- Each process reports five or seven samples, giving 15 or 21 samples per case.
  Medians and nearest-rank p95 values use all samples, without dropping outliers.
  Repeated-call batches are normalized per call; streamed-response and tail-update
  cases report the complete workload.
- Fixtures are prepared outside timed regions. Assertions verify restored state or
  output where appropriate; no wall-clock thresholds enter the normal test suite.
- Session fixtures use temporary files under `/tmp`, which is tmpfs on this
  machine. File-cache warmup is excluded; save figures do not represent physical
  disk or cold-storage latency.

### Workloads and boundaries

| Path | Workload | Included in timing |
| --- | --- | --- |
| Session load and summary | 250, 1,000, or 4,000 append-only turns; two messages per turn and a 1 KiB assistant body | Actual JSONL loading/replay; summary adds index-record construction |
| Session save | Same growing histories; one additional turn per sample | Warm-cache save, file append, and index update; snapshot construction excluded |
| Streamed fence | 256, 1,024, or 4,096 plain-code fragments | `App::push_transcript_entry` and `frame_context`; no terminal I/O |
| Tail range updates | 256, 1,024, or 4,096 measured history entries, then 128 updates | Actual cache append and line-count maintenance |
| Styled wrapping | 256, 1,024, or 4,096 styled Unicode spans, width 80 | Actual hard wrapping, including Unicode width handling and output allocation |
| Codex continuation | 64 or 1,024 input items with 2 KiB tool outputs | Candidate construction and successful-state replacement; no transport |
| Context estimation | 256 messages with 8 KiB opaque replay data each | Scripted `session.complete`, including snapshots/events/commit; policy disabled is a control |
| Web selection | Small selected item with a 256 KiB, 1 MiB, or 8 MiB unselected sibling | Warm `get_search_content` selection and formatting; no disk/network fetch |
| Reasoning capture | 16,384 deltas of 64 bytes | Actual SDK event capture and forwarding, including event-string ownership |

The long-session fixture deliberately has no compaction boundaries. It exposes
scaling with accumulated history rather than predicting every user's resume time.
The context-estimation fixture includes other SDK work, so its result is not the
isolated ratio between one estimator scan and two scans.

The separate memory experiment sends a 64 MiB reasoning stream once and records
whole-child peak RSS with Python's `resource.getrusage`. The consumer drops forwarded events.
This isolates the SDK's duplicate retention; a real TUI or host may retain the
forwarded text for display. It is not a claim about total interactive-session RSS;
the reported process peak also includes startup and launcher overhead.

## Reproducing

The ignored measurements use the `perf_audit` filter. Baseline-compatible
instrumentation lives in:

- `crates/rho/src/session/performance_growth.rs`, declared by
  `session/performance_benchmarks.rs`.
- `crates/rho/src/tui/scaling_benchmarks.rs`, declared by
  `tui/performance_benchmarks.rs`.
- `crates/rho/src/tools/web/performance_benchmarks.rs`, declared by `web/mod.rs`.
- `crates/rho-sdk/src/orchestration/context_estimate_perf_tests.rs` and
  `stream_capture_perf_tests.rs`, declared by their owning implementation modules.
- `crates/rho-providers/src/providers/openai/codex_continuation_perf_tests.rs`.

Copy those files and their test-only module declarations to a separate baseline
checkout. Only the Codex benchmark needs a call-site adaptation:
`record_success(candidate, ...)` becomes `record_success(&candidate, ...)`.
Do not copy the implementation changes to the baseline.

Build each checkout separately and preserve its executables before building the
other version:

```bash
# Required when reusing a target directory across worktrees: source timestamps
# can otherwise let Cargo reuse the other checkout's test executables.
touch crates/{rho,rho-sdk,rho-providers,rho-tools,rho-tui-pty}/src/lib.rs
CARGO_BUILD_JOBS=4 CARGO_PROFILE_TEST_OPT_LEVEL=3 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test -p rho-coding-agent -p rho-providers -p rho-sdk \
  --lib --locked --no-run --message-format=json > build.jsonl 2> build.log
```

Read the executable paths from Cargo's `compiler-artifact` records and require
`fresh: false` for all three test executables. Save separate copies and hashes.
Run each saved binary with `TEST_BIN` set to its path and `CPU` set to an allowed
logical CPU:

```bash
RHO_BENCH_SAMPLES=7 taskset -c "$CPU" "$TEST_BIN" \
  perf_audit --ignored --nocapture --test-threads=1
```

For the separate memory experiment, use the saved SDK test executable:

```bash
RHO_BENCH_REASONING_CHUNKS=1048576 RHO_BENCH_ITERATIONS=1 RHO_BENCH_SAMPLES=1 \
  python3 - "$CPU" "$SDK_TEST_BIN" <<'PY'
import resource
import subprocess
import sys

subprocess.run([
    "taskset", "-c", sys.argv[1], sys.argv[2],
    "perf_audit_reasoning_capture", "--ignored", "--nocapture", "--test-threads=1",
], check=True)
print(resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss)
PY
```

On Linux, `ru_maxrss` reports peak RSS in KiB. Each wrapper runs exactly one child
so an earlier child cannot contaminate the maximum. Repeat in alternating order
and retain individual measurements, rather than timing Cargo's build-and-test invocation.
