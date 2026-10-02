use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use pretty_assertions::assert_eq;
use rho_sdk::{
    tool::{tool_progress_channel, ToolContext, ToolErrorKind, ToolInvocation},
    CancellationToken, ToolCallId,
};
use serde_json::json;

use super::message_parent_bundle;
use crate::{
    app::subagent_messaging::{NoticeDelivery, NoticePostError, NoticePoster, ValidatedMessage},
    tools::sdk_registry::ToolBundle,
};

#[derive(Default)]
struct RecordingPoster(Mutex<Vec<(ValidatedMessage, NoticeDelivery)>>);

impl NoticePoster for RecordingPoster {
    fn post(
        &self,
        message: ValidatedMessage,
        delivery: NoticeDelivery,
    ) -> Result<(), NoticePostError> {
        self.0.lock().unwrap().push((message, delivery));
        Ok(())
    }
}

// Covers: ordinary messages must not request a wake; action requests must not silently wait.
// Owner: child communication tool dispatch and argument validation.
#[tokio::test]
async fn subagent_notice_tools_route_delivery_and_reject_empty_messages() {
    let poster = Arc::new(RecordingPoster::default());
    let bundle = message_parent_bundle(poster.clone());
    for (name, delivery) in [
        ("message_parent", NoticeDelivery::NextTurn),
        (
            "request_parent_action",
            NoticeDelivery::ParentActionRequired,
        ),
    ] {
        let tool = bundle
            .tools()
            .iter()
            .find(|tool| tool.spec().name == name)
            .unwrap();
        for message in ["  coordinate file ownership  ", " \n "] {
            let (progress, _receiver) = tool_progress_channel(NonZeroUsize::MIN);
            let context = ToolContext::new(None, CancellationToken::new(), progress);
            let result = tool
                .call(
                    ToolInvocation::new(ToolCallId::new(), json!({"message": message})),
                    context,
                )
                .await;
            if message.trim().is_empty() {
                assert_eq!(result.unwrap_err().kind(), ToolErrorKind::InvalidArguments);
                assert_eq!(*poster.0.lock().unwrap(), vec![]);
            } else {
                result.unwrap();
                assert_eq!(
                    std::mem::take(&mut *poster.0.lock().unwrap()),
                    vec![(ValidatedMessage::parse(message).unwrap(), delivery)]
                );
            }
        }
    }
}

// Covers: model-written nested tool calls cannot load user-only skills.
// Owner: app skill access policy, reached through the SDK model and child-host path.
#[tokio::test]
async fn model_originated_nested_call_cannot_load_user_only_skill() {
    use rho_sdk::{
        model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
        provider::{ScriptedProvider, ScriptedTurn},
        tool::{Tool, ToolError, ToolFuture, ToolOutput},
        Rho, RunEvent, ScopedWorkspacePolicy, SessionOptions, ToolCompletion, ToolHost,
        ToolHostCall, UserInput, Workspace,
    };

    struct NestedSkillTool;
    impl Tool for NestedSkillTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "nested_skill".into(),
                description: "load a skill through a child host".into(),
                input_schema: json!({"type": "object"}),
            }
        }

        fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
            Box::pin(async move {
                let child = ToolHost::child_builder(&context)
                    .tool(super::SdkSkillTool::new(
                        crate::config::Config::default().max_output_bytes,
                    ))
                    .build()
                    .unwrap();
                let result: Result<ToolOutput, rho_sdk::Error> = child
                    .invoke(ToolHostCall::new("skill", invocation.into_arguments()))
                    .await;
                result.map_err(|error| match error {
                    rho_sdk::Error::Tool(error) => error,
                    error => ToolError::new(ToolErrorKind::Execution, error.to_string()),
                })
            })
        }
    }

    let root = tempfile::tempdir().unwrap();
    let skill_dir = root.path().join(".agents/skills/manual-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: manual-skill\ndescription: manual skill\ndisable-model-invocation: true\n---\nmanual body\n",
    )
    .unwrap();
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "nested-skill-1".into(),
                    name: "nested_skill".into(),
                    arguments: json!({"name": "manual-skill"}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let runtime = Rho::builder()
        .provider(provider)
        .workspace(Workspace::new(root.path()).unwrap())
        .workspace_policy(ScopedWorkspacePolicy::new().allow_skills())
        .tool(NestedSkillTool)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("load it")).await.unwrap();
    let mut completions = Vec::new();
    while let Some(event) = run.next_event().await {
        if let RunEvent::ToolFinished { result, .. } = event {
            completions.push(result);
        }
    }
    run.outcome().await.unwrap();
    assert_eq!(completions.len(), 1);
    let ToolCompletion::Failure(failure) = &completions[0] else {
        panic!("a model-originated nested skill load must be denied");
    };
    assert_eq!(failure.kind(), ToolErrorKind::PolicyDenied);
}
