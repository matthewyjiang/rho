use super::*;
use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ToolCall, ToolSpec},
    tool::{ScriptedTool, ScriptedToolOutcome, ToolOutput},
};
use serde_json::json;

use crate::{config::CodemodeMode, tools::sdk_registry::StaticToolBundle};

const MCP_TOOL: &str = "mcp__docs__lookup";

async fn search_runtime(storage: StoredSession) -> (InteractiveRuntime, ScriptedProvider) {
    let mut interactive = test_runtime(Vec::new()).await;
    interactive.config.codemode.mode = CodemodeMode::Only;
    interactive.compaction.auto_compact = false;
    interactive.tools = AppToolSet::new(
        &interactive.config,
        interactive.diagnostics.clone(),
        ToolSetOptions::default(),
    );
    interactive
        .tools
        .add_bundle(StaticToolBundle::new(vec![Arc::new(ScriptedTool::new(
            ToolSpec {
                name: MCP_TOOL.into(),
                description: "look up documentation".into(),
                input_schema: json!({"type": "object"}),
            },
            ScriptedToolOutcome::Success(ToolOutput::text("docs")),
        ))]));
    let text = || {
        ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
            "done".into(),
        )]))
    };
    let provider = ScriptedProvider::new(
        interactive.provider_identity(),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "search".into(),
                    name: "tool_search".into(),
                    arguments: json!({"query": MCP_TOOL}),
                },
            )])),
            text(),
            text(),
            text(),
            text(),
        ],
    );
    interactive.provider = ProviderController::new(
        Arc::new(provider.clone()),
        rho_sdk::ReasoningLevel::Off,
        Default::default(),
    );
    let session = interactive
        .runtime
        .session(SessionOptions::new().id(SessionId::from_string(storage.id()).unwrap()))
        .await
        .unwrap();
    interactive.sessions = InteractiveSessionController::new(
        session,
        Some(storage),
        interactive.tools.web_access().clone(),
        /*recall*/ None,
        /*advisor*/ None,
    )
    .with_code_mode(interactive.tools.code_mode().clone());
    interactive.rebind_current_session().await.unwrap();
    interactive.sessions.save_snapshot(&[]).unwrap();
    (interactive, provider)
}

// Covers: searches must not leak schemas across /new, resume, or tree selection;
// ordinary runtime rebuilds retain them, and returning to a saved branch restores them.
// Owner: interactive session snapshot and conversation lifecycle.
#[tokio::test]
async fn loaded_tools_follow_the_selected_conversation() {
    #[derive(Debug)]
    enum Switch {
        New,
        Resume,
        Tree,
    }

    for switch in [Switch::New, Switch::Resume, Switch::Tree] {
        let root = tempfile::tempdir().unwrap();
        let cwd = std::env::current_dir().unwrap();
        let storage = StoredSession::create_in_root(root.path(), &cwd).unwrap();
        let (mut interactive, provider) = search_runtime(storage.clone()).await;
        let before_search = crate::session::tree::SessionTree::load(storage.path())
            .unwrap()
            .active_leaf_id()
            .unwrap()
            .clone();
        interactive
            .start(UserInput::text("load a tool"), None)
            .await
            .unwrap();
        interactive.finish_run().await.unwrap();
        let loaded_branch = crate::session::tree::SessionTree::load(storage.path())
            .unwrap()
            .active_leaf_id()
            .unwrap()
            .clone();

        interactive.rebind_current_session().await.unwrap();
        interactive
            .start(UserInput::text("inspect after rebuild"), None)
            .await
            .unwrap();
        interactive.finish_run().await.unwrap();
        match switch {
            Switch::New => {
                interactive.reset().await.unwrap();
            }
            Switch::Resume => {
                let other = StoredSession::create_in_root(root.path(), &cwd).unwrap();
                interactive.resume(other).await.unwrap();
            }
            Switch::Tree => {
                interactive
                    .select_tree_node(storage.clone(), &before_search)
                    .await
                    .unwrap();
            }
        }
        interactive
            .start(UserInput::text("inspect destination"), None)
            .await
            .unwrap();
        interactive.finish_run().await.unwrap();
        interactive
            .select_tree_node(storage, &loaded_branch)
            .await
            .unwrap();
        interactive
            .start(UserInput::text("inspect restored branch"), None)
            .await
            .unwrap();
        interactive.finish_run().await.unwrap();

        let advertised: Vec<_> = provider
            .recorded_requests()
            .iter()
            .map(|request| request.tools.iter().any(|spec| spec.name == MCP_TOOL))
            .collect();
        assert_eq!(
            advertised,
            vec![false, true, true, false, true],
            "{switch:?}"
        );
        interactive.shutdown().await;
    }
}
