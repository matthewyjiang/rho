# Sessions search benchmark

Measured September 12, 2026 on a shared Linux development machine. The benchmark
reads local session transcripts but prints only aggregate measurements. No
conversation text, private query terms, workspace names or session IDs are in
this report.

## Decision

Use SQLite FTS5 with `porter unicode61` tokenization and BM25 ranking. SQLite is
already a Rho dependency. This was selected after comparing a case-insensitive
scan, Unicode-token BM25, Porter-token BM25 and trigram BM25.

Porter adds English inflection matching to the Unicode index with little build
or tail-latency cost. Trigrams recover arbitrary substrings, but the actual-corpus
index was 2.35 times larger and took about eight times as long to build. The scan
has very cheap early hits but must read every row for an absent term. That tail
gets worse with corpus size. The tool deliberately supports lexical topic and
identifier search, not arbitrary substring or semantic similarity search.

## Corpus and method

The final prototype run contained 641 top-level session transcripts, 536,654,838
raw bytes and 98,454 nonempty display-evidence messages. Extracted evidence was
175,451,157 bytes, about 32.7% of raw transcript size. The median message was 575
characters; p95 was 7,349. Extraction took 2.105 seconds. There were no malformed
records in this run.

The prototype ignores web sidecars and nested subagent traces. It extracts
recorded display messages instead of indexing repeated model snapshots, provider
state, accounting or tool schemas carried in provider envelopes. It does not
summarize or rewrite actual message text.

Seven fixed public queries cover topic words, multiple terms, an error code and
an absent term. Each query runs once on a fresh SQLite connection and ten more
times on that connection. **Cold means a fresh connection, not an OS page-cache
flush.** Build times include insertion and commit, but exclude transcript
extraction. Query measurements return at most ten message IDs. Scans stop early
and do not rank results, so their fast median is not equivalent ranked work.

Scaled fixtures contain 295,362 messages, three times the observed count. They
sample observed message lengths with a fixed seed and generate public synthetic
words. They retain no private text. The synthetic vocabulary makes benchmark
terms unusually frequent, stressing ranking rather than reproducing real term
frequencies.

## Prototype results

SQLite 3.46.1. Sizes below are decimal MB. Times are milliseconds except build.

### Actual corpus

| Algorithm | Build, s | DB, MB | Cold p50 / p95 | Warm p50 / p95 |
| --- | ---: | ---: | ---: | ---: |
| Case-insensitive scan | 0.242 | 204.2 | 0.886 / 232.926 | 0.683 / 234.315 |
| Unicode61 BM25 | 2.760 | 261.8 | 3.711 / 10.765 | 3.118 / 10.159 |
| Porter BM25 | 2.917 | 261.9 | 6.687 / 10.844 | 6.092 / 10.239 |
| Trigram BM25 | 23.314 | 614.7 | 22.779 / 29.127 | 22.412 / 28.264 |

### Three-times fixture

| Algorithm | Build, s | DB, MB | Cold p50 / p95 | Warm p50 / p95 |
| --- | ---: | ---: | ---: | ---: |
| Case-insensitive scan | 0.600 | 591.8 | 0.152 / 681.657 | 0.072 / 680.998 |
| Unicode61 BM25 | 7.738 | 771.9 | 72.945 / 73.968 | 72.555 / 74.624 |
| Porter BM25 | 8.151 | 771.9 | 72.873 / 74.365 | 72.301 / 73.677 |
| Trigram BM25 | 29.643 | 1,426.5 | 99.967 / 352.106 | 98.164 / 365.106 |

Porter build CPU time was 2.916 seconds actual and 8.143 seconds scaled. Appending
and committing one evidence row cost 0.164 ms actual and 0.173 ms scaled. These
small append measurements do not predict occasional SQLite merge costs.

The Python experiment's peak RSS was 830,012 KiB. That includes its in-memory
actual and synthetic corpora and is **not** an estimate of Rho's index memory.

## Retrieval quality

A small labeled fixture covers multi-term relevance, English inflection, an
error code, an identifier, a topic, a substring and absence. On its six positive
queries:

| Algorithm | MRR at 3 | Recall at 3 |
| --- | ---: | ---: |
| Scan | 1.000 | 1.000 |
| Unicode61 BM25 | 0.667 | 0.667 |
| Porter BM25 | 0.833 | 0.833 |
| Trigram BM25 | 1.000 | 1.000 |

Porter fixes the inflection gap; it intentionally does not find `point` inside
`checkpoint`. All algorithms reject the absent query. This tiny fixture checks
retrieval semantics, not general relevance quality. The actual private corpus
has no human relevance labels, so this report does not claim measured real-world
precision, recall or superiority to semantic retrieval.

## Product path

The opt-in Rust benchmark exercises the actual cache, journal, scoped query,
grouping, evidence windows, JSON output and focused read. It copies transcripts
into a private temporary root, leaving real transcripts and indexes untouched.
These measurements use a debug build, not an optimized release binary.

| Measurement | Result |
| --- | ---: |
| Initial extraction and index, 641 files | 24.686 s |
| Product SQLite database | 288,899,072 bytes |
| Fresh-connection query p50 / p95 | 39.122 / 60.574 ms |
| Warm-OS-cache query p50 / p95 | 38.730 / 60.348 ms |
| Unchanged query transcript reads / file checks | 0 / 0 |
| One new session indexed and queried | 2.337 ms |
| New-session work | 1 file, 763 bytes read |
| Focused read | 1.125 ms |
| Process peak RSS | 70,128 KiB |
| Process user / system CPU | 25.248 / 2.153 s |

Product queries reopen SQLite each time. Their extra cost over the prototype
includes scoped session grouping, deterministic tie breaks and excerpt assembly.
Initial indexing streams one transcript record at a time instead of loading the
whole corpus. Subsequent calls consume catalog changes and reindex only changed
sessions. Out-of-band filesystem edits require an explicit refresh. Cancellation
rolls back the search transaction and its journal cursor.

### Context cost

Search output p50 was 5,850 bytes and p95 was 6,054 bytes, including handles,
anchors, offsets, omission metadata and index freshness facts. A focused read
returned 4,707 bytes including metadata. At the rough four-bytes-per-token proxy,
the p95 search is about 1,514 tokens. This is a byte-based estimate, not a
model-tokenizer measurement.

The default five groups, two excerpts per group and 320-character windows keep
discovery smaller than even one p95 source message. An excerpt reserves 80
characters before its first match. These are presentation defaults, not silent
retrieval limits: matching-message counts and pagination expose omitted evidence,
and a read can expand any returned anchor. A 4,096-character default read pages
long messages instead of returning the entire transcript. The configured tool
byte budget remains the hard output boundary and is named in budget errors.

## Reproduce

From the repository root, with no provider or network access:

```bash
python3 scripts/benchmark_sessions_search.py \
  --root "$HOME/.rho/sessions" --scale 3 \
  > /tmp/rho-sessions-search-prototype.json

RHO_SEARCH_BENCH_ROOT="$HOME/.rho/sessions" \
  cargo test -j 8 -p rho-coding-agent --lib \
  sessions_search_product_benchmark -- --ignored --nocapture \
  > /tmp/rho-sessions-search-product.log 2>&1
```

The Rust benchmark is ignored in normal tests because it requires an explicit
local corpus and measures performance, not because it is a flaky regression.
Run the compiled test binary under a process resource collector to measure RSS
without including Cargo compilation. Both benchmarks remove their temporary
corpus/index data when they finish. Numbers can vary as sessions change and other
work runs on the machine.
