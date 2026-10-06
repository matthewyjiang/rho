//! End-to-end Rho TUI scenarios driven through a PTY.
//!
//! These tests require a Unix PTY and a debug-built `rho` binary with the
//! fixture matrix (`RHO_TUI_TEST_MODE=matrix`).

#![cfg(unix)]

#[path = "support/tui_pty_attach.rs"]
mod attach;
#[path = "support/claude_e2e.rs"]
mod claude_e2e;
#[path = "support/tui_pty_claude_runtime.rs"]
mod claude_runtime;
#[path = "support/composer_unicode.rs"]
mod composer_unicode;
#[path = "support/tui_pty_login.rs"]
mod login;

use std::{fs, path::PathBuf, time::Duration};

use rho_tui_pty::{
    run_named, IsolatedHome, Key, PtyHarness, PtySize, RhoLaunchPlan, ScenarioRunner, WaitTimeout,
};

fn runner() -> ScenarioRunner {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_rho"));
    let artifacts = std::env::temp_dir().join("rho-pty-test-artifacts");
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

// Covers: default and remapped permission shortcuts change the session policy
// without saving it, including wrapping and configured Auto.
// Owner: interactive permission shortcut through PTY
#[test]
fn permission_cycle_is_session_only() {
    for scenario in ["permission_cycle_idle", "permission_cycle_remapped"] {
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
    assert_pass("model_prompt_switch");
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

// Covers: first session chrome must paint without waiting on MCP, catalog, or keyring tails.
// Owner: interactive TUI
#[test]
fn startup_first_frame_paints_session_chrome() {
    assert_pass("startup_first_frame");
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

// Covers: desktop grants are explicit and can be revoked while a turn runs;
// plan mode cannot grant desktop input authority.
// Owner: interactive TUI
#[test]
fn computer_use_authorization_and_revocation() {
    assert_pass("computer_use");
}

#[test]
fn computer_setup_grants_access() {
    assert_pass("computer_setup");
}

#[test]
fn computer_setup_cancel_grants_nothing() {
    assert_pass("computer_setup_cancel");
}

#[test]
fn computer_plan_mode_is_denied() {
    assert_pass("computer_plan_denied");
}

// Covers: real restarts restore consent, restricted startup cannot use it, and
// during-turn off persists revocation for the next process.
// Owner: interactive TUI and machine-local preference lifecycle.
#[test]
fn computer_preference_survives_restart_and_respects_revocation() {
    assert_pass("computer_preference");
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
        r#"provider = "openai-codex"
model = "gpt-5.5"
auth = "codex"
check_for_updates = false
web_search.mode = "off"

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
fn model_command_resolves_configured_alias() {
    let home = IsolatedHome::new().unwrap();
    std::fs::write(
        &home.config_path,
        r#"check_for_updates = false
web_search.mode = "off"

[model]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"

[model.aliases]
deep = "openai-codex/gpt-5.5"
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
    let mut harness = PtyHarness::spawn_named(&plan, "resolve_model_alias").unwrap();

    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
        .unwrap();
    harness.submit_text("/model @deep").unwrap();
    harness
        .wait_for_text(
            "model switched to openai-codex/gpt-5.5",
            WaitTimeout::secs(10, "model switch"),
        )
        .unwrap();
    assert_eq!(harness.quit_with_exit_command().unwrap(), 0);

    let config = std::fs::read_to_string(&home.config_path).unwrap();
    assert!(
        config.contains("model = \"@deep\""),
        "saved config:\n{config}"
    );
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
fn slash_command_palette() {
    assert_pass("slash_command_palette");
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

// Covers: advisor mode must ask for a model before it claims to be on, keep the
// chosen model across off and on, warn when a saved mode has no model, and bring
// the advisor's answer back to the executor without ending the turn on failure.
// Owner: interactive TUI
#[test]
fn advisor_command() {
    assert_pass("advisor_command");
}

#[test]
fn advisor_missing_model() {
    assert_pass("advisor_missing_model");
}

#[test]
fn advisor_review() {
    assert_pass("advisor_review");
}

#[test]
fn mermaid_flowchart_survives_narrow_and_restored_panes() {
    assert_pass("mermaid_flowchart_resize");
}

// Covers: a bare /skill command loads the skill before the model responds, and
// the loaded SKILL.md stays behind a collapsed card until Ctrl+O expands it.
// Owner: interactive TUI
#[test]
fn bare_skill_command_starts_a_model_turn() {
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
    let plan = RhoLaunchPlan::matrix(
        binary,
        &home,
        PtySize {
            rows: 40,
            cols: 100,
        },
    );
    let mut harness = PtyHarness::spawn_named(&plan, "bare_skill_command").unwrap();

    harness
        .wait_for_text("gpt-5.5", WaitTimeout::secs(20, "startup"))
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

#[test]
fn skill_command_reports_when_skill_tool_is_disabled() {
    let home = IsolatedHome::new().unwrap();
    let skill_dir = home.workspace.join(".agents/skills/test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: test-skill\ndescription: Test skill invocation\n---\nFollow the skill.\n",
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
    )
    .with_arg("--no-tools");
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

// Covers: /attach with no running subagents must still open an empty overlay.
// Owner: interactive TUI
#[test]
fn attach_opens_an_empty_picker_when_nothing_is_running() {
    assert_pass("attach_picker_empty");
}

// Covers: `rho attach` with no id must open the picker even when the directory
// has no running subagents.
// Owner: interactive TUI
#[test]
fn cli_attach_opens_an_empty_picker_when_nothing_is_running() {
    assert_pass("attach_cli_empty");
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

// Covers: quitting from inside the attach view must restore the terminal.
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

// Covers: a click selects a questionnaire choice without submitting, and a
// double click confirms it like Enter.
// Owner: interactive TUI
#[test]
fn questionnaire_choice_click_and_double_click_submit() {
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

// Covers: hovering a questionnaire choice lifts it and leaving reverts it.
// Owner: interactive TUI
#[test]
fn questionnaire_choice_hover_lifts_and_reverts() {
    assert_pass("questionnaire_hover");
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

// Covers: hovering the statusline model field marks it, and clicking opens the model picker.
// Owner: interactive TUI
#[test]
fn statusline_model_hover_and_click_opens_picker() {
    assert_pass("statusline_model_click");
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
