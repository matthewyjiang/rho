# FIXES.md — Performance, Rendering, and High-Priority Bug Fixes

Audit of `rho-coding-agent` at commit `bfa31ae` (2026-07-12). Scope: performance,
rendering performance, and high-priority correctness bugs. Every finding below was
verified against the source by direct read; line numbers refer to commit `bfa31ae`.

**Verification workflow for every fix** (per `AGENTS.md`): after the change run
`cargo fmt`, `python3 scripts/check_architecture.py`, and the narrowest relevant
tests (`cargo test <module>`). New test modules go in sibling `*_tests.rs` files
with explicit `#[path = "..."] mod tests;` declarations. Prefer
`pretty_assertions::assert_eq`.

Note: PR #198 (`d043d1f`) already fixed incremental SSE decoding, markdown
history-render reuse, and statusline caching — those classes were re-checked and
are **not** re-reported here. `HistoryLineCache` correctly virtualizes the
transcript; the findings below are the redundancies layered on top of it.

---

## Priority table (leverage order: impact ÷ effort, weighted by confidence)

| # | Fix | Category | Impact | Effort | Risk | Confidence |
|---|---|---|---|---|---|---|
| R1 | Build live history lines once per frame | Rendering | High — 2× markdown re-wrap + tool-output deep clone per frame at ~40fps while streaming | S | LOW | HIGH |
| P1 | Stop opening a fresh SQLite connection + running DDL + fsync per message | Perf | High — every persisted message pays fsync + connection open + schema DDL on the async runtime | M | MED | HIGH |
| B1 | Drain command output fully before returning (bash + powershell) | Bug | Silent truncation of tool output the model reasons on | S | LOW | HIGH |
| P2 | Defer startup update check and metadata fetch | Perf | Up to ~900ms+ added to every interactive launch | S | LOW | HIGH |
| R2 | Reuse composer/command line counts in scroll/clamp paths | Rendering | 2× composer + suggestion rebuild per scroll event and per post-event clamp | S | LOW | HIGH |
| R3 | Skip per-frame copy-target allocation when nothing hovered | Rendering | Allocation churn every frame incl. idle redraws | S | LOW | HIGH |
| P3 | Cache per-message token estimates (kill O(n²) re-estimation) | Perf | O(history bytes) serialization per step → O(n²) per long turn | M | MED | HIGH |
| P4 | Stop deep-cloning full history + tool specs per provider request | Perf | O(history size) allocations per step, incl. base64 images | M | MED | HIGH |
| R4 | Add a frame dirty flag; stop idle 10fps redraws | Rendering | CPU wakeup + full frame rebuild 10×/s while idle; battery cost | M | MED | HIGH |
| R5 | Stop full layout recompute on every mouse move/drag | Rendering | Full layout + live-line render per mouse event during streaming | M | MED | HIGH |
| B2 | Bound provider-controlled stream block index | Bug | Malformed/hostile stream event with huge `index` → memory exhaustion | S | LOW | MED |
| B3 | Write config atomically (temp file + rename) | Bug | Torn write during crash bricks startup until user deletes config | S | LOW | HIGH |
| R6 | Cache picker match results; compile filter regex once | Rendering | O(items) rescan (and regex compile) several times per frame while a picker is open | M | LOW | HIGH |
| R7 | Replace throwaway markdown render in `insert_stream_fragment` | Rendering | Full markdown render discarded per committed stream fragment (~every 24ms) | S | LOW | HIGH |
| B4 | Fix composer Up/Down cursor movement (visual rows, not `cursor ± width`) | Bug | Wrong cursor position in any multi-line or wide-char input | M | LOW | MED |
| B5 | Kill the PowerShell process tree on timeout/drop | Bug | Orphaned grandchild processes on Windows | M | MED | MED |
| B6 | Fix paste-burst fallback for 1-char first lines | Bug | Premature prompt submission on non-bracketed-paste terminals | S | MED | MED |
| P5 | Move file-tool I/O off the async runtime; stream ranged reads | Perf | Blocking syscalls on tokio workers; whole-file read for ranged reads | M | LOW | HIGH |
| P6 | Cache skills/AGENTS.md discovery across compactions | Perf | Full skill-tree re-read from disk on every compaction/reset | S | LOW | HIGH |
| P7 | Make workspace session-index sync incremental | Perf | O(all session files) stat + query on every session open/list | M | LOW | MED |
| B7 | Validate the persisted session `version` field | Bug | Latent: silent misload when the format first changes | S | LOW | HIGH |
| B8 | Position single-line-input cursor by display width | Bug | Visual cursor drift with wide chars in secret/config inputs | S | LOW | HIGH |
| R8 | Cache `session_header_lines` (built 3×/frame) | Rendering | Minor steady allocation | S | LOW | HIGH |

---

## High-priority bug fixes

### B1. Command tools can drop trailing stdout/stderr on normal completion

**Evidence**
- `src/tools/bash.rs:126-128` — the run loop breaks as soon as `child.try_wait()` reports exit.
- `src/tools/bash.rs:147-152` — after the break, only `chunk_rx.try_recv()` is drained. The spawned reader tasks (`read_stream`, `bash.rs:208-224`) may not yet have read/forwarded the final pipe contents, so bytes written just before exit are lost.
- The timeout path does it correctly: `bash.rs:133-135` awaits `child.wait()` then `drain_stream_chunks(...).await` until the channel closes — strong evidence the normal path is unintended.
- `src/tools/powershell.rs:148-153` — same bug on normal completion, **and** its timeout path (`powershell.rs:132-137`) also only uses `try_recv`, so it is worse than bash.

**Impact.** A command that emits a burst of output immediately before exiting can return truncated `content` to the model, which then reasons on incomplete tool output with no indication anything is missing.

**Fix**
1. In `bash.rs`, after `break status` and `process_group.disarm()`, drop the local extra sender (currently the senders are moved into the reader tasks, so just replace the `try_recv` drain with an awaited drain): `while let Some((kind, bytes)) = chunk_rx.recv().await { ... }`. The channel closes when both reader tasks hit EOF (which is guaranteed once the child has exited), so this terminates. `drain_stream_chunks` already implements exactly this — reuse it.
2. Apply the same change to `powershell.rs` on both the normal path (`:148-153`) and the timeout path (`:132-137`, add `let _ = child.wait().await;` before the drain, mirroring bash).
3. Tests: in `src/tools/` sibling test files, add a case running a command that writes a large final burst and exits (e.g. `printf 'x%.0s' {1..100000}`), asserting the captured length. Follow the existing `timeout_terminates_background_processes` test as a pattern.

**Risk:** LOW — matches the already-proven timeout-path pattern.

### B2. Provider-controlled `index` grows streamed-block vectors unboundedly

**Evidence**
- `src/provider_backend/anthropic/stream.rs:24-29` — `ensure_block` does `while self.blocks.len() <= index { push(default) }` where `index` comes straight from stream JSON (`content_index`, `stream.rs:201-207`).
- `src/model/openai/stream.rs:69-76` — same pattern for tool-call deltas.

**Impact.** One stream event with `index: 4000000000` (buggy provider, or a custom/compromised `api_base` gateway — `api_base` is user-configurable) allocates billions of default structs: memory exhaustion / hung turn. Network data is untrusted.

**Fix**
1. Define a small cap, e.g. `const MAX_STREAM_BLOCK_INDEX: usize = 4096;` (real responses use a handful of blocks).
2. In `content_index` (anthropic) and the OpenAI index parse, return `Err(ModelError::InvalidResponse(format!("stream block index {index} out of range")))` when the index exceeds the cap. Propagate through the existing error paths (both call sites already handle `ModelError`).
3. Tests: feed a synthetic event with a huge index through the SSE state machine; assert `InvalidResponse` instead of allocation. Follow the existing stream test modules for fixtures.

**Risk:** LOW — no behavior change for well-formed streams.

### B3. Config file is written non-atomically; a torn write bricks startup

**Evidence**
- `src/config.rs:205-208` — `write_config` does `fs::write(path, toml::to_string_pretty(...)?)`, overwriting `config.toml` in place with no temp file, rename, or fsync.
- `src/config.rs:336` — `load_with_store` propagates a parse error, so a truncated file makes startup fail until the user repairs/deletes it. (Contrast: session persistence deliberately uses append + `sync_data`, `src/session.rs:204-216`.)

**Fix**
1. In `write_config`: serialize to a `String` first; write to a sibling temp file (`config.toml.tmp` in the same directory, so rename stays on one filesystem); apply the same private-permission handling used elsewhere (`set_private_file_permissions`); `sync_all()`; then `fs::rename` over `config.toml`. On Windows `fs::rename` replaces existing files, so this is portable.
2. Optionally (separate decision): make `load` fall back to defaults with a visible warning on parse failure instead of hard-erroring. Keep this out of scope unless desired — it changes user-facing behavior.
3. Test: round-trip save/load in a tempdir (see existing config tests); no torn-write simulation needed — the atomicity is structural.

**Risk:** LOW.

### B4. Composer Up/Down moves the cursor by `± terminal width` in character units

**Evidence**
- `src/tui.rs:1639-1647` — `input_cursor` (a **char** index) is adjusted by `saturating_sub(width)` / `+ width` where `width` is the terminal column count.
- `src/tui/render.rs` `wrap_line_hard` wraps by **display width** and breaks on every `'\n'`; newlines are routinely inserted (Alt+Enter, buffered paste), so visual rows rarely contain exactly `width` chars.

**Impact.** In any multi-line input (or one containing CJK/emoji), Up/Down lands on the wrong column and often the wrong row. The single-paragraph ASCII case happens to work, masking the bug. No crash (clamped).

**Fix**
1. The code already computes the ingredients at `tui.rs:1628-1629`: `input_visual_lines(&self.input, terminal_width)` and `input_cursor_position(...)` (visual x,y). Use them: target row = `cursor_position.y ± 1`; walk the target visual line summing per-char display width until reaching `cursor_position.x` (or the line end); convert that back to a global char index by summing the char lengths of preceding visual lines.
2. Preserve the existing behavior where Up on row 0 / Down on the last row triggers input-history recall (`tui.rs:1630-1636`) — only replace the fallback movement at `:1639-1647`.
3. Tests: add cases to the existing render/cursor tests: (a) two lines `"ab\ncdef"`, cursor at end of `cdef`, Up → column clamped to end of `ab`; (b) a line with CJK chars wrapping at width; (c) pure-ASCII single paragraph (regression guard).

**Risk:** LOW — localized to arrow-key navigation.

### B5. PowerShell tool leaves the child process tree running on timeout/cancellation

**Evidence**
- `src/tools/powershell.rs:87-96` — spawns `powershell.exe` with only `.kill_on_drop(true)`; no job object / process group.
- `src/tools/powershell.rs:130-131` — timeout kills only the PowerShell process itself.
- Contrast: bash uses `.process_group(0)` + `ProcessGroupGuard` killing `-pid` (`bash.rs:92-96, 175-206`), with tests documenting tree-termination as the intended contract.

**Fix**
1. On Windows, create a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and assign the child to it right after spawn (the `windows-sys` dependency already includes `Win32_System_JobObjects`); hold the handle in a guard struct mirroring `ProcessGroupGuard` (kill on timeout, disarm on clean exit, kill on drop).
2. Simpler fallback if the job-object route stalls: run `taskkill /T /F /PID <pid>` on timeout/drop. Less robust (races with PID reuse) but a one-liner.
3. Test on Windows CI if available; otherwise gate with `#[cfg(windows)]` and mirror the bash `timeout_terminates_background_processes` test.

**Risk:** MED — Windows job-object edge cases (nested jobs on older Windows); keep the guard logic identical in shape to bash's.

### B6. Paste-burst fallback submits prematurely when a paste's first line is one character

**Evidence**
- `src/tui/paste_burst.rs:5,44-58` — `PASTE_BURST_MIN_CHARS = 2`; `push_enter_if_paste` returns `NotPaste` unless ≥2 plain chars arrived within the 12ms gap; the Enter-suppression window is only armed at ≥2 chars.
- `src/tui.rs:789-803` — `NotPaste` flushes the burst and lets the Enter fall through to normal handling (submit in `ComposerMode::Input`).

**Impact.** On terminals without bracketed paste, pasting text whose first line is a single character (`y\n...`, list/diff lines) commits the lone char and **submits the prompt**. Partial input gets sent to the model.

**Fix**
1. Treat a lone plain char followed by an Enter *within* `PASTE_BURST_GAP` as burst context: on the first plain char, record `last_event_at`; in `push_enter_if_paste`, if the Enter arrives within the gap of that char, buffer it (provisional suppression) instead of returning `NotPaste`. Keep the idle-gap guard so a hand-typed `y⏎` (inter-key interval ≫ 12ms) still submits normally.
2. Tests in `paste_burst` unit tests: (a) char + Enter 1ms apart → buffered; (b) char + Enter 100ms apart → submits; (c) existing multi-char cases unchanged.

**Risk:** MED — the heuristic separates fast typists from paste by the 12ms gap only; both directions need tests.

### B7. Session `version` field is persisted but never validated

**Evidence**
- `src/session.rs:18,57-63` — `SESSION_VERSION: u32 = 1` is written into every session entry; readers (`read_histories` `:225`, `summarize_session_file` `:288-292`) destructure with `..` and never compare it.

**Impact.** Latent: nothing is wrong at v1, but the first format change will parse old/new files with the wrong struct silently. The field currently provides false assurance.

**Fix.** On load, read `version`; if greater than `SESSION_VERSION`, skip the file with a warning (forward compatibility); leave a `match version` seam for future migrations. S effort now, M when the format actually changes.

**Risk:** LOW.

### B8. Single-line inputs place the terminal cursor by char index, not display width

**Evidence**
- `src/tui.rs:5100-5111` — `composer_cursor_position` uses the raw char cursor as the x column for `SecretInput` / `ConfigNumberInput` / `ConfigTextInput`. The main input correctly goes through `input_cursor_position` → display width (`:5095-5098`).

**Impact.** Wide (2-column) characters in a config value or pasted secret make the visible cursor drift left of its true position. Visual only; edits remain correct.

**Fix.** Derive x from the display width of the value's char-prefix `[..cursor]`, mirroring the main input. Add a wide-char case to the composer cursor tests.

**Risk:** LOW.

---

## Performance fixes (agent core)

### P1. Session persistence: per-message fsync + fresh SQLite connection + repeated DDL

**Evidence**
- `src/agent.rs:208-214` — every message (user, assistant, tool result) is appended synchronously inside the async agent loop.
- `src/session.rs:204-216` — `append_entry`: open, `set_permissions` syscall, serialize, **`sync_data()` (fsync)** per message.
- `src/session/index.rs:139-167,175-205` — `record_message` → `open_index` opens a **new** `rusqlite::Connection` every call and unconditionally runs `execute_batch` (CREATE TABLE + 2 CREATE INDEX) plus two `pragma table_info` scans (`ensure_column` ×2), then two `fs::metadata` stats.
- No `spawn_blocking` anywhere in the crate — all of this blocks tokio worker threads.

**Impact.** Tool-heavy turns persist dozens of messages; each pays fsync + connection setup + schema DDL. On slow/network filesystems an fsync can be tens of ms, stalling the agent loop and the streaming UI it drives.

**Fix (incremental, in order)**
1. **Connection reuse (cheapest win):** hold one `Connection` (e.g. in the session sink / an `OnceLock`-guarded holder keyed by `session_root`) and run the DDL + `ensure_column` once per process. All `open_index` callers (`record_created`, `record_message`, `record_replaced`, `list_workspace_sessions`, `set_title`, `sync_workspace`, `matching_session_paths`) switch to the shared handle. `rusqlite::Connection` is `!Sync` — wrap in a `Mutex` or route through a dedicated thread.
2. **Move persistence off the runtime:** wrap the file append + index update in `tokio::task::spawn_blocking`, or better, a dedicated persistence task fed by an unbounded channel so message order is preserved and the agent loop never waits on disk.
3. **Coalesce fsyncs:** keep `sync_data` per append if durability-per-message is a hard requirement; otherwise batch (sync on turn end + timer). Decide explicitly and document it in the code.

**Verify:** existing session tests (`cargo test session`); add a test asserting the index schema is created once (e.g. counting `execute_batch` via a seam, or simply timing-neutral behavioral tests for ordering).

**Risk:** MED — persistence ordering and the `rows == 0 → summarize + upsert` fallback (`index.rs:162-165`) must be preserved; error reporting timing changes if writes become async.

### P2. Startup blocks on the update check (up to ~900ms) and a metadata fetch

**Evidence**
- `src/app/bootstrap.rs:39-43` — `update::update_notice(...)` is awaited before the TUI launches; `src/update.rs` wraps a GitHub request in a 900ms timeout.
- `src/app/bootstrap.rs:45-50` — an uncached Anthropic `fetch_model_metadata` is also awaited before startup.

**Fix.** Spawn both as background tasks at bootstrap; deliver results via a channel the TUI already polls (there is precedent: `poll_model_metadata_fetch` at `src/tui.rs:667` — route the update notice the same way and render it when it arrives). The `update_notice` currently feeds the session header (`session_header_lines` takes it from `info`), so late arrival must invalidate the header line cache — trivial if R8's cache keys on the notice.

**Risk:** LOW — timing only.

### P3. Full-history token estimate re-serializes every message on each step

**Evidence**
- `src/agent.rs:272-287` — `estimate_request(&self.messages, &specs)` runs at the top of every loop step (and again post-compaction).
- `src/agent/context_tracker.rs:51-59` → `src/model/context.rs` — `estimate_context_tokens` walks **all** messages and specs, calling `serde_json::to_string` on every tool call/result each time. PR #198 made one pass shared per step; each step still pays O(total history bytes), so a long turn is O(n²).

**Fix.** Cache per-message token estimates: messages are append-only except for compaction (`replace_history`) — keep a running sum plus a parallel `Vec<u64>` of per-message estimates (or a rolling total invalidated wholesale on `replace_history`/`reset`). Tool-spec cost is constant per turn; compute once per turn. The `RequestContextEstimate` type (`context_tracker.rs:16-19`) stays the single currency, so `estimate_for_compaction` anchoring is unaffected.

**Verify:** `context_tracker` tests already pin the anchoring semantics (`one_request_estimate_drives_context_event_and_compaction_check`, etc.) — extend them with an append-then-estimate equality test: cached total == fresh full recompute.

**Risk:** MED — a stale cache would skew compaction thresholds; make `replace_history`/message mutation the only invalidation points and assert equality in tests.

### P4. Full message history + tool specs deep-cloned on every provider request

**Evidence**
- `src/agent.rs:298-301` — `ModelRequest { messages: self.messages.clone(), tools: specs.clone(), ... }` per step. History includes large tool-result strings and base64 image blocks.

**Fix.** Change `ModelRequest` to borrow (`&[Message]`) or hold `Arc<[Message]>`/`Arc<str>` content so per-step clones are pointer bumps. Borrowing touches the `ModelProvider` trait and all provider impls (anthropic, openai, github_copilot) — mechanical but wide; `Arc`-ing the content blocks is less invasive. Pair with P3 since both touch the step loop.

**Risk:** MED — trait signature change ripples through providers and tests.

### P5. File tools do blocking I/O on tokio workers; ranged reads load whole files

**Evidence**
- `src/tools/read_file.rs:61` (`std::fs::read_to_string` — even when `offset`/`limit` are set), `src/tools/edit_file.rs:78,97`, `src/tools/write_file.rs:69,77,89`, `src/tools/list_dir.rs:53`; tools run via `tokio::spawn` (`src/agent.rs:529-536`), not `spawn_blocking`.

**Fix.** Swap to `tokio::fs` equivalents (they wrap `spawn_blocking` internally); for `read_file` with `offset`/`limit`, use a `BufReader` line iterator and stop at `offset+limit` instead of materializing the file.

**Risk:** LOW — mechanical; keep edit/write semantics identical.

### P6. Every compaction re-reads all skills and AGENTS.md from disk

**Evidence**
- `src/agent.rs:132-136` — `replace_history` → `initial_messages` → `system_prompt`; `src/prompt.rs:12-59,87-99` re-reads every `AGENTS.md`; `src/skills.rs:41-92` re-walks skill roots and re-reads every `SKILL.md` — synchronously, inside the agent loop.

**Fix.** Discover once at agent construction and reuse across `replace_history`/`reset` (session-lifetime staleness is acceptable; a `/reload`-style escape hatch can invalidate if ever needed).

**Risk:** LOW.

### P7. Workspace session-index sync is O(all session files) on every open/list

**Evidence**
- `src/session/index.rs:59-91` — `sync_workspace` stats every file (2 × `fs::metadata`) + one SQLite currency check per file, re-parsing changed files fully; runs on `open_by_id_with_histories`, `list_in_root`, and `set_title` (`src/session.rs:99-104,135,141`).

**Fix.** For `open_by_id`/`set_title`, sync only the target session's file. Keep the full walk for `list_in_root` (it genuinely needs the directory), optionally short-circuiting on unchanged directory mtime.

**Risk:** LOW–MED (confidence MED: latency matters only for large workspaces) — stale-row removal semantics must be preserved for the list path.

---

## Rendering fixes

### R1. Live history lines built twice per frame (with a deep clone of the pending tool call)

**Evidence**
- `src/tui.rs:4470-4485` — `draw()` calls `history_len(width, now)` (`:4474`), which calls `history_live_lines` (`:4770-4772`) — and then `visible_history_lines` (`:4485`) calls `history_live_lines` **again** (`:4811`).
- `src/tui.rs:4844-4864` — each call clones the entire pending tool call (`pending.clone()` at `:4851`, including its `Vec<String>` display lines) and re-renders the live stream preview markdown.
- While streaming, frames render at up to ~40fps, so a tool with large output pays two deep clones + two markdown re-wraps per frame.

**Fix.** Build the live lines once per draw and share: e.g. compute `let live = self.history_live_lines(width, now);` at the top of `draw`, derive `history_len = history_static_len + live.len()`, and pass `&live` (or a cached field cleared at frame start) into `visible_history_lines`. Also make `entry_lines` accept `&Entry`-composable input to avoid the `pending.clone()` (construct `Entry::Tool` by reference or refactor `entry_lines` to take the tool entry parts). The scroll/clamp paths (`history_len` callers at `:4940,4985`, mouse handlers) can keep the lazy version — the frame path is the hot one.

**Risk:** LOW — pure reuse; add a render test asserting identical output before/after (see `render_tests.rs` patterns from PR #198).

### R2. `history_height_for_screen` rebuilds composer + suggestion lines, twice per scroll/clamp

**Evidence**
- `src/tui.rs:4884-4897` — it calls `self.composer_lines(width)` and `self.command_suggestion_lines(width)` per call, only to take `.len()`.
- `src/tui.rs:4941-4942` (`scroll_history_lines`) and `:4986-4987` (`clamp_history_scroll`) each call it **twice** (reserved + unreserved), and both also call `history_len`. `clamp_history_scroll_for_terminal` runs after most input events.

**Fix.** The clean seam already exists: `history_height_from_line_counts` (`:4899-4918`). Compute `composer_lines(width).len()` and `command_suggestion_lines(width).len()` once per caller and call `history_height_from_line_counts` twice with the two `include_jump_button` values. This compounds with R6 while a picker is open (composer lines include picker rendering).

**Risk:** LOW.

### R3. `code_block_copy_targets` allocated every frame even when unused

**Evidence**
- `src/tui.rs:4486` — `draw` unconditionally builds the target Vec (Arc clone + Range per cached code block, `:4831-4842`); it is only consumed when `self.hovered_code_block_copy` is `Some` (`:4495`).

**Fix.** Move the computation inside the `if let Some(hovered_line) = self.hovered_code_block_copy` branch.

**Risk:** LOW — one-line reorder.

### R4. Unconditional redraw every ≤100ms while idle (no frame dirty flag)

**Evidence**
- `src/tui.rs:666-690` — the main loop calls `terminal.draw(...)` on **every** iteration; `event::poll` times out at ≤100ms (`event_poll_timeout`, `:736-744`), so an idle app rebuilds the full frame 10×/second (ratatui diffing only saves terminal writes, not Rust-side frame construction).

**Fix.** Add a `needs_redraw: bool` set by: any handled terminal event, agent/stream events, paste-burst flushes, and active animation timers (spinner, scrollbar reveal window, copy notice — `event_poll_timeout` already knows these deadlines). Only call `terminal.draw` when set; when idle with no animation deadline, poll with a long timeout. Sequence this **after** R1/R2 land so invalidation points are fewer and clearer.

**Risk:** MED — a missed invalidation shows a stale UI. Mitigate: default `needs_redraw = true` after every `handle_terminal_event` and every agent-event drain (coarse but safe — the win is the idle case, which stays precise).

### R5. Every mouse move/drag runs a full layout pass + copy-target allocation

**Evidence**
- `src/tui/mouse.rs:140-149` (`Moved`) — `screen_layout(...)` (which rebuilds composer/command lines and live history length), then `history_len` **again**, then `code_block_copy_targets`. `Drag` (`:77-105`) and `Up` (`:107-117`) repeat the same work. Mouse-move fires many times per second; during streaming each event also re-renders the live stream preview (compounds R1).

**Fix.** (a) Short-circuit `Moved` when the pointer cell hasn't changed (store last `(column,row)`). (b) Reuse one `screen_layout` result per event instead of layout + separate `history_len` (the layout already computed it). (c) Only compute copy targets when the pointer is inside the history rect. Preserve hit-testing exactly — this is refactor-only.

**Risk:** MED — stateful hover/selection/scrollbar interactions; cover with the existing `mouse_tests.rs`.

### R6. Picker matches recomputed several times per frame; filter regex recompiled per call

**Evidence**
- `src/tui/picker.rs:165-180` — `picker_matching_indices` builds a fresh `RegexBuilder` and rescans all items on every invocation; `selected_item` (`:157-162`) calls `matching_indices()` again; `render.rs` calls both while rendering picker lines, and the composer render path runs per frame.
- The `@`-file picker uses `fuzzy_picker_matching_indices` (`:182-189`) — no regex, but a full fuzzy score + sort of every item per call, several times per frame on a large repo.

**Fix.** Cache `(filter_string, Vec<usize>)` on the `Picker` (invalidate when `filter` changes) so all per-frame callers share one computation; store the compiled `Regex` alongside. This also fixes typing latency in large file pickers.

**Risk:** LOW — pure memoization; picker tests exist for match semantics.

### R7. `insert_stream_fragment` renders markdown into a discarded buffer

**Evidence**
- `src/tui.rs:3242-3250` — for assistant fragments it builds `text_lines` via `push_wrapped_markdown` and never uses them; the only effect is advancing `self.assistant_stream_in_code_block`. Runs per committed fragment (~every 24ms during streaming).

**Fix.** Replace with a lightweight fence scan that toggles the flag using the same rule the renderer applies to ``` fences (see `markdown.rs` fence handling). Add a test: fragment sequence with a split code fence produces the same flag trajectory as the full render.

**Risk:** LOW — but the fence semantics must match exactly (indented fences, ~~~ variants if supported).

### R8. `session_header_lines` rebuilt three times per frame

**Evidence**
- `src/tui/render.rs:30-53` — two `"─".repeat(width)` allocations per call; called from `visible_history_lines` (`tui.rs:4787`), `history_static_len` (`:4821`), and `code_block_copy_targets` (`:4832`).

**Fix.** Cache keyed by `(width, update_notice)`; invalidate on resize and when the notice changes (interacts with P2's async notice delivery). Alternatively, R3 removes one of the three call sites for free. Lowest priority — do it opportunistically alongside R1.

**Risk:** LOW.

---

## Suggested sequencing

1. **Quick, independent wins:** B1, B2, B3, B8, R3, R2, P2, P6 (all S effort, LOW risk).
2. **Frame-path cluster (do in order):** R1 → R5 → R4 (dirty flag last, when invalidation points are fewest), R7, R6, R8 alongside.
3. **Agent-loop cluster:** P1 (connection reuse first, then off-thread persistence), then P3 + P4 together (both touch the step loop).
4. **Behavioral bug fixes needing care:** B4 (cursor math), B6 (paste heuristic — needs both-direction tests), B5 (Windows-only, needs Windows validation).
5. **Latent/low:** B7, P5, P7.

## Checked and cleared (do not re-audit)

- SSE line decoding, UTF-8/CRLF/partial-chunk handling (`provider_backend/line_decoder.rs`) — solid post-#198.
- `HistoryLineCache` virtualization/invalidation and `StatusLine` caching — correct; the transcript is *not* re-wrapped per frame.
- Byte-slice/char-boundary safety across `markdown.rs`, `render.rs`, `stream.rs`, `text_selection.rs`, input editing; saturating coordinate math in `scrollbar.rs`/`mouse.rs`; markdown table row-length normalization. No reproducible panic found.
- `edit_file` CRLF normalization and non-overlapping replace-all; diff size guard (128 KiB); compaction arithmetic (saturating); credential chunking round-trip; bounded token-refresh retries; regexes in web tools are `LazyLock`-compiled; provider HTTP client reuse.

**Not audited** (out of requested scope): security posture, test-coverage gaps, dependency currency, docs, and product direction. `src/tui.rs` was audited via targeted reads of its hot paths, not an exhaustive line-by-line review of all 264KB.
