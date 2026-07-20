# Usage ledger

Rho's usage ledger is a durable, provider-neutral SQLite interface for accounting tools. It is separate from session persistence and is not rebuildable from transcripts. Rho writes one row for each provider request across interactive, automation, delegated-agent, compaction, title-generation, and goal-evaluation paths.

## Location and access

The database is `~/.rho/usage.sqlite3` by default. Setting `RHO_HOME` changes Rho's data root, including the ledger, to `$RHO_HOME/usage.sqlite3`. Rho creates the data directory and database with mode `0700` and `0600`, respectively, on Unix.

The database uses WAL mode and a five-second busy timeout. Multiple Rho processes can write concurrently, and reporting tools can use SQLite's read-only mode while Rho is active. Readers should tolerate normal SQLite `-wal` and `-shm` sidecar files.

## Compatibility and schema

`PRAGMA user_version` versions database migrations. Version 1 contains `usage_events`; each immutable row also has a `schema_version` for row-semantics evolution. Future schema changes should be additive, and existing columns will be preserved. Readers should ignore columns they do not recognize and reject a `user_version` newer than they support.

Each row represents one actual provider request. `event_id` is the primary key, and writes use `INSERT OR IGNORE`, so retrying persistence with the same event is safe. A separately billed provider retry has a different event ID and its own row.

| Column | Semantics |
| --- | --- |
| `event_id` | Stable opaque request-event identity. |
| `schema_version` | Semantics version for this row, initially `1`. |
| `occurred_at_ms` | UTC Unix timestamp in milliseconds. |
| `session_id`, `parent_session_id`, `run_id` | Optional Rho execution identities. |
| `step_index`, `attempt_index` | Optional one-based orchestration step and provider-attempt indexes. |
| `workspace_path` | Optional workspace identity as recorded by the caller. |
| `provider`, `model` | Exact provider and model identities used for the request, without pricing aliases. |
| `purpose` | Open string describing why the model was called. Documented values are `agent`, `subagent`, `compaction`, `title`, and `goal`. |
| `request_outcome` | `completed`, `failed`, or `cancelled`. Usage observed before failure or cancellation is still recorded. |
| `input_tokens` | Uncached input tokens. |
| `output_tokens` | Output tokens. |
| `cache_read_tokens` | Provider-reported cache-read tokens. |
| `cache_write_tokens` | Provider-reported cache-write tokens. |
| `total_tokens` | Provider-reported total. It is not recomputed or redistributed. |
| `cost_usd_micros` | Provider-reported USD micros, stored without floating-point conversion. Local estimates are not stored. |
| `rho_version` | Optional Rho version that wrote the row. |

All token and cost fields are independently nullable. `NULL` means the provider did not report that value and is distinct from zero. Cache categories are preserved as reported and are not added to `input_tokens`. `ModelUsage::context_window` is not request consumption and is not stored. Rust `u64` values larger than SQLite's signed 64-bit integer maximum are rejected, and no partial row is inserted.

## Privacy

The ledger stores accounting and execution identity metadata only. It must never contain prompts, responses, reasoning, tool arguments, credentials, provider payloads, or transcript content. Workspace paths and caller-provided identifiers may still be sensitive metadata, so access is restricted by filesystem permissions.
