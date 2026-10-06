//! Runtime-contract test inputs: no user auth/config or process environment.

use super::{
    permission::PermissionDecision,
    policy::{AcpAgentPolicy, AcpSpawnPlan, ExtensionAnswer, SessionConfigChoice},
    AcpSessionRequest,
};
use crate::{
    agent::{AgentRuntime, PromptPolicy},
    cli_runtime::{
        parent_messages::ParentMessageInbox, status_sink::RuntimeLabel, CliExecutable,
        CliSessionOverrides,
    },
    run_artifacts::{AttachmentEvent, RunArtifactIdentity},
    subagent::{self, RunStatus},
};
use agent_client_protocol::schema::v1::{
    AuthMethod, AuthMethodId, Meta, RequestPermissionRequest, SessionModeId,
};
use serde_json::{json, Value};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};

// Failure bound only: four possible handshake steps, each with the production
// ten-second budget. Synchronization always uses observed requests/signals.
pub(super) const TEST_BUDGET: Duration = super::handshake::HANDSHAKE_STEP_BUDGET.saturating_mul(4);

pub(super) struct TestPolicy {
    pub(super) cwd: PathBuf,
    pub(super) decision: PermissionDecision,
    pub(super) mode: Option<String>,
    pub(super) auth: Option<String>,
    pub(super) meta: Option<Meta>,
    pub(super) config: Vec<SessionConfigChoice>,
    pub(super) argv: Vec<OsString>,
    pub(super) env: Vec<(OsString, OsString)>,
}

impl TestPolicy {
    pub(super) fn new(cwd: &Path) -> Self {
        Self {
            cwd: cwd.into(),
            decision: PermissionDecision::AllowOnce,
            mode: None,
            auth: None,
            meta: None,
            config: vec![],
            argv: vec![],
            env: vec![],
        }
    }
}

impl AcpAgentPolicy for TestPolicy {
    fn label(&self) -> RuntimeLabel {
        RuntimeLabel {
            starting_activity: "starting fake",
            program: "fake-acp",
            resume_command: None,
            session_label: "acp session",
            cost_label: "acp cost",
        }
    }
    fn resolve_executable(&self) -> Result<CliExecutable, String> {
        Err("test must inject executable".into())
    }
    fn spawn_plan(&self, _frozen: Option<Vec<String>>) -> Result<AcpSpawnPlan, String> {
        Ok(AcpSpawnPlan {
            argv: self.argv.clone(),
            cwd: self.cwd.clone(),
            env: self.env.clone(),
        })
    }
    fn prepare(&mut self, _run_dir: &Path) -> Result<Vec<String>, String> {
        Ok(vec![])
    }
    fn log_path(&self, output: &Path) -> PathBuf {
        output.with_file_name("fake.stderr.log")
    }
    fn auth_method(&self, _advertised: &[AuthMethod]) -> Option<AuthMethodId> {
        self.auth.clone().map(AuthMethodId::new)
    }
    fn session_mode(&self) -> Option<SessionModeId> {
        self.mode.clone().map(SessionModeId::new)
    }
    fn session_meta(&self) -> Option<Meta> {
        self.meta.clone()
    }
    fn session_config(&self) -> Vec<SessionConfigChoice> {
        self.config.clone()
    }
    fn decide_permission(&self, _request: &RequestPermissionRequest) -> PermissionDecision {
        self.decision
    }
    fn answer_extension(&self, method: &str, _params: &Value) -> Option<ExtensionAnswer> {
        (method == "fake/ask").then(|| ExtensionAnswer {
            reply: json!({"answer":"continue"}),
            events: vec![],
        })
    }
}

pub(super) fn request(dir: &Path, inbox: Option<ParentMessageInbox>) -> AcpSessionRequest {
    AcpSessionRequest {
        identity: RunArtifactIdentity {
            agent_id: "test".into(),
            agent_fingerprint: "test-fingerprint".into(),
            provider: "fake-acp".into(),
            model: None,
            runtime: AgentRuntime::Cursor,
            reasoning: None,
        },
        prompt: "task".into(),
        system_prompt: PromptPolicy::Extend(String::new()),
        output_file: dir.join("result.json"),
        cwd: dir.into(),
        cancellation: rho_tools::cancellation::RunCancellation::new(),
        status_tx: None,
        started_status: None,
        parent_messages: inbox,
        overrides: CliSessionOverrides::default(),
    }
}

pub(super) fn read_artifacts(dir: &Path) -> (RunStatus, Vec<AttachmentEvent>) {
    let status =
        serde_json::from_str(&std::fs::read_to_string(dir.join("result.json")).unwrap()).unwrap();
    let events = std::fs::read_to_string(dir.join(subagent::ATTACHMENT_FILE_NAME))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (status, events)
}

pub(super) fn terminal_events(events: &[AttachmentEvent]) -> Vec<AttachmentEvent> {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AttachmentEvent::Completed
                    | AttachmentEvent::Failed(_)
                    | AttachmentEvent::Cancelled
            )
        })
        .cloned()
        .collect()
}
