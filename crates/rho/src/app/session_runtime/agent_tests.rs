use std::path::Path;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{Message, ModelIdentity},
    UserInput,
};

use super::{loaded_agent, loaded_agent_with, AgentTask, MODEL_OVERRIDE_METADATA};
use crate::{
    agent::{AgentCapabilities, AgentCatalog, ToolCapability},
    app::session_runtime::{
        test_support::{idle_runtime, root_agent},
        CommandRejected,
    },
    config::Config,
    permission::PermissionMode,
    session::Session as StoredSession,
};

fn launch_config() -> Config {
    Config {
        provider: "test".into(),
        model: "launch-model".into(),
        permission_mode: PermissionMode::Bypass,
        ..Config::default()
    }
}

// Covers: a reloaded child must come back with its delegated tool boundary,
// never the root's, even when it shares the launch agent's id; a root session
// stays the launch agent; a changed definition fails the load.
// Owner: session runtime load agent selection.
#[test]
fn load_restores_the_child_binding_or_fails() {
    let cwd = tempfile::tempdir().unwrap();
    let agents = cwd.path().join(".agents/agents");
    std::fs::create_dir_all(&agents).unwrap();
    let config = launch_config();
    let launch_agent = root_agent(&config);
    // Same id as the launch agent, so only the persisted role tells them apart.
    let file = agents.join(format!("{}.md", launch_agent.id()));
    std::fs::write(
        &file,
        "---\ndescription: Loaded child\ntools: [read_file, agent]\n---\nInspect files\n",
    )
    .unwrap();
    let catalog = || {
        Ok(AgentCatalog::discover_with_home(
            cwd.path(),
            /*home*/ None,
        )?)
    };
    let definitions = catalog().unwrap();
    let definition = &definitions
        .find(launch_agent.id().as_str())
        .unwrap()
        .definition;
    let session = |root: &Path| {
        StoredSession::create_in_root_with_agent(
            root,
            cwd.path(),
            launch_agent.id().as_str(),
            &launch_agent.fingerprint().to_string(),
        )
        .unwrap()
    };
    let child_root = tempfile::tempdir().unwrap();
    let child = session(child_root.path());
    child
        .rebind_fresh_delegated_agent(
            launch_agent.id().as_str(),
            &definition.fingerprint().to_string(),
            /*initial_snapshot*/ None,
        )
        .unwrap();

    let (agent, _) = loaded_agent(&launch_agent, &child, config.clone(), catalog).unwrap();
    assert_eq!(agent.fingerprint(), definition.fingerprint());
    assert_eq!(
        agent.rho_capabilities(),
        Some(&AgentCapabilities::new([ToolCapability::ReadFile].into()))
    );

    let root_dir = tempfile::tempdir().unwrap();
    let root = session(root_dir.path());
    let (agent, _) = loaded_agent(&launch_agent, &root, config.clone(), catalog).unwrap();
    assert_eq!(agent.fingerprint(), launch_agent.fingerprint());

    std::fs::write(
        &file,
        "---\ndescription: Loaded child\ntools: [read_file, write_file]\n---\nInspect files\n",
    )
    .unwrap();
    let snapshot = child
        .snapshot_for_resume(
            ModelIdentity::new("test", "test", "call-model"),
            "cache".into(),
        )
        .unwrap()
        .with_metadata(
            MODEL_OVERRIDE_METADATA,
            r#"{"provider":"test","model":"call-model","auth":"test-auth"}"#,
        );
    child.save_snapshot(&snapshot, &[]).unwrap();
    assert!(
        loaded_agent_with(&launch_agent, &child, config, catalog, |_, _, _, _| panic!(
            "changed definitions must fail before override binding"
        ),)
        .is_err()
    );
}

// Covers: rebinding must not silently discard a conversation, including a
// loaded conversation whose completed-runs counter starts at zero.
// Owner: session runtime agent rebind.
#[tokio::test]
async fn rebind_rejects_nonfresh_sessions_without_replacing_them() {
    let cwd = tempfile::tempdir().unwrap();
    let config = launch_config();
    for (completed_runs, history) in [
        (0, vec![Message::user_text("retained conversation")]),
        (1, Vec::new()),
    ] {
        let mut runtime = idle_runtime(cwd.path(), &config).await;
        runtime.completed_runs = completed_runs;
        for message in history {
            runtime.built.session.append_message(message).unwrap();
        }
        let before = runtime.built.session.snapshot();
        let input = UserInput::text("task");
        let error = runtime
            .rebind_agent_with(
                &AgentTask {
                    agent_id: "child".into(),
                    model_override: None,
                    input,
                },
                &config,
                |_| panic!("nonfresh sessions must not rebind"),
                |_, _, _| async { panic!("nonfresh sessions must not rebuild") },
            )
            .await
            .unwrap_err();
        assert!(error.downcast_ref::<CommandRejected>().is_some());
        assert_eq!(runtime.built.session.snapshot(), before);
        runtime.shutdown().await;
    }
}
