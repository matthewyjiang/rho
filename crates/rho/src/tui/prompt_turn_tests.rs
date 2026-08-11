use super::*;
use crate::tui::tests::test_app;

#[test]
fn terminal_lifecycle_errors_bypass_sdk_failure_handling() {
    let error = sdk_failure_from_running_terminal_error(
        super::during_turn::RunningTerminalError::Terminal(anyhow::anyhow!("resume failed")),
    )
    .unwrap_err();

    assert_eq!(error.to_string(), "resume failed");
}

fn failed_turn() -> FailedTurn {
    FailedTurn {
        input: rho_sdk::UserInput::text("continue the existing goal turn"),
        display_user: Some(Message::user_text("continuing active goal")),
        notification_context: None,
        initial_tool_call: None,
        generate_session_title_after_completion: true,
    }
}

#[test]
fn approval_and_questionnaire_share_one_interaction_slot() {
    assert!(interaction_slot_available(false, false));
    assert!(!interaction_slot_available(true, false));
    assert!(!interaction_slot_available(false, true));
    assert!(!interaction_slot_available(true, true));
}

#[test]
fn mixed_interactions_preserve_arrival_order() {
    let approval_request = rho_sdk::ApprovalRequest::new(
        rho_sdk::CapabilityRequest::read_path(
            "/workspace/file",
            rho_sdk::PathScope::PrimaryWorkspace,
            rho_sdk::CapabilitySource::built_in_tool("read_file"),
        ),
        "approval required",
    );
    let (approval, _approval_response) = rho_sdk::PendingApproval::new(approval_request);
    let question = rho_sdk::HostQuestion::new(
        "color",
        "Choose one color",
        vec![rho_sdk::HostChoice::new("blue", "blue")],
        rho_sdk::SelectionMode::One,
    )
    .unwrap();
    let parent_request =
        rho_sdk::HostInputRequest::questionnaire("parent", vec![question.clone()]).unwrap();
    let child_request = rho_sdk::HostInputRequest::questionnaire("child", vec![question]).unwrap();
    let (child_response, _child_response_rx) = tokio::sync::oneshot::channel();
    let child = crate::app::subagent_host_input::SubagentHostInputRequest {
        run_id: "abc123".into(),
        agent_id: "worker".into(),
        parent_session_id: rho_sdk::SessionId::new(),
        request: child_request,
        response: child_response,
    };

    let mut queue = RunningInteractionQueue::default();
    queue.push(QueuedRunningInteraction::Approval(approval));
    queue.push(QueuedRunningInteraction::SubagentQuestionnaire(child));
    queue.push(QueuedRunningInteraction::ParentQuestionnaire {
        call_id: rho_sdk::ToolCallId::from_string("parent-call").unwrap(),
        request: parent_request,
    });

    assert!(matches!(
        queue.pop(),
        Some(QueuedRunningInteraction::Approval(_))
    ));
    assert!(matches!(
        queue.pop(),
        Some(QueuedRunningInteraction::SubagentQuestionnaire(_))
    ));
    assert!(matches!(
        queue.pop(),
        Some(QueuedRunningInteraction::ParentQuestionnaire { .. })
    ));
    assert!(queue.pop().is_none());
}

// Covers: retrying the first turn must retain deferred title generation.
// Owner: TUI turn orchestration
#[test]
fn retry_request_reuses_the_failed_turn_and_deferred_title_state() {
    let failed_turn = failed_turn();
    let PromptTurnRequest::Retry(retry) = PromptTurnRequest::Retry(failed_turn.clone()) else {
        unreachable!("constructed a retry request")
    };

    assert_eq!(retry, failed_turn);
}

#[test]
fn persisted_display_excludes_notification_context_for_standard_and_command_prompts() {
    let cases = [
        (
            TurnPrompt::standard("model prompt".into(), "visible prompt".into()),
            "model prompt",
            "visible prompt",
        ),
        (
            TurnPrompt::command("expanded command context".into(), "/goal run tests".into()),
            "expanded command context",
            "/goal run tests",
        ),
    ];

    for (prompt, expected_model, expected_display) in cases {
        let mut failed_turn = FailedTurn::from_prompt(prompt, Vec::new()).unwrap();
        failed_turn.attach_notification_context("hidden agent result".into());
        let model_input = failed_turn.model_input().unwrap();

        assert_eq!(
            model_input.blocks(),
            &[
                ContentBlock::Text("hidden agent result".into()),
                ContentBlock::Text(expected_model.into()),
            ]
        );
        assert_eq!(
            failed_turn.display_user,
            Some(Message::user_text(expected_display))
        );
    }
}

#[test]
fn retry_attachment_keeps_prior_batches_and_adds_each_new_batch_once() {
    let mut failed_turn = FailedTurn::from_prompt(
        TurnPrompt::standard("user prompt".into(), "user prompt".into()),
        Vec::new(),
    )
    .unwrap();
    failed_turn.attach_notification_context("first batch".into());

    let mut retry = failed_turn.clone();
    retry.attach_notification_context("second batch".into());
    let later_retry_without_new_notifications = retry.clone();
    let model_input = later_retry_without_new_notifications.model_input().unwrap();
    let ContentBlock::Text(context) = &model_input.blocks()[0] else {
        panic!("notification context must be text")
    };

    assert_eq!(context.matches("first batch").count(), 1);
    assert_eq!(context.matches("second batch").count(), 1);
    assert!(context.find("first batch") < context.find("second batch"));
    assert!(context.len() <= crate::tools::agent::NOTIFICATION_CONTEXT_BYTES);
    assert_eq!(
        &model_input.blocks()[1..],
        &[ContentBlock::Text("user prompt".into())]
    );
    assert_eq!(
        later_retry_without_new_notifications.display_user,
        Some(Message::user_text("user prompt"))
    );
}

// Covers: mixed chat media must keep image blocks multimodal while injecting document text only
// into model context and retaining compact transcript content.
// Owner: TUI prompt assembly
#[test]
fn prompt_assembly_preserves_order_and_separates_document_display_from_model() {
    let image = ImageContent {
        data: "aW1hZ2U=".into(),
        mime_type: "image/png".into(),
    };
    let document = ChatTextDocument {
        name: "report.pdf".into(),
        mime: "application/pdf".into(),
        body: "extracted body".into(),
        truncated: true,
        warnings: vec!["some pages had no text".into()],
    };

    let failed_turn = FailedTurn::from_prompt(
        TurnPrompt::standard("model prompt".into(), "display prompt".into()),
        vec![
            ChatMedia::TextDocument(document),
            ChatMedia::Image(image.clone()),
        ],
    )
    .unwrap();

    assert_eq!(
        failed_turn.input.blocks(),
        &[
            ContentBlock::Text("model prompt".into()),
            ContentBlock::Text(
                "Attached file: report.pdf (application/pdf)\nExtraction notice: content was truncated to the attachment limit.\nExtraction warnings: some pages had no text\n<<<\nextracted body\n>>>"
                    .into(),
            ),
            ContentBlock::Image(image.clone()),
        ]
    );
    assert_eq!(
        failed_turn.display_user,
        Some(Message::User(vec![
            ContentBlock::Text("display prompt".into()),
            ContentBlock::Text("[pdf: report.pdf · 14 chars · truncated]".into()),
            ContentBlock::Image(image),
        ]))
    );
}

#[test]
fn failed_turn_keeps_live_partial_assistant_text_before_error() {
    let mut app = test_app();
    app.begin_provider_turn_ui();
    app.turn.set_current_turn_start(Some(0));
    app.streams
        .assistant_stream
        .push_delta("partial assistant before stream failure");

    let outcome =
        app.finalize_failed_turn("provider stream failed".into(), failed_turn(), Vec::new());

    assert_eq!(outcome.kind(), TurnOutcomeKind::Failed);
    assert!(matches!(
        app.history.entries(),
        [Entry::Assistant(text), Entry::Error(error)]
            if text == "partial assistant before stream failure"
                && error == "provider stream failed"
    ));
    assert!(!app.is_ui_busy());
    assert!(app.streams.assistant_stream.is_empty());
    assert_eq!(app.status(), "error");
}

// Covers: idle completion batches stay restorable only until provider start
// accepts the input. A later failure must not put the batch back, or the next
// turn boundary would redeliver the same notice.
// Owner: TUI turn-boundary delivery
#[tokio::test]
async fn committed_idle_boundary_batch_is_not_restored_after_post_start_failure() {
    let mut app = test_app();
    let mut agent = crate::app::interactive_runtime::test_edit_tool_runtime(
        crate::config::EditTool::default(),
    )
    .await;
    let session_id = agent.session_id().clone();
    app.subagent_inbox
        .push_notice_for_test(crate::app::subagent_messaging::SubagentNotice {
            run_id: "abc123".into(),
            agent_id: "worker".into(),
            parent_session_id: session_id.clone(),
            message: "child is blocked on schema".into(),
        });

    let delivery = app
        .collect_turn_boundary_prompts(&mut agent)
        .expect("pending notice should drain");
    assert!(!app.subagent_inbox.has_pending_notices());

    // Pre-start abandon restores so a later turn can deliver again.
    let mut pending = Some(RestorableTurnBoundary::Standalone(delivery.batch));
    app.abandon_provider_turn_start(&mut agent, &mut pending);
    assert!(pending.is_none());
    assert!(
        app.subagent_inbox.has_pending_notices(),
        "failed provider start must restore the drained batch"
    );

    let delivery = app
        .collect_turn_boundary_prompts(&mut agent)
        .expect("restored notice should drain again");
    let mut pending = Some(RestorableTurnBoundary::Standalone(delivery.batch));
    // Provider start accepted the input: commit by taking the restorable batch.
    let _committed = pending.take();
    // Post-start failure path calls the same abandon helper; with nothing left
    // pending it must not resurrect the delivery.
    app.abandon_provider_turn_start(&mut agent, &mut pending);
    assert!(
        !app.subagent_inbox.has_pending_notices(),
        "committed delivery must not return after a post-start failure"
    );
    assert!(
        app.collect_turn_boundary_prompts(&mut agent).is_none(),
        "next turn boundary must not repeat the committed delivery"
    );
}
