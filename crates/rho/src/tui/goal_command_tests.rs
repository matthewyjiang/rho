use super::*;
use crate::tui::tests::test_app;
use pretty_assertions::assert_eq;

fn block_goal(goal: &mut GoalState) {
    assert_eq!(
        goal.record_evaluation(&goal::GoalEvaluation::Blocked {
            reason: "release needs user authority".into(),
            pending_steps: vec![goal::HumanStep {
                action: "push tag v1.0.0".into(),
                reason: "requires the user's Git credentials".into(),
            }],
        }),
        goal::GoalDisposition::Pause
    );
}

#[test]
fn initial_prompt_identifies_goal_setting_action() {
    assert_eq!(
        initial_goal_prompt("all tests pass"),
        "The user invoked Rho's `/goal` command to set the following completion goal. Treat this as a goal-setting action, not as an ordinary conversational message or a claim that the goal is already complete.\n\nGoal:\nall tests pass\n\nBegin working toward the goal now. Make concrete progress, use tools as needed, and verify the completion condition before stopping."
    );
}

#[test]
fn goal_turn_preserves_command_for_display_history_and_persistence() {
    let turn = TurnPrompt::command(
        initial_goal_prompt("all tests pass"),
        "/goal all tests pass".into(),
    );

    assert_eq!(turn.display, "/goal all tests pass");
    assert_eq!(turn.history, "/goal all tests pass");
    assert_eq!(
        turn.persisted_display.as_deref(),
        Some("/goal all tests pass")
    );
    assert!(turn
        .model
        .starts_with("The user invoked Rho's `/goal` command"));
}

#[test]
fn goal_aliases_are_case_insensitive() {
    for alias in ["clear", "STOP", "off", "reset", "none", "cancel"] {
        assert!(is_goal_clear_alias(alias), "{alias}");
    }
    assert!(is_goal_resume_alias("RESUME"));
    assert!(!is_goal_clear_alias("finish the work"));
    assert!(!is_goal_resume_alias("resume later"));
}

#[test]
fn clearing_goal_removes_active_indicator() {
    let mut app = test_app();
    app.goal = Some(GoalState::new("tests pass".into()));

    app.clear_goal();

    assert!(app.goal.is_none());
    assert_eq!(app.status, "goal cleared");
    assert!(matches!(
        app.transcript.last(),
        Some(Entry::Notice(message)) if message == "goal cleared"
    ));
}

#[test]
fn status_reports_active_condition_and_progress() {
    let mut app = test_app();
    let mut goal = GoalState::new("tests pass".into());
    goal.turns = 3;
    goal.last_reason = Some("one test still fails".into());
    app.goal = Some(goal);

    let status = app.goal_status_message();

    assert!(status.contains("goal active: tests pass"), "{status}");
    assert!(status.contains("3 turn(s)"), "{status}");
    assert!(
        status.contains("last evaluation: one test still fails"),
        "{status}"
    );
}

#[test]
fn status_reports_blocked_steps_and_resumption_command() {
    let mut app = test_app();
    let mut goal = GoalState::new("release is public".into());
    block_goal(&mut goal);
    app.goal = Some(goal);

    let status = app.goal_status_message();

    assert!(
        status.contains("goal blocked: release is public"),
        "{status}"
    );
    assert!(status.contains("- push tag v1.0.0"), "{status}");
    assert!(
        status.contains("requires the user's Git credentials"),
        "{status}"
    );
    assert!(status.contains("use /goal resume"), "{status}");
}

#[test]
fn blocked_goal_does_not_resume_after_a_turn() {
    for outcome in [TurnOutcomeKind::Completed, TurnOutcomeKind::Failed] {
        assert!(should_resume_goal_after_turn(
            outcome,
            Some(goal::GoalLoopState::Active),
            /*should_quit*/ false
        ));
        assert!(!should_resume_goal_after_turn(
            outcome,
            Some(goal::GoalLoopState::Blocked),
            /*should_quit*/ false
        ));
    }
    assert!(!should_resume_goal_after_turn(
        TurnOutcomeKind::Interrupted,
        Some(goal::GoalLoopState::Active),
        /*should_quit*/ false
    ));
    assert!(!should_resume_goal_after_turn(
        TurnOutcomeKind::Cancelled,
        Some(goal::GoalLoopState::Active),
        /*should_quit*/ false
    ));
    assert!(!should_resume_goal_after_turn(
        TurnOutcomeKind::Failed,
        None,
        /*should_quit*/ false
    ));
    assert!(!should_resume_goal_after_turn(
        TurnOutcomeKind::Failed,
        Some(goal::GoalLoopState::Active),
        /*should_quit*/ true
    ));
}

#[test]
fn user_message_resumes_blocked_goal_with_verification_first() {
    let mut app = test_app();
    let mut goal = GoalState::new("release is public".into());
    block_goal(&mut goal);
    app.goal = Some(goal);

    let turn = app.prepare_goal_resumption_turn(TurnPrompt::standard(
        "I pushed it".into(),
        "I pushed it".into(),
    ));

    assert_eq!(
        app.goal.as_ref().map(GoalState::loop_state),
        Some(goal::GoalLoopState::Blocked)
    );
    assert!(turn.model.contains("First verify"), "{}", turn.model);
    assert!(turn.model.contains("push tag v1.0.0"), "{}", turn.model);
    assert!(turn.model.contains("I pushed it"), "{}", turn.model);
    assert_eq!(turn.history, "I pushed it");
    assert_eq!(turn.persisted_display.as_deref(), Some("I pushed it"));

    app.finish_goal_resumption_turn(TurnOutcomeKind::Interrupted);

    let goal = app.goal.as_ref().expect("goal remains armed");
    assert_eq!(goal.loop_state(), goal::GoalLoopState::Blocked);
    assert_eq!(goal.pending_steps()[0].action, "push tag v1.0.0");
}

#[test]
fn command_prompt_resumes_blocked_goal_without_exposing_expanded_context() {
    let mut app = test_app();
    let mut goal = GoalState::new("release is public".into());
    block_goal(&mut goal);
    let expected_model = blocked_goal_resumption_prompt(
        "release is public",
        goal.pending_steps(),
        Some("expanded skill instructions"),
    );
    app.goal = Some(goal);

    let turn = app.prepare_goal_resumption_turn(TurnPrompt::command(
        "expanded skill instructions".into(),
        "/skill:release".into(),
    ));

    assert_eq!(
        turn,
        TurnPrompt {
            model: expected_model,
            display: "/skill:release".into(),
            history: "/skill:release".into(),
            persisted_display: Some("/skill:release".into()),
        }
    );
}

#[test]
fn completed_resumption_turn_activates_goal_loop() {
    let mut app = test_app();
    let mut goal = GoalState::new("release is public".into());
    block_goal(&mut goal);
    app.goal = Some(goal);
    app.prepare_goal_resumption_turn(TurnPrompt::standard(
        "I pushed it".into(),
        "I pushed it".into(),
    ));

    app.finish_goal_resumption_turn(TurnOutcomeKind::Completed);

    assert_eq!(
        app.goal.as_ref().map(GoalState::loop_state),
        Some(goal::GoalLoopState::Active)
    );
}

#[test]
fn failed_goal_turn_drains_queued_prompts_before_retrying() {
    assert!(should_drain_queued_prompts(
        TurnOutcomeKind::Failed,
        /*resume_goal*/ true
    ));
    assert!(!should_drain_queued_prompts(
        TurnOutcomeKind::Failed,
        /*resume_goal*/ false
    ));
    assert!(should_drain_queued_prompts(
        TurnOutcomeKind::Completed,
        /*resume_goal*/ false
    ));
    assert!(!should_drain_queued_prompts(
        TurnOutcomeKind::Cancelled,
        /*resume_goal*/ false
    ));
}

#[test]
fn goal_loop_retries_failed_turns_and_stops_on_interrupt_or_cancel() {
    assert_eq!(
        goal_loop_action_after_turn(TurnOutcomeKind::Completed),
        GoalLoopAction::Continue
    );
    assert_eq!(
        goal_loop_action_after_turn(TurnOutcomeKind::Failed),
        GoalLoopAction::RetryAfterFailure
    );
    assert_eq!(
        goal_loop_action_after_turn(TurnOutcomeKind::Interrupted),
        GoalLoopAction::Stop
    );
    assert_eq!(
        goal_loop_action_after_turn(TurnOutcomeKind::Cancelled),
        GoalLoopAction::Stop
    );
}
