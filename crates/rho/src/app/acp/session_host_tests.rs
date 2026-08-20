use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{atomic::AtomicU64, Arc, Mutex},
    time::Duration,
};

use agent_client_protocol::{
    schema::v1::{
        ContentBlock as AcpContentBlock, EmbeddedResource, EmbeddedResourceResource, ImageContent,
        RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
        ResourceLink, SessionId, SessionNotification, SessionUpdate, TextResourceContents,
    },
    Error as AcpError,
};
use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ImageContent as SdkImage},
    ApprovalRequest, CancellationToken, CapabilityRequest, CapabilitySource, PathScope,
    PendingApproval, RunEvent,
};

use super::{
    advertised_config_options,
    convert::{user_input_from_prompt, validate_session_cwd, workspace_cwd, SessionCwdError},
    pump_sources, ApprovalSource, CurrentModel, EventMapper, EventSource, PromptGate,
};
use crate::{app::acp::AcpClientPort, config::Config};
use rho_providers::reasoning::ReasoningLevel;

// Covers: session/new and session/load must refuse a non-workspace cwd
// Owner: acp session host
#[test]
fn session_cwd_must_be_an_absolute_existing_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let file = dir.join("not-a-dir");
    std::fs::write(&file, b"x").unwrap();
    let missing = dir.join("missing");

    let cases: &[(&str, PathBuf, Result<(), SessionCwdError>)] = &[
        (
            "relative",
            PathBuf::from("relative"),
            Err(SessionCwdError::NotAbsolute),
        ),
        ("missing", missing, Err(SessionCwdError::NotDirectory)),
        ("file", file, Err(SessionCwdError::NotDirectory)),
        ("directory", dir, Ok(())),
    ];

    for (label, path, expected) in cases {
        assert_eq!(validate_session_cwd(path).map(drop), *expected, "{label}");
    }
}

// Covers: session/load must fall back to the process cwd when the request cwd is empty
// Owner: acp session host
#[test]
fn load_workspace_prefers_request_cwd_then_process_cwd() {
    let process = Path::new("/process");
    let requested = Path::new("/requested");
    assert_eq!(workspace_cwd(Path::new(""), process), process);
    assert_eq!(workspace_cwd(requested, process), requested);
}

// Covers: prompt blocks must reach the model as typed text/image or fenced context
// Owner: acp session host
#[test]
fn prompt_content_maps_text_image_and_embedded_context() {
    let prompt = vec![
        AcpContentBlock::from("hello"),
        AcpContentBlock::Image(ImageContent::new("abc", "image/png")),
        AcpContentBlock::Resource(EmbeddedResource::new(
            EmbeddedResourceResource::TextResourceContents(TextResourceContents::new(
                "fn main() {}",
                "file:///workspace/main.rs",
            )),
        )),
        AcpContentBlock::ResourceLink(ResourceLink::new("notes", "file:///workspace/notes.md")),
    ];

    let input = user_input_from_prompt(&prompt).unwrap();
    assert_eq!(
        input.blocks(),
        [
            ContentBlock::Text("hello".into()),
            ContentBlock::Image(SdkImage {
                data: "abc".into(),
                mime_type: "image/png".into(),
            }),
            ContentBlock::Text("```resource file:///workspace/main.rs\nfn main() {}\n```".into()),
            ContentBlock::Text("```resource file:///workspace/notes.md\nnotes\n```".into()),
        ]
    );
}

// Covers: a resource body that contains its own fence must not close ours early
// Owner: acp session host
#[test]
fn fenced_context_outgrows_backticks_in_the_body() {
    let prompt = vec![AcpContentBlock::Resource(EmbeddedResource::new(
        EmbeddedResourceResource::TextResourceContents(TextResourceContents::new(
            "```rust\nfn main() {}\n```",
            "file:///workspace/readme.md",
        )),
    ))];

    let input = user_input_from_prompt(&prompt).unwrap();
    assert_eq!(
        input.blocks(),
        [ContentBlock::Text(
            "````resource file:///workspace/readme.md\n```rust\nfn main() {}\n```\n````".into()
        )]
    );
}

// Covers: a resource URI that contains a fence must not close the generated fence
// Owner: acp session host
#[test]
fn fenced_context_outgrows_backticks_in_the_header() {
    let prompt = vec![AcpContentBlock::ResourceLink(ResourceLink::new(
        "notes",
        "file:///workspace/notes.md\n```\ninjected",
    ))];

    let input = user_input_from_prompt(&prompt).unwrap();
    assert_eq!(
        input.blocks(),
        [ContentBlock::Text(
            "````resource file:///workspace/notes.md\n```\ninjected\nnotes\n````".into()
        )]
    );
}

// Covers: an empty prompt must not start a run
// Owner: acp session host
#[test]
fn empty_prompt_is_rejected() {
    assert!(user_input_from_prompt(&[]).is_err());
}

// Covers: session/cancel must be safe when no prompt is running
// Owner: acp session host
#[test]
fn cancel_is_idle_safe() {
    PromptGate::new().cancel();
}

// Covers: session/cancel during prompt start must still cancel the run
// Owner: acp session host
#[test]
fn cancel_during_start_marks_the_gate() {
    let gate = PromptGate::new();
    gate.begin();
    gate.cancel();
    let token = rho_sdk::CancellationToken::new();
    gate.activate(token.clone());
    assert!(token.is_cancelled());
}

// Covers: a cancel aimed at a finished prompt must not cancel the next one
// Owner: acp session host
#[test]
fn cancel_before_the_next_prompt_does_not_leak_into_it() {
    let gate = PromptGate::new();
    gate.begin();
    gate.finish();
    gate.cancel();
    gate.begin();
    let token = rho_sdk::CancellationToken::new();
    gate.activate(token.clone());
    assert!(!token.is_cancelled());
}

struct HoldingPermissionPort {
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    updates: Arc<Mutex<Vec<SessionUpdate>>>,
}

impl AcpClientPort for HoldingPermissionPort {
    fn send_session_notification(
        &self,
        notification: SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        Box::pin(async move {
            self.updates
                .lock()
                .expect("updates")
                .push(notification.update);
            Ok(())
        })
    }

    fn request_permission(
        &self,
        _request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, AcpError>> + Send + '_>>
    {
        Box::pin(async move {
            let entered = self.entered.lock().expect("entered").take();
            if let Some(entered) = entered {
                let _ = entered.send(());
            }
            let release = self.release.lock().expect("release").take();
            if let Some(release) = release {
                let _ = release.await;
            }
            Ok(RequestPermissionResponse::new(
                RequestPermissionOutcome::Cancelled,
            ))
        })
    }
}

// Covers: a host permission prompt must not stall SDK event delivery
// Owner: acp session host
#[tokio::test]
async fn permission_request_does_not_block_event_drain() {
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let updates = Arc::new(Mutex::new(Vec::new()));
    let port = HoldingPermissionPort {
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(Some(release_rx)),
        updates: Arc::clone(&updates),
    };
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let (approval_tx, mut approval_rx) = tokio::sync::mpsc::unbounded_channel();
    let (pending, _decision) = PendingApproval::new(ApprovalRequest::new(
        CapabilityRequest::write_path(
            "/workspace/file.rs",
            PathScope::PrimaryWorkspace,
            CapabilitySource::built_in_tool("write"),
        ),
        "edit file",
    ));

    let pump = tokio::spawn(async move {
        let session_id = SessionId::new("session-1");
        let cancel = CancellationToken::new();
        let mut mapper = EventMapper::new();
        let placeholders = AtomicU64::new(0);
        pump_sources(super::PumpSources {
            session_id: &session_id,
            cancel,
            mapper: &mut mapper,
            client: &port,
            placeholders: &placeholders,
            events: &mut EventSource::Channel(&mut event_rx),
            approvals: &mut ApprovalSource::Channel(&mut approval_rx),
            approvals_open: true,
        })
        .await
    });

    approval_tx.send(pending).expect("approval");
    entered_rx.await.expect("permission started");
    event_tx
        .send(RunEvent::AssistantTextDelta { text: "hi".into() })
        .expect("event");

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if updates
                .lock()
                .expect("updates")
                .iter()
                .any(|update| matches!(update, SessionUpdate::AgentMessageChunk(_)))
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("event must be delivered while the permission request is still pending");

    release_tx.send(()).expect("release permission");
    drop(event_tx);
    drop(approval_tx);
    pump.await.expect("pump task").expect("pump");
}

// Covers: session/new and set_config_option must advertise model, then
// thought_level only when the current model can change reasoning.
// Owner: acp session config mapper
#[test]
fn advertised_options_merge_model_with_optional_thought_level() {
    let cases = [
        (
            CurrentModel {
                provider: "test".into(),
                model: "model".into(),
                auth: "api-key".into(),
            },
            ["model", "thought_level"].as_slice(),
        ),
        (
            CurrentModel {
                provider: "github-copilot".into(),
                model: "gpt-4.1".into(),
                auth: "github-copilot".into(),
            },
            ["model"].as_slice(),
        ),
    ];
    for (current, expected_ids) in cases {
        let config = Config {
            provider: current.provider.clone(),
            model: current.model.clone(),
            auth: current.auth.clone(),
            ..Config::default()
        };
        let options = advertised_config_options(&current, &config, ReasoningLevel::High);
        let ids = options
            .iter()
            .map(|option| option.id.0.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(ids, expected_ids, "{}", current.provider);
    }
}
