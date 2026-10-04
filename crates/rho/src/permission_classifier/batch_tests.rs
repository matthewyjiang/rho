use std::{path::Path, sync::Arc};

use pretty_assertions::assert_eq;
use rho_providers::reasoning::ReasoningLevel;
use rho_sdk::{
    decision::QuestionKind,
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolCall},
    provider::{ScriptedProvider, ScriptedTurn},
    ApprovalRequest, CancellationToken, CapabilityRequest, CapabilitySource, PathScope,
    ProviderError, ProviderErrorKind, ProviderRequestUsageRecording, Retryability, SessionId,
};

use super::{
    batch::{BatchMember, BatchRequest},
    classify::{ClassifierModel, Screen},
    ClassifierVerdict, TranscriptBudget, TranscriptOverBudget, REVIEW_QUESTION,
};

fn write(path: &str) -> ApprovalRequest {
    ApprovalRequest::new(
        CapabilityRequest::write_path(
            path,
            PathScope::PrimaryWorkspace,
            CapabilitySource::built_in_tool("write"),
        ),
        "",
    )
}

fn history() -> Vec<Message> {
    let call = |id: &str, path: &str| {
        ContentBlock::ToolCall(ToolCall {
            id: id.into(),
            name: "write".into(),
            arguments: serde_json::json!({"path": path, "content": ""}),
        })
    };
    vec![
        Message::User(vec![ContentBlock::Text("update a and c".into())]),
        Message::Assistant(vec![call("w1", "a"), call("w2", "b"), call("w3", "c")]),
    ]
}

fn text_turn(text: &str) -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
        text.into(),
    )]))
}

fn scope_deny() -> ClassifierVerdict {
    let QuestionKind::Choice(options) = REVIEW_QUESTION.kind else {
        panic!("the review is a choice question");
    };
    let option = options
        .iter()
        .find(|option| option.id == "deny_scope_expansion")
        .unwrap();
    ClassifierVerdict::Deny {
        reason: option.description.into(),
    }
}

/// A result as a comparable value: the verdict, or whether the error kept its
/// over-budget type.
#[derive(Debug, PartialEq)]
enum Reviewed {
    Verdict(ClassifierVerdict),
    OverBudget,
    Failed,
}

impl From<anyhow::Result<ClassifierVerdict>> for Reviewed {
    fn from(result: anyhow::Result<ClassifierVerdict>) -> Self {
        match result {
            Ok(verdict) => Self::Verdict(verdict),
            Err(error) if error.downcast_ref::<TranscriptOverBudget>().is_some() => {
                Self::OverBudget
            }
            Err(_) => Self::Failed,
        }
    }
}

// Covers: the review of a batch asks once for every member under its own
// question, even when the screen let an earlier member through, so each
// answer lands on the member it names; a lone member takes the single
// review; and a failed batch fails every member, keeping an over-budget
// transcript typed so production can still show its token counts.
// Owner: permission classifier batched review
#[tokio::test]
async fn batch_review_answers_each_member_under_its_own_question() {
    let cases = [
        (
            "members 1 and 3 together",
            vec![0, 2],
            TranscriptBudget::Unbounded,
            vec![text_turn(
                r#"{"request_1":"allow","request_3":"deny_scope_expansion"}"#,
            )],
            vec![
                Reviewed::Verdict(ClassifierVerdict::Allow),
                Reviewed::Verdict(scope_deny()),
            ],
            1,
        ),
        (
            "lone member takes the single review",
            vec![1],
            TranscriptBudget::Unbounded,
            vec![text_turn(r#"{"verdict":"deny_scope_expansion"}"#)],
            vec![Reviewed::Verdict(scope_deny())],
            1,
        ),
        (
            "provider error fails every member",
            vec![0, 1],
            TranscriptBudget::Unbounded,
            vec![ScriptedTurn::failed(ProviderError::new(
                ProviderErrorKind::Unavailable,
                "provider down",
                Retryability::Permanent,
            ))],
            vec![Reviewed::Failed, Reviewed::Failed],
            1,
        ),
        (
            "over-budget transcript fails every member without a call",
            vec![0, 1],
            TranscriptBudget::Tokens(1),
            vec![],
            vec![Reviewed::OverBudget, Reviewed::OverBudget],
            0,
        ),
    ];

    let history = history();
    let pendings = [write("a"), write("b"), write("c")];
    let members: Vec<BatchMember<'_>> = pendings
        .iter()
        .zip(["w1", "w2", "w3"])
        .map(|(pending, call_id)| BatchMember {
            pending,
            call_id: Some(call_id),
        })
        .collect();
    let session_id = SessionId::new();
    let request = BatchRequest {
        history: &history,
        members: &members,
        cancellation: CancellationToken::new(),
        session_id: &session_id,
        workspace_path: Path::new("/workspace"),
        usage_recording: ProviderRequestUsageRecording::default(),
    };

    for (name, reviewed, budget, turns, expected, expected_requests) in cases {
        let provider = Arc::new(ScriptedProvider::new(
            ModelIdentity::new("provider", "api", "model"),
            turns,
        ));
        let model = ClassifierModel {
            provider: provider.clone(),
            reasoning: ReasoningLevel::Medium,
            budget,
            screen: Screen::Classifier,
        };

        let results: Vec<Reviewed> = model
            .review_batch(&request, &reviewed)
            .await
            .into_iter()
            .map(Reviewed::from)
            .collect();

        assert_eq!(
            (results, provider.recorded_requests().len()),
            (expected, expected_requests),
            "{name}"
        );
    }
}
