use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolCall},
    provider::{ScriptedProvider, ScriptedTurn},
    Rho, RunEvent, ScopedWorkspacePolicy, SessionOptions, ToolCompletion, UserInput, Workspace,
};
use serde_json::{json, Value};

use super::*;

// Covers: the model-facing tool must authorize the outside-workspace archive
// before it opens or builds a cache. The allowed path exercises real SDK dispatch.
// Owner: host tool / SDK capability contract, not the storage query tests.
#[tokio::test]
async fn archive_access_is_authorized_before_search_execution() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let prior = crate::session::Session::create_in_root(root.path(), cwd.path()).unwrap();
    prior
        .append_message(&Message::user_text("permissionneedle"))
        .unwrap();
    let original = std::fs::read(prior.path()).unwrap();
    for allow_archive in [false, true] {
        let binding = SessionBinding::default();
        let tool = Sessions {
            binding: binding.clone(),
            recall: Default::default(),
            max_output_bytes: rho_tools::DEFAULT_MAX_OUTPUT_BYTES,
            environment: Default::default(),
            root: Ok(root.path().to_path_buf()),
        };
        let provider = ScriptedProvider::new(
            ModelIdentity::new("scripted", "test", "model"),
            [
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                    ToolCall::new(
                        "search".into(),
                        "sessions".into(),
                        json!({"action":"search","query":"permissionneedle"}),
                    ),
                )])),
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "done".into(),
                )])),
            ],
        );
        let mut policy = ScopedWorkspacePolicy::new().allow_read_paths();
        if allow_archive {
            policy = policy.allow_outside_workspace_paths();
        }
        let runtime = Rho::builder()
            .provider(provider)
            .workspace(Workspace::new(cwd.path()).unwrap())
            .workspace_policy(policy)
            .tool(tool)
            .build()
            .unwrap();
        let session = runtime.session(SessionOptions::default()).await.unwrap();
        binding.bind(session.id().as_str());
        let mut run = session
            .start(UserInput::text("search prior evidence"))
            .await
            .unwrap();
        let mut result = None;
        while let Some(event) = run.next_event().await {
            if let RunEvent::ToolFinished {
                result: completion, ..
            } = event
            {
                result = Some(completion);
            }
        }
        run.outcome().await.unwrap();
        match (allow_archive, result.unwrap()) {
            (true, ToolCompletion::Completed(output)) => {
                let output: Value = serde_json::from_str(output.content()).unwrap();
                assert_eq!(output["sessions"][0]["id"], prior.id());
            }
            (false, ToolCompletion::Failure(error)) => {
                assert_eq!(error.kind(), ToolErrorKind::PolicyDenied);
                assert!(!root.path().join("search.sqlite3").exists());
            }
            (_, other) => panic!("unexpected tool outcome: {other:?}"),
        }
        assert_eq!(std::fs::read(prior.path()).unwrap(), original);
        runtime.shutdown();
    }
}

// Covers: the core's argument schemas merge into the tool's as standalone
// schemas: an argument whose type the core defines once, such as `list`'s
// `state`, carries that definition instead of a `$ref` to a `$defs` the
// merged schema does not have, which providers reject (#1714).
// Owner: sessions tool schema, generated from the session-control core.
#[test]
fn control_arguments_carry_their_definitions() {
    let schema = input_schema();
    let state = &schema["properties"]["state"];

    assert_eq!(
        (
            schema.to_string().contains("$ref"),
            // The job states, as `enum` or `oneOf` of `const`s.
            state.to_string().contains(r#""waiting_for_input""#),
        ),
        (false, true),
        "state: {state}"
    );
}

// Covers: an argument a model fills with `null`, as models do with the flat
// schema's unused arguments, counts as absent for every action: a history
// call is not refused for a control argument it does not take, and a `read`
// with a `null` anchor reads the live session, not history (#1714).
// Owner: sessions tool's argument routing.
#[test]
fn null_arguments_count_as_absent_for_every_action() {
    let route = |arguments: Value| match Action::parse(arguments) {
        Ok(Action::Index(_)) => "history",
        Ok(Action::Recall(_)) => "recall",
        Ok(Action::Control(_)) => "control",
        Err(_) => "refused",
    };
    let nulls = json!({
        "workspace": null, "prompt": null, "timeout_secs": null, "cursor": null,
        "anchor": null, "scope": null, "recall_id": null,
    });
    let with_nulls = |call: Value| {
        let mut call = call;
        for (name, null) in nulls.as_object().unwrap() {
            call.as_object_mut()
                .unwrap()
                .entry(name.clone())
                .or_insert(null.clone());
        }
        call
    };

    let routes: Vec<&str> = [
        json!({"action": "search", "query": "q"}),
        json!({"action": "recall", "recall_id": "r"}),
        json!({"action": "read", "session": "s", "anchor": "a"}),
        json!({"action": "read", "session": "s"}),
    ]
    .into_iter()
    .map(|call| route(with_nulls(call)))
    .collect();

    assert_eq!(routes, ["history", "recall", "history", "control"]);
}
