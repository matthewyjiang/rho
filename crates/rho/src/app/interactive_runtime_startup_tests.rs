use super::*;
use crate::app::interactive_runtime::{InteractiveRuntimeOptions, StartupHydrateChanges};
use pretty_assertions::assert_eq;

/// Exercise production startup scheduling while controlling only the catalog's
/// asynchronous completion. No credentials, provider requests, or catalog fetch.
pub(crate) async fn test_startup_runtime(
    cwd: &std::path::Path,
    storage: Option<StoredSession>,
    catalog_names: impl std::future::Future<Output = usize> + Send + 'static,
) -> InteractiveRuntime {
    let config = Config::default();
    let agent = crate::app::agent_binding::AgentBinder::bind(
        Arc::new(crate::agent::AgentDefinition {
            prompt: crate::agent::PromptPolicy::Replace("startup fixture".into()),
            ..test_bound_agent().definition().clone()
        }),
        crate::app::agent_binding::AgentInvocation {
            role: crate::app::agent_binding::AgentRole::InteractiveRoot,
            available_tools: AgentCapabilities::all_host_tools(),
        },
        &config,
    )
    .unwrap();
    super::super::startup::initialize(
        InteractiveRuntimeOptions {
            config: &config,
            catalog: None,
            config_path: cwd.join("config.toml"),
            cwd: cwd.to_path_buf(),
            no_system_prompt: false,
            no_tools: true,
            no_subagents: true,
            questionnaire_enabled: false,
            history: Vec::new(),
            session_id: storage.as_ref().map(|stored| stored.id().to_string()),
            storage,
            diagnostics: RuntimeDiagnostics::new(&config),
            agent,
            unavailable_error: Some(rho_providers::model::ModelError::MissingCredentials(
                "startup fixture",
            )),
            launch_added_dirs: Default::default(),
        },
        catalog_names,
    )
    .await
    .unwrap()
}

// Covers: fresh and resumed startup must observe catalog completion even when
// the prompt stays unchanged or frozen, without rewriting retained history.
// Owner: interactive startup scheduling. PTY cannot gate catalog completion
// independently of a complete selected metadata row without a production hook.
#[tokio::test]
async fn startup_schedules_catalog_completion_independently_of_prompt_rewrites() {
    for resumed in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let storage = resumed.then(|| {
            let stored = StoredSession::create_in_root(home.path(), home.path()).unwrap();
            stored
                .append_message(&Message::user_text("retained history"))
                .unwrap();
            stored
        });
        let (release, catalog) = tokio::sync::oneshot::channel();
        let (finished, completion) = tokio::sync::oneshot::channel();
        let mut runtime = test_startup_runtime(home.path(), storage, async move {
            catalog.await.unwrap();
            // Current-thread executor finishes the task before observing this.
            finished.send(()).unwrap();
            0
        })
        .await;
        let history = runtime.history();
        assert_eq!(runtime.may_rewrite_startup_prompt, !resumed);
        assert!(runtime.startup_hydrate_pending());
        assert!(!runtime.startup_hydrate_ready());
        assert_eq!(
            runtime.poll_startup_hydrates().await.unwrap(),
            StartupHydrateChanges::default()
        );

        release.send(()).unwrap();
        completion.await.unwrap();
        assert!(runtime.startup_hydrate_ready());
        assert_eq!(
            runtime.poll_startup_hydrates().await.unwrap(),
            StartupHydrateChanges {
                catalog_completed: true,
                ..Default::default()
            }
        );
        assert!(!runtime.startup_hydrate_pending());
        assert_eq!(
            runtime.poll_startup_hydrates().await.unwrap(),
            StartupHydrateChanges::default()
        );
        assert_eq!(runtime.history(), history);
        runtime.shutdown().await;
    }
}
