#!/usr/bin/env python3
"""Verify that every FIXES.md finding has implementation and regression evidence."""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


@dataclass(frozen=True)
class Evidence:
    implementation: tuple[tuple[str, str], ...]
    validation: tuple[tuple[str, str], ...]


EVIDENCE = {
    "B1": Evidence(
        (("src/tools/bash.rs", "drain_stream_chunks(&mut chunk_rx"), ("src/tools/powershell.rs", "drain_stream_chunks(&mut chunk_rx")),
        (("src/tools/bash_output_tests.rs", "captures_large_final_output_burst"), ("src/tools/powershell_tests.rs", "captures_large_final_output_burst")),
    ),
    "B2": Evidence(
        (("src/provider_backend/anthropic/stream.rs", "MAX_STREAM_BLOCK_INDEX"), ("src/model/openai/stream.rs", "MAX_STREAM_BLOCK_INDEX")),
        (("src/provider_backend/anthropic/stream_index_tests.rs", "rejects_out_of_range_content_block_index"), ("src/model/openai/stream_tests.rs", "rejects_out_of_range_tool_call_index")),
    ),
    "B3": Evidence(
        (("src/config_writer.rs", "sync_all()"), ("src/config_writer.rs", "ReplaceFileW"), ("src/config_writer.rs", "Uuid::new_v4")),
        (("src/config_atomic_tests.rs", "saves_config_atomically_and_round_trips"),),
    ),
    "B4": Evidence(
        (("src/tui.rs", "input_cursor_index_on_visual_line"),),
        (("src/tui/render_tests.rs", "visual_cursor_movement_clamps_to_shorter_explicit_line"), ("src/tui/render_tests.rs", "visual_cursor_movement_uses_wide_character_columns"), ("src/tui/render_tests.rs", "visual_cursor_movement_preserves_ascii_wrapped_column")),
    ),
    "B5": Evidence(
        (("src/tools/powershell.rs", "JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE"), ("src/tools/powershell.rs", "AssignProcessToJobObject")),
        (("src/tools/powershell_tests.rs", "timeout_terminates_background_processes"),),
    ),
    "B6": Evidence(
        (("src/tui/paste_burst.rs", "suppress_enter_until"),),
        (("src/tui/paste_burst.rs", "single_char_enter_is_buffered_as_paste"), ("src/tui/paste_burst.rs", "enter_after_idle_gap_is_not_part_of_paste")),
    ),
    "B7": Evidence(
        (("src/session.rs", "validate_session_version"),),
        (("src/session_version_tests.rs", "rejects_sessions_from_a_newer_format_version"),),
    ),
    "B8": Evidence(
        (("src/tui.rs", "char_prefix_display_width"),),
        (("src/tui/render_tests.rs", "char_prefix_width_accounts_for_wide_characters"),),
    ),
    "P1": Evidence(
        (("src/session/index.rs", "INDEX_CONNECTIONS"), ("src/agent/history.rs", "rho-session-persistence"), ("src/session.rs", "sync_data()")),
        (("src/agent/history_tests.rs", "persists_queued_messages_in_order_before_drop_returns"), ("src/session.rs", "append_message_updates_session_summary")),
    ),
    "P2": Evidence(
        (("src/app/bootstrap.rs", "pending_update_notice"), ("src/tui.rs", "pending_model_metadata"), ("src/tui.rs", "background_ready")),
        (("src/tui.rs", "transcript_and_status_mutations_do_not_require_a_terminal"), ("src/tui.rs", "loading_spinner_advances_frames")),
    ),
    "P3": Evidence(
        (("src/agent/context_tracker.rs", "cached_message_tokens"), ("src/agent/context_tracker.rs", "message_appended")),
        (("src/agent/context_tracker.rs", "appended_message_cache_matches_fresh_estimate"), ("src/agent/context_tracker.rs", "reset_clears_provider_window_and_unknown_state")),
    ),
    "P4": Evidence(
        (("src/provider_backend/mod.rs", "messages: &'a [Message]"), ("src/provider_backend/mod.rs", "tools: &'a [ToolSpec]")),
        (("src/agent_tests.rs", "without_system_prompt_sends_only_user_message"),),
    ),
    "P5": Evidence(
        (("src/tools/read_file.rs", "tokio::fs::File::open"), ("src/tools/edit_file.rs", "tokio::fs::read_to_string"), ("src/tools/write_file.rs", "tokio::fs"), ("src/tools/list_dir.rs", "tokio::fs::read_dir")),
        (("src/tools/read_file_tests.rs", "ranged_read_stops_after_limit"), ("src/tools/read_file_tests.rs", "reads_selected_line_range")),
    ),
    "P6": Evidence(
        (("src/agent.rs", "initial_system_message"),),
        (("src/agent_tests.rs", "replace_history_keeps_initial_system_message"), ("src/agent_tests.rs", "reset_clears_history_back_to_system_prompt")),
    ),
    "P7": Evidence(
        (("src/session.rs", "sync_session_file"),),
        (("src/session.rs", "opens_session_by_uuid_prefix"), ("src/session.rs", "set_title_updates_session_summary"), ("src/session.rs", "list_removes_stale_index_rows")),
    ),
    "R1": Evidence(
        (("src/tui.rs", "let live_history = self.history_live_lines"), ("src/tui/render.rs", "tool_entry_lines")),
        (("src/tui.rs", "history_lines_include_header_transcript_pending_preview_and_spinner"),),
    ),
    "R2": Evidence(
        (("src/tui.rs", "history_height_from_line_counts"),),
        (("src/tui/tests/layout_tests.rs", "manual_scroll_preserves_top_line_when_new_output_arrives"), ("src/tui/tests/layout_tests.rs", "small_scroll_to_rendered_bottom_resumes_bottom_following")),
    ),
    "R3": Evidence(
        (("src/tui.rs", "if let Some(hovered_line) = self.hovered_code_block_copy"),),
        (("src/tui/tests/mouse_tests.rs", "code_block_copy_button_hovers_and_copies_raw_contents"),),
    ),
    "R4": Evidence(
        (("src/tui.rs", "let mut needs_redraw = true"), ("src/tui.rs", "Duration::from_secs(3600)")),
        (("src/tui.rs", "loading_spinner_advances_frames"), ("src/tui/tests/layout_tests.rs", "scrollbar_renders_briefly_after_mouse_wheel_scroll")),
    ),
    "R5": Evidence(
        (("src/tui/mouse.rs", "last_mouse_position"), ("src/tui/mouse.rs", "layout.history_len")),
        (("src/tui/tests/mouse_tests.rs", "dragging_transcript_text_copies_on_mouse_release"), ("src/tui/tests/mouse_tests.rs", "code_block_copy_button_hovers_and_copies_raw_contents")),
    ),
    "R6": Evidence(
        (("src/tui/picker.rs", "PickerMatchCache"), ("src/tui/picker.rs", "_regex: regex")),
        (("src/tui/tests/render_performance_tests.rs", "picker_cache_invalidates_when_filter_changes"),),
    ),
    "R7": Evidence(
        (("src/tui.rs", "update_code_block_state(render_text"),),
        (("src/tui/tests/render_performance_tests.rs", "code_block_state_scan_matches_markdown_rendering"),),
    ),
    "R8": Evidence(
        (("src/tui.rs", "SessionHeaderCache"),),
        (("src/tui/tests/render_performance_tests.rs", "session_header_cache_tracks_width_and_notice"),),
    ),
}


def require_contains(kind: str, finding: str, path: str, needle: str) -> None:
    content = (ROOT / path).read_text(encoding="utf-8")
    if needle not in content:
        raise SystemExit(f"{finding}: missing {kind} evidence {path}: {needle!r}")


def main() -> None:
    findings = set(re.findall(r"^### ([BPR]\d+)\.", (ROOT / "FIXES.md").read_text(), re.MULTILINE))
    matrix = set(re.findall(r"^\| ([BPR]\d+) \|", (ROOT / "FIXES_VALIDATION.md").read_text(), re.MULTILINE))
    configured = set(EVIDENCE)
    if findings != configured or findings != matrix:
        raise SystemExit(
            f"finding mismatch: FIXES={sorted(findings)}, evidence={sorted(configured)}, matrix={sorted(matrix)}"
        )
    for finding in sorted(findings, key=lambda item: ("BPR".index(item[0]), int(item[1:]))):
        evidence = EVIDENCE[finding]
        for path, needle in evidence.implementation:
            require_contains("implementation", finding, path, needle)
        for path, needle in evidence.validation:
            require_contains("validation", finding, path, needle)
        print(f"{finding}: PASS ({len(evidence.implementation)} implementation, {len(evidence.validation)} validation checks)")
    print(f"all {len(findings)} FIXES.md findings have implementation and validation evidence")


if __name__ == "__main__":
    main()
