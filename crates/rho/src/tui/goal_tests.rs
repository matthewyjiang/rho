use super::*;
use pretty_assertions::assert_eq;
use rho_providers::model::ContentBlock;

fn blocked_evaluation() -> GoalEvaluation {
    GoalEvaluation::Blocked {
        reason: "repository work is complete".into(),
        pending_steps: vec![HumanStep {
            action: "publish release v1.0.0".into(),
            reason: "requires the user's release credentials".into(),
        }],
    }
}

#[test]
fn parses_met_unmet_and_fenced_evaluations() {
    assert_eq!(
        parse_evaluation(r#"{"state":"Met","reason":"tests pass","human_steps":[]}"#).unwrap(),
        GoalEvaluation::Met {
            reason: "tests pass".into(),
        }
    );
    assert_eq!(
        parse_evaluation(
            "```json\n{\"state\":\"Unmet\",\"reason\":\"lint still fails\",\"human_steps\":[]}\n```"
        )
        .unwrap(),
        GoalEvaluation::Unmet {
            reason: "lint still fails".into(),
        }
    );
}

#[test]
fn parses_blocked_evaluation_with_human_steps() {
    assert_eq!(
        parse_evaluation(
            r#"{"state":"Blocked","reason":"repository work is complete","human_steps":[{"action":"publish release v1.0.0","reason":"requires the user's release credentials"}]}"#
        )
        .unwrap(),
        blocked_evaluation()
    );
}

#[test]
fn rejects_invalid_evaluation_details() {
    assert!(parse_evaluation(r#"{"state":"Unmet","reason":"  ","human_steps":[]}"#).is_err());
    assert!(parse_evaluation(
        r#"{"state":"Blocked","reason":"waiting for user","human_steps":[]}"#
    )
    .is_err());
    assert!(parse_evaluation(
        r#"{"state":"Blocked","reason":"waiting for user","human_steps":[{"action":"push tag","reason":"  "}]}"#
    )
    .is_err());
}

#[test]
fn blocked_goal_pauses_then_can_resume_and_complete() {
    let mut goal = GoalState::new("release is public".into());

    assert_eq!(
        goal.record_evaluation(&blocked_evaluation()),
        GoalDisposition::Pause
    );
    assert_eq!(goal.loop_state(), GoalLoopState::Blocked);
    assert_eq!(goal.pending_steps(), blocked_evaluation().pending_steps());

    assert!(goal.begin_verification());
    assert_eq!(goal.loop_state(), GoalLoopState::Blocked);
    assert_eq!(goal.pending_steps(), blocked_evaluation().pending_steps());
    goal.complete_verification();
    assert_eq!(goal.loop_state(), GoalLoopState::Active);
    assert_eq!(
        goal.record_evaluation(&GoalEvaluation::Met {
            reason: "release is now public".into(),
        }),
        GoalDisposition::Complete
    );
    assert_eq!(goal.turns, 2);
}

#[test]
fn agent_resolvable_failure_remains_active() {
    let mut goal = GoalState::new("tests pass".into());
    let evaluation = parse_evaluation(
        r#"{"state":"Unmet","reason":"network request can be retried","human_steps":[]}"#,
    )
    .unwrap();

    assert_eq!(
        goal.record_evaluation(&evaluation),
        GoalDisposition::Continue
    );
    assert_eq!(goal.loop_state(), GoalLoopState::Active);
    assert!(goal.pending_steps().is_empty());
}

#[test]
fn transcript_omits_opaque_provider_context() {
    let identity =
        rho_providers::model::ModelIdentity::new("anthropic", "anthropic-messages", "claude-test");
    let transcript = evaluation_transcript(&[Message::assistant(
        rho_providers::model::AssistantMessage {
            content: vec![ContentBlock::Text("answer".into())],
            provenance: Some(identity.clone()),
            reasoning_summary: Some("safe summary".into()),
            provider_context: vec![rho_providers::model::ProviderContextBlock {
                identity,
                kind: "anthropic_content_block".into(),
                position: Some(0),
                data: serde_json::json!({"signature": "secret-signature"}),
            }],
        },
    )]);

    assert!(transcript.contains("answer"));
    assert!(transcript.contains("safe summary"));
    assert!(!transcript.contains("secret-signature"));
    assert!(!transcript.contains("provider_context"));
}

#[test]
fn transcript_tail_is_unicode_safe() {
    assert_eq!(
        tail_chars("a项目bc", 3),
        "[earlier transcript omitted]\n目bc"
    );
}

#[test]
fn formats_elapsed_time() {
    assert_eq!(format_elapsed(Duration::from_secs(9)), "9s");
    assert_eq!(format_elapsed(Duration::from_secs(125)), "2m 5s");
    assert_eq!(format_elapsed(Duration::from_secs(3_720)), "1h 2m");
    assert_eq!(
        format_elapsed_with(
            Duration::from_millis(3_200),
            ElapsedPrecision::TenthsUnderMinute
        ),
        "3.2s"
    );
    assert_eq!(
        format_elapsed_with(
            Duration::from_secs(125),
            ElapsedPrecision::TenthsUnderMinute
        ),
        "2m 5s"
    );
}
