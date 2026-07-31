//! Installation and end-to-end behavior of the hook runtime.
#![cfg(unix)]

use std::time::Duration;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolSpec},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{Tool, ToolContext, ToolFuture, ToolInvocation, ToolOutput},
    CancellationToken, CapabilityRequest, CapabilitySource, PathScope, Rho, ScopedWorkspacePolicy,
    SessionOptions, Workspace,
};
use tempfile::TempDir;

use super::*;

/// Fixture workspace whose trusted project hook denies `bash` and whose post
/// hook records every successful edit.
struct Fixture {
    home: TempDir,
    project: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            home: TempDir::new().unwrap(),
            project: TempDir::new().unwrap(),
        };
        fixture.write_program(
            "deny-bash",
            r#"cat > /dev/null; echo '{"version":1,"decision":"deny","reason":"bash is off limits here"}'"#,
        );
        fixture.write_program(
            "record-edit",
            &format!(
                "cat >> {}\n",
                fixture.project.path().join("edits.log").display()
            ),
        );
        std::fs::write(
            fixture.project.path().join(".rho/hooks.toml"),
            r#"
version = 1

[[hook]]
id = "deny-bash"
on = "before_tool_use"
tools = ["bash"]
command = ["/bin/sh", "./.rho/hooks/deny-bash"]
timeout = "10s"

[[hook]]
id = "record-edit"
on = "after_tool_use"
tools = ["edit_file"]
command = ["/bin/sh", "./.rho/hooks/record-edit"]
timeout = "10s"
"#,
        )
        .unwrap();
        fixture
    }

    fn write_program(&self, name: &str, script: &str) {
        let directory = self.project.path().join(".rho/hooks");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    }

    fn catalog(&self, trust: ProjectTrust) -> HookCatalog {
        HookCatalog::discover(Some(self.home.path()), Some(self.project.path()), trust).unwrap()
    }

    fn edits(&self) -> String {
        std::fs::read_to_string(self.project.path().join("edits.log")).unwrap_or_default()
    }
}

/// Tool that asks for the capability its name implies, so it flows through the
/// real authorization path.
struct CapabilityTool {
    name: &'static str,
}

impl Tool for CapabilityTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.into(),
            description: "test tool".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        let root = context
            .workspace_root()
            .map(std::path::Path::to_path_buf)
            .expect("the fixture runtime has a workspace");
        Box::pin(async move {
            context
                .authorize(CapabilityRequest::read_path(
                    root.join("file.txt"),
                    PathScope::PrimaryWorkspace,
                    CapabilitySource::built_in_tool(self.name),
                ))
                .await
                .map_err(|error| rho_sdk::tool::ToolError::policy_denied(&error))?;
            Ok(ToolOutput::text("ok"))
        })
    }
}

async fn run_tool(fixture: &Fixture, trust: ProjectTrust, tool: &'static str) -> (String, String) {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                rho_sdk::model::ToolCall {
                    id: "call-1".into(),
                    name: tool.into(),
                    arguments: serde_json::json!({}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let builder = Rho::builder()
        .provider(provider)
        .tool(CapabilityTool { name: tool })
        .workspace(Workspace::new(fixture.project.path()).unwrap())
        .workspace_policy(ScopedWorkspacePolicy::new().allow_read_paths());
    let runtime = HookPipeline::start(fixture.catalog(trust), CancellationToken::new());
    let builder = match runtime.as_ref() {
        Some(hooks) => hooks.attach(builder),
        None => builder,
    };
    let rho = builder.build().unwrap();
    let session = rho.session(SessionOptions::default()).await.unwrap();
    session.complete("go").await.unwrap();
    let content = session
        .history()
        .into_iter()
        .find_map(|message| match message {
            rho_sdk::model::Message::ToolResult(result) => Some(result.content),
            _ => None,
        })
        .expect("a tool result was recorded");
    // Release the SDK runtime first: it owns the observational sender, and the
    // worker only finishes once that sender is gone.
    drop(session);
    drop(rho);
    let activity = match runtime {
        Some(runtime) => {
            let engine = std::sync::Arc::clone(runtime.engine());
            runtime.shutdown(Duration::from_secs(10)).await;
            engine
                .activity()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        }
        None => String::new(),
    };
    (content, activity)
}

#[tokio::test]
async fn a_trusted_project_hook_denies_the_tool_it_names() {
    let fixture = Fixture::new();

    let (content, activity) = run_tool(&fixture, ProjectTrust::Trusted, "bash").await;

    assert!(
        content.contains("bash is off limits here"),
        "the model must see why the call failed: {content}"
    );
    assert!(activity.contains("project:deny-bash before_tool_use denied"));
}

#[tokio::test]
async fn an_untrusted_project_hook_does_not_run() {
    let fixture = Fixture::new();

    let (content, activity) = run_tool(&fixture, ProjectTrust::Untrusted, "bash").await;

    assert_eq!(content, "ok");
    assert_eq!(activity, "");
}

#[tokio::test]
async fn a_post_hook_records_a_successful_edit() {
    let fixture = Fixture::new();

    let (content, activity) = run_tool(&fixture, ProjectTrust::Trusted, "edit_file").await;

    assert_eq!(content, "ok");
    assert!(activity.contains("project:record-edit after_tool_use observed"));
    let recorded = fixture.edits();
    assert!(
        recorded.contains("\"event\":\"after_tool_use\""),
        "the hook received the event envelope: {recorded}"
    );
    assert!(recorded.contains("\"name\":\"edit_file\""));
    assert!(recorded.contains("\"status\":\"succeeded\""));
}

#[tokio::test]
async fn a_hook_that_matches_no_tool_leaves_the_call_alone() {
    let fixture = Fixture::new();

    let (content, _) = run_tool(&fixture, ProjectTrust::Trusted, "read_file").await;

    assert_eq!(content, "ok");
}

#[test]
fn an_empty_catalog_starts_nothing() {
    assert!(HookPipeline::start(HookCatalog::default(), CancellationToken::new()).is_none());
}

#[tokio::test]
async fn a_runtime_rebuild_reuses_one_engine_and_worker() {
    let fixture = Fixture::new();
    let hooks = HookPipeline::start(
        fixture.catalog(ProjectTrust::Trusted),
        CancellationToken::new(),
    )
    .expect("the fixture configures hooks");

    // The interactive host rebuilds its SDK runtime on permission-mode changes;
    // each rebuild must share the engine rather than start a second worker.
    let mut rebuilds = Vec::new();
    for _ in 0..3 {
        let builder = hooks.attach(Rho::builder().provider(ScriptedProvider::new(
            ModelIdentity::new("scripted", "test", "model"),
            [],
        )));
        let rho = builder.build().unwrap();
        assert!(rho.hooks().is_enabled());
        rebuilds.push(rho);
    }

    // One engine, one activity log: whatever any rebuild records is visible here.
    assert!(hooks.engine().activity().is_empty());
    drop(rebuilds);
    hooks.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn a_delegated_run_reports_its_parent_session_to_hooks() {
    let fixture = Fixture::new();
    fixture.write_program(
        "record-run",
        &format!(
            "cat >> {}\n",
            fixture.project.path().join("runs.log").display()
        ),
    );
    std::fs::write(
        fixture.project.path().join(".rho/hooks.toml"),
        r#"
version = 1

[[hook]]
id = "record-run"
on = "run_completed"
command = ["/bin/sh", "./.rho/hooks/record-run"]
timeout = "10s"
"#,
    )
    .unwrap();
    let parent = rho_sdk::SessionId::from_string("parent-session").unwrap();
    let hooks = HookPipeline::start(
        fixture.catalog(ProjectTrust::Trusted),
        CancellationToken::new(),
    )
    .expect("the fixture configures hooks");
    let rho = hooks
        .attach(
            Rho::builder()
                .provider(ScriptedProvider::new(
                    ModelIdentity::new("scripted", "test", "model"),
                    [ScriptedTurn::completed(ModelResponse::Assistant(vec![
                        ContentBlock::Text("done".into()),
                    ]))],
                ))
                .workspace(Workspace::new(fixture.project.path()).unwrap())
                .hook_delegation(rho_sdk::hooks::HookDelegation::new(parent)),
        )
        .build()
        .unwrap();
    let session = rho.session(SessionOptions::default()).await.unwrap();
    session.complete("go").await.unwrap();

    drop(session);
    drop(rho);
    hooks.shutdown(Duration::from_secs(10)).await;

    let recorded =
        std::fs::read_to_string(fixture.project.path().join("runs.log")).unwrap_or_default();
    assert!(
        recorded.contains("\"parent_session_id\":\"parent-session\""),
        "a delegated run must attribute itself to its parent: {recorded}"
    );
    assert!(recorded.contains("\"event\":\"run_completed\""));
    assert!(recorded.contains("\"stop_reason\":\"end_turn\""));
}

#[tokio::test]
async fn the_host_reports_the_session_boundary_after_its_runs() {
    let fixture = Fixture::new();
    fixture.write_program(
        "record-session",
        &format!(
            "cat >> {}\n",
            fixture.project.path().join("session.log").display()
        ),
    );
    std::fs::write(
        fixture.project.path().join(".rho/hooks.toml"),
        r#"
version = 1

[[hook]]
id = "record-session"
on = "session_completed"
command = ["/bin/sh", "./.rho/hooks/record-session"]
timeout = "10s"
"#,
    )
    .unwrap();
    let hooks = HookPipeline::start(
        fixture.catalog(ProjectTrust::Trusted),
        CancellationToken::new(),
    )
    .expect("the fixture configures hooks");
    let rho = hooks
        .attach(
            Rho::builder()
                .provider(ScriptedProvider::new(
                    ModelIdentity::new("scripted", "test", "model"),
                    [ScriptedTurn::completed(ModelResponse::Assistant(vec![
                        ContentBlock::Text("done".into()),
                    ]))],
                ))
                .workspace(Workspace::new(fixture.project.path()).unwrap()),
        )
        .build()
        .unwrap();
    let session = rho.session(SessionOptions::default()).await.unwrap();
    session.complete("go").await.unwrap();

    rho.hooks().session_completed(session.id(), 1).await;
    drop(session);
    drop(rho);
    hooks.shutdown(Duration::from_secs(10)).await;

    let recorded =
        std::fs::read_to_string(fixture.project.path().join("session.log")).unwrap_or_default();
    assert!(
        recorded.contains("\"event\":\"session_completed\""),
        "the session boundary hook ran: {recorded}"
    );
    assert!(recorded.contains("\"runs\":1"));
}
