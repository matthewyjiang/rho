use super::*;
use crate::tools::{
    mcp::{
        report::{ConnectedServerReport, McpLiveServerState},
        McpConnectOutcome, McpServerReport, McpSessionReport, McpTransportSummary,
    },
    openapi::{bundle::OpenApiLoadOutcome, report::LoadedSource},
};
use pretty_assertions::assert_eq;

// Covers: deferred loading after prompt freeze supplies the same stable MCP and
// OpenAPI context as startup, even when only live_context_warm prevents a
// rewrite.
// Owner: interactive runtime MCP hydration (session history seam, not UI copy).
#[tokio::test]
async fn frozen_prompt_receives_connected_context_as_a_notice() {
    for (may_rewrite, warm) in [(false, false), (true, true)] {
        let mut runtime = super::super::tests::test_runtime(Vec::new()).await;
        runtime.may_rewrite_startup_prompt = may_rewrite;
        runtime.live_context_warm = warm;
        let report = McpSessionReport {
            servers: vec![McpServerReport::connected(ConnectedServerReport {
                identity: "docs".into(),
                transport: McpTransportSummary::Stdio {
                    command: "fixture".into(),
                    args: Vec::new(),
                },
                tools: Vec::new(),
                instructions: Some("server guidance".into()),
                live: McpLiveServerState::default(),
                filtered_out_count: 0,
                collision_skipped_count: 0,
            })],
            ..Default::default()
        };
        let openapi = OpenApiReport {
            sources: vec![LoadedSource {
                name: "github".into(),
                read_operations: 2,
                tags: vec!["repos".into()],
            }],
            ..Default::default()
        };
        let expected = prompt::tool_sources_context(&report, &openapi);
        let loaded = openapi.clone();
        let system_before = runtime.sessions.prompt.system.clone();
        let before = runtime.history().len();
        let (finished, completion) = tokio::sync::oneshot::channel();
        runtime.pending_tool_sources = Some(tokio::spawn(async move {
            finished.send(()).unwrap();
            ToolSourcesOutcome {
                mcp: McpConnectOutcome {
                    report,
                    bundle: None,
                    catalog: Default::default(),
                },
                openapi: OpenApiLoadOutcome {
                    bundle: None,
                    report: loaded,
                },
            }
        }));
        completion.await.unwrap();
        assert_eq!(
            runtime.poll_startup_hydrates().await.unwrap(),
            StartupHydrateChanges {
                tool_inventory_changed: true,
                openapi: Some(openapi.clone()),
                ..Default::default()
            }
        );
        assert_eq!(runtime.sessions.prompt.system, system_before);
        assert_eq!(
            &runtime.history()[before..],
            &[rho_sdk::model::Message::user_text(expected)]
        );
    }
}
