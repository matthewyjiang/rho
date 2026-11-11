//! End-to-end Rho TUI scenarios driven through a PTY.
//!
//! These tests require a Unix PTY and a debug-built `rho` binary with the
//! fixture matrix (`RHO_TUI_TEST_MODE=matrix`).

#![cfg(unix)]

#[path = "support/acp_e2e.rs"]
mod acp_e2e;
#[path = "support/tui_pty_antigravity_runtime.rs"]
mod antigravity_runtime;
#[path = "support/tui_pty_attach_previews.rs"]
mod attach_previews;
#[path = "support/tui_pty_attach_view.rs"]
mod attach_view;
#[path = "support/claude_e2e.rs"]
mod claude_e2e;
#[path = "support/tui_pty_claude_runtime.rs"]
mod claude_runtime;
#[path = "support/composer_unicode.rs"]
mod composer_unicode;
#[path = "support/tui_pty_cursor_runtime.rs"]
mod cursor_runtime;
#[path = "support/key_checking_provider.rs"]
mod key_checking_provider;
#[path = "support/tui_pty_login.rs"]
mod login;

use std::{fs, path::PathBuf, time::Duration};

use rho_tui_pty::{
    run_named, IsolatedHome, Key, PtyHarness, PtySize, RhoLaunchPlan, ScenarioRunner, WaitTimeout,
};

fn runner() -> ScenarioRunner {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let artifacts = std::env::var_os("RHO_PTY_ARTIFACTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("rho-pty-test-artifacts"));
    ScenarioRunner::new(binary).with_artifacts(artifacts)
}

// Covers: rewind defaults on, restores before the turn, and respects explicit opt-out.
// Owner: interactive workspace rewind UX
#[test]
fn workspace_rewind() {
    for scenario in ["workspace_rewind", "workspace_rewind_off"] {
        assert_pass(scenario);
    }
}

/// Where an inline scenario's session runs.
#[derive(Clone, Copy)]
enum SessionHost {
    InProcess,
    Daemon,
}

impl SessionHost {
    fn launch(self, plan: RhoLaunchPlan) -> RhoLaunchPlan {
        match self {
            Self::InProcess => plan,
            Self::Daemon => plan.with_env("RHO_TUI_DAEMON", "1"),
        }
    }
}

fn assert_pass(name: &str) {
    let outcome = run_named(&runner(), name).expect("scenario runner error");
    assert!(
        outcome.passed,
        "scenario {name} failed:\n{}",
        outcome.message
    );
}

// Covers: confirmation rejection/failure parks remaining queued follow-ups.
// Owner: interactive turn orchestration (PTY).
#[test]
fn send_confirm_handoff() {
    assert_pass("send_confirm_handoff");
}

// Covers: review a long plan and preserve approval feedback in model history.
// Owner: interactive UX (PTY).
#[test]
fn plan_exit_approve() {
    assert_pass("plan_exit_approve");
}

// Covers: reserved approval text in feedback must not grant permission or execute.
// Owner: interactive UX (PTY).
#[test]
fn plan_exit_keep_planning() {
    assert_pass("plan_exit_keep_planning");
}

// Covers: handoff from both goal and idle completion drivers must run before returning.
// Owner: interactive turn orchestration (PTY).
#[test]
fn plan_exit_non_composer_drivers() {
    for scenario in ["plan_exit_goal", "plan_exit_idle_completion"] {
        assert_pass(scenario);
    }
}

// Covers: a proposal turn failing after approval must discard intent, not execute.
// Owner: interactive UX (PTY).
#[test]
fn plan_exit_failed_turn() {
    assert_pass("plan_exit_failed_turn");
}

// Covers: an Alt+M press queued during the proposal turn must not discard approval.
// Owner: turn-end permission ordering (PTY).
#[test]
fn plan_exit_queued_cycle() {
    assert_pass("plan_exit_queued_cycle");
}

// Covers: debug startup and the first turn fit the Windows main-thread stack.
// Owner: process startup through the interactive TUI.
#[test]
fn smoke_startup_stream_exit() {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let wrapper = directory.path().join("rho-windows-stack");
        let binary = env!("CARGO_BIN_EXE_rho").replace('\'', "'\\''");
        // Linux normally gives main 8 MiB, hiding overflows of Windows' default
        // 1 MiB reserve. Limit only the exec'd Rho process, not the test runner.
        fs::write(
            &wrapper,
            format!("#!/bin/sh\nulimit -s 1024 || exit\nexec '{binary}' \"$@\"\n"),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
        let runner = ScenarioRunner::new(wrapper)
            .with_artifacts(std::env::temp_dir().join("rho-pty-test-artifacts"));
        let outcome = run_named(&runner, "startup_stream_exit").expect("scenario runner error");
        assert!(
            outcome.passed,
            "Windows main-thread stack budget (1024 KiB): {}",
            outcome.message
        );
    }
    #[cfg(not(target_os = "linux"))]
    assert_pass("startup_stream_exit");
}

// Covers: /new must not discard the system prompt when storage is attached
// before the first turn, including the saved session subsequently resumed.
// Owner: interactive session lifecycle through provider requests.
#[test]
fn new_session_preserves_system_prompt() {
    assert_pass("new_session_system_prompt");
}

// Covers: single-line overlays keep long edits and masked carets visible across
// navigation and resize. Owner: interactive TUI.
#[test]
fn single_line_editor_keeps_insertion_point_visible() {
    assert_pass("line_editor_viewport");
}

// Covers: history and mouse geometry must not re-anchor setup's capped field
// viewport at the full terminal width. Owner: interactive TUI.
#[test]
fn setup_line_editor_keeps_its_capped_viewport() {
    assert_pass("setup_line_editor_viewport");
}

// Covers: native todos remain visible with status markers and overflow in Plan mode.
// Owner: interactive TUI tool lifecycle and checklist card.
#[test]
fn todo_card_shows_checklist_in_plan_mode() {
    assert_pass("todo_card");
}

// Covers: live nested updates, current-item focus, and new-session isolation.
// Owner: interactive task-progress UX.
#[test]
fn todo_progress_stays_current() {
    assert_pass("todo_progress");
}

// Covers: default and remapped permission shortcuts change the session policy
// without saving it, including wrapping and configured Auto.
// Owner: interactive permission shortcut through PTY
#[test]
fn permission_cycle_is_session_only() {
    for scenario in ["permission_cycle_idle", "permission_cycle_remapped"] {
        assert_pass(scenario);
    }
}

// Covers: a home whose config.toml rho 2.x wrote starts, in process and as
// a daemon client, with its display and keybinding settings still applied
// and the original kept as a backup.
// Owner: interactive startup through PTY
#[test]
fn a_2x_config_starts_and_keeps_its_tui_settings() {
    for scenario in ["config_2x_upgrade", "daemon_config_2x_upgrade"] {
        assert_pass(scenario);
    }
}

// Covers: repeated presses queue from the pending mode, not the active mode,
// and cancellation applies the last queued policy before the next idle press.
// Owner: interactive queued permission shortcut through PTY
#[test]
fn permission_cycle_queues_until_turn_end() {
    assert_pass("permission_cycle_queued");
}

// Covers: menu and shortcut share a saved streaming preference during a turn.
// Owner: interactive TUI
#[test]
fn streaming_controls_persist_during_turn() {
    assert_pass("streaming_controls");
}

// Covers: an unfocused terminal gets OSC 9 for an approval and the finished
// turn. Owner: interactive TUI event loop.
#[test]
fn unfocused_terminal_is_notified() {
    assert_pass("turn_notifications");
}

// Covers: Ctrl+R searches prompt history and recalls into the composer.
// Owner: interactive TUI composer.
#[test]
fn ctrl_r_recalls_prompt_from_history_search() {
    assert_pass("prompt_history_search");
}

// Covers: vim editing mode, its mode indicator, and composer undo.
// Owner: interactive TUI composer.
#[test]
fn vim_mode_edits_and_undoes_in_the_composer() {
    assert_pass("composer_vim");
}

// Covers: project and global memories append locally, preserve prior instructions,
// and do not start a model turn; an ordinary prompt still works afterwards.
// Owner: interactive TUI command lifecycle and filesystem effects.
#[test]
fn remember_appends_instructions_without_model_turn() {
    assert_pass("remember_instruction");
}

// Covers: model-scoped prompt switches are visible and invalid files do not
// interrupt the interactive session. Owner: interactive TUI lifecycle.
#[test]
fn model_prompt_switch_and_invalid_file_recovery() {
    for scenario in ["model_prompt_switch", "model_prompt_before_first_prompt"] {
        assert_pass(scenario);
    }
}

// Covers: /init must target the root, /new must reload it without changing the
// current cache early, and /resume of another session must re-read later edits.
// Owner: interactive TUI onboarding and session lifecycle.
#[test]
fn init_writes_project_instructions_and_new_reloads_them() {
    assert_pass("init_command");
}

// Covers: /init must use the live editor at the git root and preserve unrelated
// guidance; /new must load the addition without changing the cached session early.
// Owner: interactive TUI onboarding and session lifecycle.
#[test]
fn init_preserves_existing_root_instructions_and_new_reloads_additions() {
    assert_pass("init_existing");
}

// Covers: with RHO_TUI_DAEMON=1, feedback typed with a plan approval
// reaches the implementation turn the daemon runs.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_plan_exit_approve_carries_feedback() {
    assert_pass("daemon_plan_exit_approve");
}

// Covers: with RHO_TUI_DAEMON=1, /goal and a delegated child's completion
// hand an approved plan to the implementation turn.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_plan_exit_non_composer_drivers() {
    for scenario in [
        "daemon_plan_exit_goal",
        "daemon_plan_exit_delegated_completion",
    ] {
        assert_pass(scenario);
    }
}

// Covers: with RHO_TUI_DAEMON=1, a proposal turn failing after approval
// discards it instead of implementing.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_plan_exit_failed_turn() {
    assert_pass("daemon_plan_exit_failed_turn");
}

// Covers: with RHO_TUI_DAEMON=1, an Alt+M queued during the proposal turn
// does not outlast the approved plan's mode.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_plan_exit_queued_cycle() {
    assert_pass("daemon_plan_exit_queued_cycle");
}

// Covers: with RHO_TUI_DAEMON=1, /init refused in plan mode says why once
// and leaves the composer empty.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_init_refuses_plan_permission_mode() {
    assert_pass("daemon_init_plan");
}

// Covers: with RHO_TUI_DAEMON=1, a first launch walks sign-in and the model
// step and lands in the daemon session.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_first_run_setup_signs_in_and_picks_a_model() {
    assert_pass("daemon_first_run_setup");
}

// Covers: with RHO_TUI_DAEMON=1, a session signed out of its provider says
// so and routes a prompt to the login picker, holding it.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_signed_out_session_routes_a_prompt_to_login() {
    assert_pass("daemon_signed_out_setup_state");
}

// Covers: Plan must reject /init before writing project instructions.
// Owner: interactive TUI permission gating.
#[test]
fn init_refuses_plan_permission_mode() {
    assert_pass("init_plan");
}

// Covers: /new must not strand new child results behind the previous session ID.
// Owner: interactive TUI session lifecycle and automatic completion delivery.
#[test]
fn subagent_completion_after_new() {
    assert_pass("subagent_completion_after_new");
}

// Covers: messages for same-role agents retain their task identity and expandable body.
// Owner: interactive TUI
#[test]
fn agent_messages_keep_task_identity_and_expand_details() {
    assert_pass("agent_messages");
}

// Covers: parent messages must become child attach cards only after application,
// with their bodies and expandable delivery details preserved on reattach.
// Owner: interactive TUI
#[test]
fn parent_messages_appear_in_child_attach() {
    assert_pass("attach_parent_message");
}

// Covers: --prompt must start the first turn without typing Enter.
// Owner: interactive TUI
#[test]
fn smoke_startup_prompt_stream_exit() {
    assert_pass("startup_prompt_stream_exit");
}

// Covers: a shell child that grabs the terminal foreground suspends the TUI.
// Owner: interactive lifecycle and child process isolation.
#[test]
fn shell_keeps_terminal() {
    assert_pass("shell_keeps_terminal");
}

// Covers: unsaved conversations never persist, including after /new and exit.
// Owner: interactive session lifecycle and filesystem effects.
#[test]
fn no_save_session() {
    assert_pass("no_save_session");
}

// Covers: an idle session must not redraw every tick while a startup hydrate
// (the models.dev catalog fetch) is still in flight. Treating "in flight" as
// "ready to apply" made rho repaint ~10x/s until the fetch finished, which
// also kept PTY quiet-window waits from settling under load.
// Owner: interactive TUI event loop
#[test]
fn idle_session_does_not_redraw_while_catalog_fetch_is_in_flight() {
    // A proxy that accepts and never answers holds the fetch open for the test.
    let proxy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
    std::thread::spawn(move || {
        let _held: Vec<_> = proxy.incoming().collect();
    });
    let home = IsolatedHome::new().unwrap();
    let plan = RhoLaunchPlan::matrix(
        PathBuf::from(env!("CARGO_BIN_EXE_rho")),
        &home,
        PtySize {
            rows: 28,
            cols: 100,
        },
    )
    .with_env("HTTPS_PROXY", proxy_url);
    let mut harness = PtyHarness::spawn(&plan).unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(10, "startup"))
        .unwrap();
    // Well under the 5s fetch timeout, so the fetch is still pending throughout.
    harness
        .wait_for_quiet(
            Duration::from_millis(1_000),
            WaitTimeout::secs(3, "idle with catalog fetch in flight"),
        )
        .unwrap();
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

// Covers: /mcp must open and report in-flight servers while connect is still running.
// Owner: interactive TUI
#[test]
fn mcp_connecting_keeps_the_session_inspectable() {
    assert_pass("mcp_connecting");
}

// Covers: a turn held during MCP connect must start on its own once the
// servers settle, without a second Enter.
// Owner: interactive TUI
#[test]
fn mcp_connect_release_starts_the_held_turn() {
    assert_pass("mcp_connect_release");
}

// Covers: esc must return a turn held during MCP connect to the composer.
// Owner: interactive TUI
#[test]
fn mcp_hold_take_back_returns_the_prompt() {
    assert_pass("mcp_hold_take_back");
}

// Covers: with RHO_TUI_DAEMON=1, MCP startup behaves as in process: /mcp
// reports a connecting server, a turn held during connect starts once the
// servers settle, and Esc takes a held turn back.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_mcp_startup_matches_the_in_process_tui() {
    for scenario in [
        "daemon_mcp_connecting",
        "daemon_mcp_connect_release",
        "daemon_mcp_hold_take_back",
    ] {
        assert_pass(scenario);
    }
}

#[test]
fn smoke_cancel_and_resubmit() {
    assert_pass("cancel_and_resubmit");
}

#[test]
fn smoke_type_during_stream() {
    assert_pass("type_during_stream");
}

// Covers: applied steering must appear as a transcript user message
// Owner: interactive TUI
#[test]
fn applied_steering_appears_as_transcript_user_message() {
    assert_pass("steer_appears_in_transcript");
}

// Covers: Alt+Enter and Ctrl+Enter must queue follow-ups during a turn, not steers
// Owner: interactive TUI
#[test]
fn queue_follow_up_during_turn() {
    assert_pass("queue_follow_up_during_turn");
}

// Covers: /compact must keep the composer usable so typed keys are not dropped.
// Owner: interactive TUI
#[test]
fn type_during_compact() {
    assert_pass("type_during_compact");
}

// Covers: Enter during /compact queues a follow-up that starts after a failed compact.
// Owner: interactive TUI
#[test]
fn submit_during_compact() {
    assert_pass("submit_during_compact");
}

// Covers: idle usage calibration must survive into the next prompt's visible
// auto-compaction, including fresh provider usage after a real session resume.
// Owner: interactive TUI lifecycle, not SDK estimation policy.
#[test]
fn calibrated_context_auto_compacts_after_idle_and_resume() {
    assert_pass("calibrated_context_auto_compact");
}

#[test]
fn smoke_resize_during_stream() {
    assert_pass("resize_during_stream");
}

#[test]
fn smoke_scroll_during_stream() {
    assert_pass("scroll_during_stream");
}

#[test]
fn smoke_terminal_restoration() {
    assert_pass("terminal_restoration");
}

// Covers: an interactive workflow must use its own resizable screen, show typed parallel and
// exclusive node states, and restore the terminal after a durable completion.
// Owner: interactive TUI
#[test]
fn workflow_run_uses_separate_terminal_mode() {
    assert_pass("workflow_run_interactive");
}

// Covers: terminal cancellation must reach durable resumable state, and resume must not rerun a
// node that already succeeded.
// Owner: interactive TUI
#[test]
fn workflow_cancel_then_resume_preserves_completed_nodes() {
    assert_pass("workflow_cancel_resume");
}

// Covers: retrying a failed hub launch reuses the ready run; watch never starts it.
// Owner: interactive workflow hub recovery.
// Requires the verified-handle workflow command adapter; other platforms fail closed.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn workflow_hub_retries_planned_run() {
    assert_pass("workflow_hub_planned_retry");
}

// Covers: pasting an absolute document path must attach extracted text instead of parsing it as a
// slash command.
// Owner: interactive TUI
#[test]
fn absolute_document_path_paste_attaches_and_submits_text() {
    assert_pass("document_attachment");
}

#[test]
fn edit_diff_streams_and_survives_cancellation() {
    assert_pass("edit_diff");
}

// Covers: write content updates before the incomplete call can execute.
// Owner: interactive TUI
#[test]
fn write_input_stream() {
    assert_pass("write_input_stream");
}

// Covers: enabling fast mode must update the persistent model indicator without a restart.
// Owner: interactive TUI
#[test]
fn fast_mode_appears_beside_the_active_model() {
    let home = IsolatedHome::new().unwrap();
    fs::write(
        &home.config_path,
        r#"web_search.mode = "off"

[model]
provider = "openai-codex"
model = "gpt-5.5"
auth = "codex"

[behavior]
credential_store = "file"
"#,
    )
    .unwrap();
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 28,
            cols: 100,
        },
    );
    let mut harness = PtyHarness::spawn_named(&plan, "fast_mode_statusline").unwrap();
    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();

    harness.submit_text("/fast on").unwrap();
    harness
        .wait_for_text(
            "gpt-5.5 (fast)",
            WaitTimeout::secs(10, "fast mode statusline"),
        )
        .unwrap();
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

#[test]
fn first_launch_walks_the_full_screen_setup() {
    assert_pass("first_run_setup");
}

#[test]
fn first_launch_setup_survives_a_decision_host_sign_in() {
    assert_pass("first_run_decision_host");
}

#[test]
fn first_launch_setup_can_be_skipped_into_a_session() {
    assert_pass("first_run_setup_skipped");
}

#[test]
fn signed_out_session_offers_login_from_header_statusline_and_prompt() {
    assert_pass("signed_out_setup_state");
}

// Covers: /info must open a single-pane overlay, copy with c and a drag,
// keep a narrow field readable, and Esc must dismiss it instead of leaving
// a transcript block.
// Owner: interactive TUI
#[test]
fn runtime_info_reflows_after_narrow_resize() {
    assert_pass("runtime_info");
}

// Covers: /context must attribute a tool turn's output to that tool, keep its
// right-aligned columns inside the panel, report free space against the window,
// stay readable after a narrow resize, and dismiss without a transcript block.
// Owner: interactive TUI
#[test]
fn context_overlay_attributes_sources_and_dismisses() {
    assert_pass("context_overlay");
}

// Covers: /limits must open a single-pane overlay and Esc must return to the session.
// Owner: interactive TUI
#[test]
fn limits_overlay_opens_and_dismisses() {
    assert_pass("limits_overlay");
}

// Covers: /doctor must open a single-pane dashboard immediately and Esc must
// return to the session.
// Owner: interactive TUI
#[test]
fn doctor_overlay_opens_and_dismisses() {
    assert_pass("doctor_overlay");
}

// Covers: the /doctor MCP row reports the session's servers in process and,
// with RHO_TUI_DAEMON=1, the daemon session's servers rather than the
// client's, with the same row.
// Owner: interactive TUI (#1685)
#[test]
fn doctor_reports_the_session_mcp_servers() {
    assert_pass("doctor_mcp");
    #[cfg(unix)]
    assert_pass("daemon_doctor_mcp");
}

// Covers: /hooks must open a single-pane overlay with the spawn contract, and
// Esc must dismiss it instead of leaving a transcript notice.
// Owner: interactive TUI
#[test]
fn hooks_overlay_shows_contract_and_dismisses() {
    assert_pass("hooks_contract");
}

// Covers: fragile interactive surfaces from issue #711. One test per scenario
// so the harness runs them in parallel.
// Owner: interactive TUI
#[test]
fn markdown_headings() {
    assert_pass("markdown_headings");
}

// Covers: exact-width fenced-code rendering must retain the combining accent.
// Owner: interactive TUI.
#[test]
fn code_wrap_grapheme() {
    assert_pass("code_wrap_grapheme");
}

#[test]
fn streaming_markdown_stability() {
    assert_pass("streaming_markdown_stability");
}

#[test]
fn side_btw() {
    assert_pass("side_btw");
}

#[test]
fn side_during_turn() {
    assert_pass("side_during_turn");
}

#[test]
fn spinner_activity_anchor() {
    assert_pass("spinner_activity_anchor");
}

#[test]
fn spinner_activity_jump_rail() {
    assert_pass("spinner_activity_jump_rail");
}

#[test]
fn file_path_autocomplete() {
    assert_pass("file_path_autocomplete");
}

// Covers: tab-completing a slash command must not turn a plain Enter into the
// first argument choice; the bare command runs as typed.
// Owner: interactive TUI
#[test]
fn tab_completion_keeps_enter_on_the_bare_command() {
    assert_pass("tab_complete_enter_bare_command");
}

// Covers: a saved advisor mode with no model warns instead of claiming to run.
// Owner: interactive TUI
#[test]
fn advisor_missing_model() {
    assert_pass("advisor_missing_model");
}

// Covers: the advisor's answer comes back to the executor without ending the
// turn on failure.
// Owner: interactive TUI
#[test]
fn advisor_review() {
    assert_pass("advisor_review");
}

#[test]
fn mermaid_flowchart_survives_narrow_and_restored_panes() {
    assert_pass("mermaid_flowchart_resize");
}

// Covers: a bare /skill command loads the skill before the model responds, and
// the loaded SKILL.md stays behind a collapsed card until Ctrl+O expands it;
// an unknown skill is reported as an unknown command.
// Owner: interactive TUI
#[test]
fn bare_skill_command_starts_a_model_turn() {
    bare_skill_command(SessionHost::InProcess);
}

// Covers: the same with RHO_TUI_DAEMON=1, where the daemon loads the skill.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_bare_skill_command_starts_a_model_turn() {
    bare_skill_command(SessionHost::Daemon);
}

fn bare_skill_command(host: SessionHost) {
    // The fixture model echoes only the final instruction line, so this line
    // can appear on screen only inside the expanded skill card.
    const EXPANDED_ONLY: &str = "Skill detail shown only in the expanded card.";
    let home = IsolatedHome::new().unwrap();
    let skill_dir = home.workspace.join(".agents/skills/test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: test-skill\ndescription: Test skill invocation\ndisable-model-invocation: true\n---\n{EXPANDED_ONLY}\n\nFollow the unique bare skill instruction.\n"),
    )
    .unwrap();
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    // Tall enough that the expanded card and the reply both fit on screen.
    let plan = host.launch(RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 40,
            cols: 100,
        },
    ));
    let mut harness = PtyHarness::spawn_named(&plan, "bare_skill_command").unwrap();

    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();
    harness.submit_text("/skill:missing-skill").unwrap();
    harness
        .wait_for_text(
            "unknown command '/skill:missing-skill'",
            WaitTimeout::secs(10, "unknown skill"),
        )
        .unwrap();
    harness.submit_text("/skill:test-skill").unwrap();
    harness
        .wait_for_text(
            "skill command loaded before model response: Follow the unique bare skill instruction.",
            WaitTimeout::secs(20, "skill response"),
        )
        .unwrap();
    let screen = harness.screen().contents();
    assert!(
        screen.contains("skill(test-skill)") && !screen.contains(EXPANDED_ONLY),
        "skill card should be collapsed after loading:\n{screen}"
    );

    harness.inject_key(&Key::Ctrl('o')).unwrap();
    harness
        .wait_for_text(EXPANDED_ONLY, WaitTimeout::secs(10, "expanded skill card"))
        .unwrap();
    harness.inject_key(&Key::Ctrl('o')).unwrap();
    harness
        .wait_for_text_gone(EXPANDED_ONLY, WaitTimeout::secs(10, "collapsed skill card"))
        .unwrap();

    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

// Covers: `$` completes a skill mention in the composer, and a prompt that
// mentions two skills loads both as one host batch before the model responds.
// Owner: interactive TUI
#[test]
fn skill_mentions_complete_and_load_in_one_batch() {
    let home = IsolatedHome::new().unwrap();
    for name in ["alpha", "beta"] {
        let skill_dir = home.workspace.join(".agents/skills").join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: The {name} mention skill\n---\nFollow {name}.\n"
            ),
        )
        .unwrap();
    }
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 40,
            cols: 100,
        },
    );
    let mut harness = PtyHarness::spawn_named(&plan, "skill_mentions").unwrap();

    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();
    harness.type_text("use $al").unwrap();
    harness
        .wait_for_text(
            "The alpha mention skill",
            WaitTimeout::secs(10, "skill mention palette"),
        )
        .unwrap();
    harness.inject_key(&Key::Tab).unwrap();
    harness.type_text("and $beta").unwrap();
    harness
        .wait_for_text(
            "use $alpha and $beta",
            WaitTimeout::secs(10, "completed mention"),
        )
        .unwrap();
    // Typed keys can arrive as one burst on a loaded runner; outlast the
    // paste-Enter suppression so Enter submits instead of adding a newline.
    harness.settle_plain_text_input();
    harness.inject_key(&Key::Enter).unwrap();
    harness
        .wait_for_text(
            "skill command loaded before model response: Follow alpha.",
            WaitTimeout::secs(20, "skill response"),
        )
        .unwrap();
    let screen = harness.screen().contents();
    assert!(
        screen.contains("skill(alpha)") && screen.contains("skill(beta)"),
        "both mentioned skills should load before the reply:\n{screen}"
    );

    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

// Covers: /skill says why it cannot run for an agent without the skill tool.
// Owner: interactive TUI
#[test]
fn skill_command_reports_when_skill_tool_is_disabled() {
    disabled_skill_command(SessionHost::InProcess);
}

// Covers: the same with RHO_TUI_DAEMON=1, where the daemon refuses it.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_skill_command_reports_when_skill_tool_is_disabled() {
    disabled_skill_command(SessionHost::Daemon);
}

fn disabled_skill_command(host: SessionHost) {
    let home = IsolatedHome::new().unwrap();
    let skill_dir = home.workspace.join(".agents/skills/test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: test-skill\ndescription: Test skill invocation\n---\nFollow the skill.\n",
    )
    .unwrap();
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let plan = host.launch(
        RhoLaunchPlan::matrix(
            binary,
            &home,
            PtySize {
                rows: 28,
                cols: 100,
            },
        )
        .with_arg("--no-tools"),
    );
    let mut harness = PtyHarness::spawn_named(&plan, "disabled_skill_command").unwrap();

    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();
    harness.submit_text("/skill:test-skill").unwrap();
    harness
        .wait_for_text(
            "skill commands are unavailable because the active agent has no skill tool",
            WaitTimeout::secs(5, "skill unavailable"),
        )
        .unwrap();

    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);
}

#[test]
fn goal_waits_for_subagents_before_evaluation() {
    assert_pass("goal_waits_for_subagents");
}

#[test]
fn goal_answers_background_agent_questionnaire_while_waiting() {
    assert_pass("goal_questionnaire");
}

#[test]
fn goal_waits_for_subagents_before_retrying() {
    assert_pass("goal_waits_for_subagents_during_retry");
}

// Covers: full prompt expansion survives streaming and launch while collapsed
// receipts and finished errors remain visible above the retained prompt.
// Owner: interactive UX; the scenario holds each intermediate phase explicitly.
#[test]
fn agent_prompt_streaming_preserves_expansion_and_collapsed_results() {
    assert_pass("agent_prompt_streaming");
}

#[test]
fn background_agent_completion_is_delivered_after_turn_end() {
    assert_pass("background_agent_auto_delivery");
}

#[test]
fn subagent_rail_mouse_activation_uses_release_and_survives_refresh() {
    assert_pass("subagent_rail_mouse");
}

// Covers: /attach must open a full-screen picker of running subagents by role.
// Owner: interactive TUI
#[test]
fn attach_opens_a_picker_of_running_subagents() {
    assert_pass("attach_picker");
}

// Covers: /attach must swap the main TUI into a live in-place attach view
// and keep typed keys out of the hidden composer.
// Owner: interactive TUI
#[test]
fn attach_opens_an_in_place_view_and_returns_to_the_composer() {
    assert_pass("attach_view_from_command");
}

// Covers: Tab must cycle between running subagents without leaving attach.
// Owner: interactive TUI
#[test]
fn attach_view_cycles_between_running_subagents() {
    assert_pass("attach_view_cycle");
}

// Covers: a parent approval arriving mid-attach must badge, not yank, and
// must not resolve on y until the user returns.
// Owner: interactive TUI
#[test]
fn attach_view_badges_a_parent_approval_until_return() {
    assert_pass("attach_view_parent_approval");
}

// Covers: a Ctrl-C armed mid-turn (leaving attach) still quits after the turn ends, and restores the terminal.
// Owner: interactive TUI
#[test]
fn attach_view_quit_restores_the_terminal() {
    assert_pass("attach_view_quit_restores");
}

// Covers: a started process must stay on the activity rail after the turn ends.
// Owner: interactive TUI
#[test]
fn process_rail_stays_visible_after_turn_ends() {
    assert_pass("process_rail");
}

// Covers: clicking a process rail row opens peek and q returns to the composer.
// Owner: interactive TUI
#[test]
fn process_rail_click_opens_peek_and_q_returns() {
    assert_pass("process_rail_peek");
}

// Covers: hover lifts and drops a questionnaire choice, a click selects it
// without submitting, and a double click confirms it like Enter.
// Owner: interactive TUI
#[test]
fn questionnaire_choice_hover_click_and_double_click_submit() {
    assert_pass("questionnaire_click");
}

// Covers: a single click never resolves an approval; a double click allows it
// and the turn resumes.
// Owner: interactive TUI
#[test]
fn approval_double_click_allows_and_resumes_turn() {
    assert_pass("approval_click");
}

// Covers: a click focuses an inline choice option without resolving it, and a
// double click confirms it like Enter.
// Owner: interactive TUI
#[test]
fn inline_choice_click_focuses_and_double_click_confirms() {
    assert_pass("inline_choice_click");
}

// Covers: clicking and wheeling the `/` palette move its highlight, and a
// double click completes the row like Tab without running it.
// Owner: interactive TUI
#[test]
fn slash_palette_click_highlights_and_double_click_completes() {
    assert_pass("slash_palette_click");
}

// Covers: a double click on an `@` palette row inserts that path.
// Owner: interactive TUI
#[test]
fn file_palette_double_click_inserts_path() {
    assert_pass("file_palette_click");
}

// Covers: a click selects an inline picker row without submitting; a double click submits it.
// Owner: interactive TUI
#[test]
fn inline_picker_click_selects_and_double_click_submits() {
    assert_pass("inline_picker_click");
}

// Covers: a click selects an overlay picker row; a double click submits it.
// Owner: interactive TUI
#[test]
fn overlay_picker_double_click_submits() {
    assert_pass("overlay_picker_double_click");
}

// Covers: panel scrollbar hover and drag scroll the body, and drag-select copies panel text.
// Owner: interactive TUI
#[test]
fn panel_scrollbar_drag_and_selection_copy() {
    assert_pass("panel_pointer");
}

// Covers: the side chat prompt edits like the main composer (multi-line,
// word keys, pointer, recall, collapsed paste) and stays pinned below a
// scrolled transcript.
// Owner: interactive TUI
#[test]
fn side_chat_composer_matches_main_composer() {
    assert_pass("side_composer");
}

// Covers: drag-select copies side chat text, and the side chat scrollbar drags.
// Owner: interactive TUI
#[test]
fn side_chat_selection_copy_and_scrollbar_drag() {
    assert_pass("side_chat_pointer");
}

// Covers: provider and model hover and click as one target that opens the
// model picker; a reasoning click cycles the effort; a model without
// reasoning paints no reasoning target.
// Owner: interactive TUI
#[test]
fn statusline_model_and_reasoning_clicks() {
    assert_pass("statusline_model_click");
    assert_pass("statusline_no_reasoning_click");
}

// Covers: hovering a composer attachment offers removal, and clicking removes it.
// Owner: interactive TUI
#[test]
fn composer_attachment_click_removes_it() {
    assert_pass("attachment_click_remove");
}

// Covers: the drag highlight updates before release, and clicking into a
// recalled prompt keeps the stashed draft for Down. Owner: interactive TUI
#[test]
fn text_selection_highlight_follows_drag_before_release() {
    assert_pass("text_selection_drag");
}

// Covers: hovering a collapsed tool card lifts its text and reverts on exit,
// and click-to-expand still works through the shared hit-test path.
// Owner: interactive TUI
#[test]
fn tool_card_hover_lifts_text_and_expands_on_click() {
    assert_pass("tool_card_hover");
}

#[test]
fn screen_text_selection_copies_composer_text() {
    assert_pass("screen_text_selection");
}

#[test]
fn background_agent_questionnaire_is_answered_in_parent_tui() {
    assert_pass("background_agent_questionnaire");
}

// Covers: a subagent that asks after its parent's turn ended still reaches
// the person, and its answer the subagent, in process and on a daemon
// session, with no parent turn in between.
// Owner: interactive delegated-questionnaire routing (PTY)
#[test]
fn late_background_agent_questionnaire_is_answered_while_idle() {
    for scenario in [
        "late_background_agent_questionnaire",
        "daemon_late_background_agent_questionnaire",
    ] {
        assert_pass(scenario);
    }
}

// Covers: completed reasoning can be revealed and hidden retroactively, in zen
// mode too. Owner: interactive TUI
#[test]
fn reasoning_output_toggles_retroactively() {
    assert_pass("reasoning_output_retroactive");
}

// Covers: workflow tools run without a second form and keep live progress.
// Owner: interactive TUI
// Requires the verified-handle workflow command adapter; other platforms fail closed.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn workflow_tools_run_in_the_background() {
    assert_pass("workflow_background");
}

// Covers: questionnaire timeouts submit fallback answers with distinct
// provenance, and any interaction pauses the fallback. Owner: interactive TUI
#[test]
fn questionnaire_timeout_falls_back_unless_interrupted() {
    for scenario in ["questionnaire_timeout", "questionnaire_timeout_pause"] {
        assert_pass(scenario);
    }
}

// Covers: a long supervised process approval can be inspected and cancelled.
// Owner: interactive approval UX
#[test]
fn supervised_approval_can_be_inspected_and_cancelled() {
    assert_pass("supervised_approval");
}

// Covers: codemode cards show script generation past the collapsed budget and
// keep streaming after the argument buffer passes 4096 bytes.
// Owner: interactive TUI
#[test]
fn codemode_cards_stream_script_generation() {
    for scenario in ["codemode_card", "codemode_large_script_stream"] {
        assert_pass(scenario);
    }
}

// Covers: steering queued during a tool can be inspected and retracted.
// Owner: interactive turn orchestration (PTY)
#[test]
fn steering_can_be_retracted_during_a_tool() {
    assert_pass("retract_steering_during_tool");
}

// Covers: /fast suggestions follow model switches. Owner: interactive TUI
#[test]
fn fast_command_suggestions_follow_model_switches() {
    assert_pass("fast_command_discovery");
}

// Covers: Tab completes one nested path component at a time in the inline
// shell composer. Owner: interactive TUI
#[test]
fn inline_shell_tab_completes_nested_paths() {
    assert_pass("shell_tab_completion");
}

// Covers: the sessions tool's search and read cards collapse and expand over
// real session evidence. Owner: interactive TUI
#[test]
fn sessions_tool_cards_show_real_evidence() {
    assert_pass("sessions_tool_cards");
}

// Covers: reasoning levels survive compatible pinned-model cycles and normalize
// for incompatible ones, idle or queued during a turn. Owner: interactive TUI
#[test]
fn pinned_model_cycles_normalize_reasoning() {
    for scenario in ["cycle_reasoning_idle", "cycle_reasoning_queued"] {
        assert_pass(scenario);
    }
}

// Covers: dual-grant OAuth offers browser or device-code even with a browser.
// Owner: interactive login UX
#[test]
fn oauth_login_offers_a_flow_choice() {
    assert_pass("login_oauth_flow_choice");
}

// Covers: quiet subagent notices queue and coalesce, and a notice at the
// completion checkpoint does not buy another provider request.
// Owner: interactive turn orchestration (PTY)
#[test]
fn quiet_subagent_notices_coalesce_without_extra_requests() {
    for scenario in ["quiet_subagent_notices", "quiet_subagent_running_notices"] {
        assert_pass(scenario);
    }
}

// Covers: a failed parent action under /goal retries without waiting for the
// blocked child. Owner: interactive turn orchestration (PTY)
#[test]
fn goal_retries_a_failed_action_without_waiting_for_children() {
    assert_pass("goal_subagent_action_retry");
}

// Covers: boundary notices keep a streaming message intact, and a pending
// background failure is folded in before the parent turn completes.
// Owner: interactive turn orchestration (PTY)
#[test]
fn boundary_notices_arrive_without_breaking_the_turn() {
    for scenario in ["streaming_boundary_notice", "boundary_notifications"] {
        assert_pass(scenario);
    }
}

// Covers: pending input stays below the activity rail while a process is live.
// Owner: interactive TUI
#[test]
fn pending_input_stays_below_the_activity_rail() {
    assert_pass("pending_input_below_activity");
}

// Covers: with RHO_TUI_DAEMON=1, resuming a saved session follows it in the
// daemon and shows its prompt, tool card, and answer as the in-process TUI did.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_resume_shows_the_saved_conversation() {
    assert_pass("daemon_resume");
}

// Covers: quit during a daemon turn leaves it running and prints a usable
// resume hint for a new saved session, but not an untouched session.
// Owner: interactive daemon session lifecycle
#[test]
fn daemon_detach_preserves_the_turn_and_resume_hint() {
    assert_pass("daemon_detach");
}

// Covers: the daemon-hosted TUI reconnects across a daemon restarting in
// place, keeps its conversation without an error, and its next prompt runs
// in the restarted daemon (#1713).
// Owner: interactive daemon session lifecycle
#[test]
fn daemon_restart_keeps_the_tui_following_its_session() {
    assert_pass("daemon_restart");
}

// Covers: a goal set from the TUI lives in the daemon session: a TUI that
// reopens the session after quitting sees it blocked and resumes it.
// Owner: daemon goal controller and the TUI as its client (#1667)
#[test]
fn daemon_goal_outlives_the_tui_that_set_it() {
    assert_pass("daemon_goal");
}

// Covers: with RHO_TUI_DAEMON=1, a prompt sent from the TUI runs in the
// daemon: its user row shows once, its tool card and reply stream in, and the
// composer is idle once the session rests.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_prompt_streams_the_turn_and_returns_to_idle() {
    assert_pass("daemon_prompt");
}

// Covers: with RHO_TUI_DAEMON=1, Esc cancels the daemon's running turn, the
// transcript says the model was interrupted, and the next prompt runs.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_cancel_interrupts_the_turn_and_returns_to_idle() {
    assert_pass("daemon_cancel");
}

// Covers: with RHO_TUI_DAEMON=1, the statusline's cost follows a response
// while the daemon streams it, and the finished turn counts it once.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_stream_cost_follows_the_response() {
    assert_pass("daemon_stream_cost");
}

// Covers: with RHO_TUI_DAEMON=1, a streamed call's small tail shows once
// the model's stream pauses, though its card is sent only as it grows.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_streamed_call_shows_its_tail_when_the_stream_pauses() {
    assert_pass("daemon_codemode_large_script_stream");
}

// Covers: with RHO_TUI_DAEMON=1, `output_streaming` holds and reveals the
// daemon turn's text as it does an in-process turn's.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_streaming_controls_hold_and_reveal_output() {
    assert_pass("daemon_streaming_controls");
}

// Covers: a rate-limited provider request shows the retry's status while
// the retried request runs, then its reply.
// Owner: interactive TUI
#[test]
fn provider_retry_shows_its_status_then_the_reply() {
    assert_pass("provider_retry");
}

// Covers: with RHO_TUI_DAEMON=1, the daemon turn's provider retry shows the
// same status as an in-process turn's.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_provider_retry_shows_its_status_then_the_reply() {
    assert_pass("daemon_provider_retry");
}

// Covers: the provider's own web search shows as a finished card with its
// query.
// Owner: interactive TUI
#[test]
fn web_search_activity_shows_a_finished_card() {
    assert_pass("web_search_activity");
}

// Covers: with RHO_TUI_DAEMON=1, the daemon turn's provider web search
// shows the same finished card as an in-process turn's.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_web_search_activity_shows_a_finished_card() {
    assert_pass("daemon_web_search_activity");
}

// Covers: a turn the provider failed with a diagnostic shows the
// diagnostic.
// Owner: interactive TUI
#[test]
fn provider_diagnostic_shows_on_a_failed_turn() {
    assert_pass("provider_diagnostic");
}

// Covers: with RHO_TUI_DAEMON=1, the daemon's failed turn shows the
// provider's diagnostic as an in-process turn does.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_provider_diagnostic_shows_on_a_failed_turn() {
    assert_pass("daemon_provider_diagnostic");
}

// Covers: with RHO_TUI_DAEMON=1, deleting a session the TUI just left,
// which the daemon still holds open, goes through the daemon.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_tui_deletes_a_session_the_daemon_holds() {
    assert_pass("daemon_delete_left_session");
}

// Covers: `/agents create` loads the creator skill, and an agent without the
// creator's tools is told which it lacks, in process and with
// RHO_TUI_DAEMON=1, where the daemon runs the creator.
// Owner: interactive TUI agent creator
#[test]
fn agent_creator_starts_or_names_missing_tools() {
    for scenario in [
        "create_agent_command",
        "create_agent_missing_tools",
        "daemon_create_agent_command",
        "daemon_create_agent_missing_tools",
    ] {
        assert_pass(scenario);
    }
}

// Covers: with RHO_TUI_DAEMON=1, the palette completes a command's
// argument, `/fast` follows the session's model, `/rewind` says when
// rewind is off, and a permission change mid-turn says it is queued.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_commands_match_the_in_process_palette_and_replies() {
    for scenario in [
        "daemon_tab_complete_enter_bare_command",
        "daemon_fast_command_discovery",
        "daemon_fast_off_without_fast_mode",
        "daemon_workspace_rewind_off",
        "daemon_permission_cycle_queued",
    ] {
        assert_pass(scenario);
    }
}

// Covers: with RHO_TUI_DAEMON=1, `/model`, the pin cycle, the model
// picker and the statusline model and reasoning clicks behave as in
// process: the pinned and all views, pinning from the picker, the prompt
// file a switch lands on, a refused switch, and the reasoning level a cycle
// keeps or rounds, idle or queued behind a turn.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_model_switches_match_the_in_process_tui() {
    for scenario in [
        "daemon_open_model_picker",
        "daemon_cycle_and_pinned_model_picker",
        "daemon_statusline_model_click",
        "daemon_statusline_no_reasoning_click",
        "daemon_model_prompt_switch",
        "daemon_model_prompt_before_first_prompt",
        "daemon_cycle_reasoning_idle",
        "daemon_cycle_reasoning_queued",
    ] {
        assert_pass(scenario);
    }
}

// Covers: with RHO_TUI_DAEMON=1 in supervised mode, a daemon turn's write
// opens the TUI's approval prompt, Allow once answers the daemon, and the
// turn completes with the write done.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_approval_allows_the_turns_write() {
    assert_pass("daemon_approval");
}

// Covers: with RHO_TUI_DAEMON=1, Enter on the approval prompt's default
// denies the daemon turn's write, and the turn completes with it refused.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_approval_denies_the_turns_write() {
    assert_pass("daemon_approval_deny");
}

// Covers: daemon permission changes affect tools and mid-turn configure does
// not block cancellation. Owner: interactive TUI daemon settings.
#[test]
fn daemon_permission_changes_do_not_block_the_turn() {
    assert_pass("daemon_permission_mode");
}

// Covers: in process, `/config` and `/advisor` say they need the daemon.
// Owner: interactive TUI settings commands in process.
#[test]
fn settings_commands_need_the_daemon_in_process() {
    assert_pass("settings_need_the_daemon");
}

// Covers: `/config` saves through the daemon: a client setting takes effect
// live, vim mode turns on and back off from the default row, an agent setting
// shows its saved value once confirmed and a refused one says why, and
// lowering the prompt history limit asks first.
// Owner: interactive TUI settings screen as a daemon client.
#[test]
fn daemon_config_changes_settings_through_the_daemon() {
    assert_pass("daemon_config");
}

// Covers: `/advisor on` without a model asks for one, Space filters its page,
// the session's next turn takes the advisor, and `/advisor off` turns it off.
// Owner: interactive TUI `/advisor` as a daemon client.
#[test]
fn daemon_advisor_reaches_the_next_turn() {
    assert_pass("daemon_advisor");
}

// Covers: hidden header hints come back live from the daemon `/config`.
// Owner: interactive TUI header as a daemon client.
#[test]
fn header_hints_toggle_live() {
    assert_pass("header_hints_toggle");
}

// Covers: `/remember`, `/title`, and `!` output land in the daemon's
// session: a TUI reopening it replays the added rows.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_session_takes_context_added_without_a_turn() {
    assert_pass("daemon_session_context");
}

// Covers: `/add-dir` widens the daemon's session, which a reopening TUI
// still lists, and `/context` and `/hooks` show the daemon's reports.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_session_adds_a_directory_and_reports_on_itself() {
    assert_pass("daemon_session_reports");
}

// Covers: `/login` has the daemon check and save an API key, and `/logout`
// has it delete the credential.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_saves_and_deletes_a_key_for_the_tui() {
    assert_pass("daemon_login");
}

// Covers: the workflow hub starts its run in the daemon, where it keeps
// going after the TUI quits and shows on the rail of the next one, and
// `/codemode` saves the mode through the daemon.
// Owner: interactive TUI as a daemon client (#1667)
// Requires the verified-handle workflow command adapter; other platforms fail closed.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn daemon_runs_the_workflow_the_hub_starts() {
    assert_pass("daemon_workflow");
}

// Covers: with RHO_TUI_DAEMON=1, a turn ends while the workflow its tools
// started runs on, and the daemon delivers the result to the idle session.
// Owner: interactive TUI as a daemon client (#1667)
// Requires the verified-handle workflow command adapter; other platforms fail closed.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn daemon_workflow_tools_run_in_the_background() {
    assert_pass("daemon_workflow_background");
}

// Covers: with RHO_TUI_DAEMON=1, a failed hub launch reports its ready run
// to the model while idle, and Retry starts that run under the same id.
// Owner: interactive TUI as a daemon client (#1667)
// Requires the verified-handle workflow command adapter; other platforms fail closed.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn daemon_workflow_hub_retries_planned_run() {
    assert_pass("daemon_workflow_hub_planned_retry");
}

// Covers: a daemon session's turn is checkpointed, and `/rewind` has the
// daemon put its file and the conversation back.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_rewinds_a_turns_file_and_conversation() {
    assert_pass("daemon_rewind");
}

// Covers: `/tree` in a daemon session restores an earlier turn, the daemon
// continues from it, and a reopened session shows that branch.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_restores_an_earlier_turn_from_the_tree() {
    assert_pass("daemon_tree");
}

// Covers: `/new`, `/resume`, and the `--resume` picker switch the daemon
// session the TUI follows; a launch flag's setting carries into `/new`.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_tui_switches_the_session_it_follows() {
    assert_pass("daemon_switch");
}

// Covers: with RHO_TUI_DAEMON=1, a turn that starts a subagent ends while
// the subagent runs, its card and messages read as in process, and its rail
// row opens the attach view.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_turn_ends_while_its_subagents_run() {
    for scenario in [
        "daemon_agent_prompt_streaming",
        "daemon_agent_messages",
        "daemon_subagent_rail_mouse",
    ] {
        assert_pass(scenario);
    }
}

// Covers: with RHO_TUI_DAEMON=1, the daemon gives the idle session a
// subagent's result in a turn of its own, also after /new, and a subagent's
// question outlives the reply of the turn that started it.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_delivers_subagent_results_to_the_idle_session() {
    for scenario in [
        "daemon_background_agent_auto_delivery",
        "daemon_background_agent_questionnaire",
        "daemon_subagent_completion_after_new",
    ] {
        assert_pass(scenario);
    }
}

// Covers: with RHO_TUI_DAEMON=1, a subagent's informational notices start
// no turn and buy no provider request, its action request does, and a
// delivered notice leaves the streamed reply before it whole.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_quiet_subagent_notices_wait_for_a_turn() {
    for scenario in [
        "daemon_quiet_subagent_notices",
        "daemon_quiet_subagent_running_notices",
        "daemon_streaming_boundary_notice",
    ] {
        assert_pass(scenario);
    }
}

// Covers: with RHO_TUI_DAEMON=1, /attach lists a daemon session's running
// subagents, opens and cycles the in-place view, and shows a parent's
// message there once applied.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_attach_opens_a_daemon_sessions_subagents() {
    for scenario in [
        "daemon_attach_picker",
        "daemon_attach_view_from_command",
        "daemon_attach_view_cycle",
        "daemon_attach_parent_message",
    ] {
        assert_pass(scenario);
    }
}

// Covers: with RHO_TUI_DAEMON=1, the attach view badges an approval of the
// parent's running turn, and Ctrl-C leaves it mid-turn and quits after.
// Owner: interactive TUI as a daemon client (#1667)
#[test]
fn daemon_attach_view_follows_the_parents_turn() {
    for scenario in [
        "daemon_attach_view_parent_approval",
        "daemon_attach_view_quit_restores",
    ] {
        assert_pass(scenario);
    }
}

// Covers: MCP inventory belongs to the daemon session even when the new client's
// config differs, and remains inspectable behind a queued mid-turn configure.
// Owner: interactive TUI as a daemon client.
#[test]
fn daemon_mcp_inventory_uses_the_session_without_waiting_for_the_turn() {
    assert_pass("daemon_mcp_inventory");
}
