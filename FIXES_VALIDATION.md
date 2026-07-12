# FIXES.md implementation and validation matrix

This matrix maps every finding in `FIXES.md` to its implementation and validation evidence. The implementation preserves per-message `sync_data` durability while moving persistence onto an ordered worker thread.

| ID | Implementation evidence | Validation evidence |
|---|---|---|
| B1 | `src/tools/bash.rs`, `src/tools/powershell.rs` await reader-channel closure after process exit and timeout | `bash_output_tests::captures_large_final_output_burst`; `powershell_tests::captures_large_final_output_burst` |
| B2 | Anthropic and OpenAI stream parsers reject block indices above the bounded maximum | `anthropic::stream_index_tests::rejects_out_of_range_content_block_index`; `openai::stream_tests::rejects_out_of_range_tool_call_index` |
| B3 | `src/config_writer.rs` writes, sets private permissions, syncs, and atomically replaces the destination, including `ReplaceFileW` on Windows | `config_atomic_tests::saves_config_atomically_and_round_trips`; Windows CI platform check |
| B4 | Composer vertical movement uses visual lines, display columns, and a visual-line-to-character-index conversion | `render_tests::visual_cursor_movement_clamps_to_shorter_explicit_line`; `visual_cursor_movement_uses_wide_character_columns`; `visual_cursor_movement_preserves_ascii_wrapped_column` |
| B5 | PowerShell children are assigned to a kill-on-close Windows Job Object held by a guard | `powershell_tests::timeout_terminates_background_processes`; Windows CI platform check |
| B6 | A single character followed by Enter inside the paste gap is buffered provisionally | `paste_burst::tests::single_char_enter_is_buffered_as_paste`; `enter_after_idle_gap_is_not_part_of_paste`; `rapid_plain_text_burst_extends_enter_suppression` |
| B7 | Session readers validate `version` and reject unsupported newer formats | `session_version_tests::rejects_sessions_from_a_newer_format_version` |
| B8 | Secret and config input cursors use prefix display width rather than character count | `render_tests::char_prefix_width_accounts_for_wide_characters` and composer cursor tests in `src/tui.rs` |
| P1 | `src/session/index.rs` reuses one schema-initialized SQLite connection per root; `SessionHistorySink` queues ordered writes on a dedicated thread; `sync_data` durability is retained | `agent::history::tests::persists_queued_messages_in_order_before_drop_returns`; session test suite |
| P2 | Bootstrap spawns update checking; model metadata starts from the TUI background fetch path; completed handles invalidate redraw state | app and TUI test suites; macOS, Windows, and Linux CI checks |
| P3 | `ContextTracker` caches the append-only per-message token sum and invalidates it on reset/history replacement | `context_tracker::tests::appended_message_cache_matches_fresh_estimate` plus anchoring and reset tests |
| P4 | `ModelRequest` borrows message and tool slices and all providers consume borrowed requests | provider and agent test suites; Clippy across all targets and features |
| P5 | File tools use `tokio::fs`; ranged reads use an async buffered reader and stop at the requested limit | `read_file_tests`, especially `ranged_read_stops_after_limit` |
| P6 | Agent construction caches the initial system message and reset/compaction reuse it rather than rediscovering skills and AGENTS.md | `agent::tests::replace_history_keeps_initial_system_message`; reset and compaction tests |
| P7 | Session open and title updates synchronize only matching target session files; full workspace synchronization remains for listing | session open, title, backfill, stale-row, and ordering tests in `src/session.rs` |
| R1 | Draw builds live history once and passes it to visible-history rendering; pending tool rendering borrows tool fields | TUI render/layout suite and full test suite |
| R2 | Scroll and clamp callers compute composer and suggestion counts once and reuse `history_height_from_line_counts` | TUI layout scrolling and clamping tests |
| R3 | Code-copy targets are built only when a code block is hovered | mouse code-copy hover/copy tests and TUI layout suite |
| R4 | Main loop tracks `needs_redraw`, redraws after events/background completion/animation deadlines, and uses a long idle poll timeout | loading-spinner, scrollbar reveal, copy-notice, and transcript/status mutation tests; full platform CI |
| R5 | Mouse handling skips duplicate cells, reuses `ScreenLayout.history_len`, and computes copy targets only inside history | `mouse_tests::code_block_copy_button_hovers_and_copies_raw_contents`; drag-selection and scrollbar tests |
| R6 | `UiPicker` caches filter, compiled regex, and matching indices, invalidating when the filter changes | `render_performance_tests::picker_cache_invalidates_when_filter_changes`; picker matching/navigation tests |
| R7 | Stream insertion uses `update_code_block_state` instead of a discarded markdown render | `render_performance_tests::code_block_state_scan_matches_markdown_rendering` |
| R8 | Session header lines are cached by width and update notice | `render_performance_tests::session_header_cache_tracks_width_and_notice` |

## Repository-wide gates

- `cargo fmt --check`
- `python3 scripts/check_architecture.py`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
- GitHub CI Linux test and build jobs
- GitHub CI `macos-latest` platform check
- GitHub CI `windows-latest` platform check
- mergeability check against current `origin/main`
