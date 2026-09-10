//! Lifecycle contracts use a private prompt home and scripted providers. The
//! PTY scenario owns picker interaction; these tests inspect provider history.

use super::*;
use crate::{model_identity::PromptModel, prompt};
use pretty_assertions::assert_eq;

async fn configured_runtime() -> (
    InteractiveRuntime,
    tempfile::TempDir,
    std::path::PathBuf,
    StoredSession,
) {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".rho/model-prompts");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("selected.md");
    std::fs::write(
        &path,
        "---\nprovider: test\nmodel: next\n---\nfirst overlay",
    )
    .unwrap();
    let mut runtime = advisor_test_runtime().await;
    let running = PromptModel::from_sdk_identity(&runtime.provider.provider().identity());
    let template = prompt::system_prompt_template_with_home_and_models(
        &[],
        home.path(),
        Some(home.path()),
        prompt::PromptModels {
            running: &running,
            advisor: None,
        },
    );
    let built = template.build(&running).unwrap();
    runtime.prompt_template = Some(template);
    crate::app::conversation_switch::replace_system_prompt(runtime.sessions.session(), &built.text)
        .unwrap();
    runtime.adopt_model_prompt(built);
    runtime
        .sessions
        .session()
        .append_message(Message::user_text("retained history"))
        .unwrap();
    let stored = StoredSession::create_in_root(home.path(), home.path()).unwrap();
    let session = runtime
        .runtime
        .session(
            SessionOptions::new()
                .id(SessionId::from_string(stored.id()).unwrap())
                .history(runtime.history()),
        )
        .await
        .unwrap();
    runtime.sessions.replace_session(session, None);
    (runtime, home, path, stored)
}

// Covers: recovery after a failed save restores the durable prompt, not the
// edited prompt loaded on resume. No provider/PTY interaction owns this state.
// Owner: interactive lifecycle persistence
#[tokio::test]
async fn durable_recovery_restores_prompt_and_advisor_without_reading_files() {
    for captured in [false, true] {
        let (mut runtime, _home, path, stored) = configured_runtime().await;
        runtime.attach_storage(stored.clone());
        runtime
            .replace_provider(next_provider(), rho_sdk::ReasoningLevel::Low, "test")
            .unwrap();
        let durable = runtime.sessions.snapshot();
        let sources = runtime.diagnostics.prompt_sources();
        std::fs::write(
            &path,
            "---\nprovider: test\nmodel: next\n---\nedited overlay",
        )
        .unwrap();
        runtime.resume(stored.clone()).await.unwrap();
        assert_ne!(runtime.history(), durable.history());
        std::fs::remove_file(path).unwrap();
        let checkpoint = captured.then(|| (stored.clone(), durable.clone()));
        // Fail persistence, then expose the unchanged durable leaf again.
        // Both recovery sources run without Unix permission assumptions,
        // sleep timing, or a provider request.
        let backup = stored.path().with_extension("backup");
        std::fs::rename(stored.path(), &backup).unwrap();
        std::fs::create_dir(stored.path()).unwrap();
        assert!(runtime.sessions.save_snapshot(&[]).is_err());
        std::fs::remove_dir(stored.path()).unwrap();
        std::fs::rename(backup, stored.path()).unwrap();
        runtime.restore_durable_session(checkpoint).await.unwrap();
        assert_eq!(runtime.history(), durable.history());
        assert_eq!(runtime.sessions.snapshot().metadata(), durable.metadata());
        let Message::System(text) = &durable.history()[0] else {
            panic!("missing prompt")
        };
        assert_eq!(
            runtime.active_system_prompt(),
            rho_sdk::SystemPrompt::Custom(text.clone())
        );
        assert_eq!(
            runtime.tools.advisor().unwrap().system_prompt(),
            Some(text.clone())
        );
        assert_eq!(runtime.diagnostics.prompt_sources(), sources);
        assert!(!runtime.may_rewrite_startup_prompt);
        assert_eq!(runtime.sessions.prompt.loaded, None);
        // Snapshot options suppress the builder prompt. Check the rebuilt
        // runtime policy too, not just the restored visible history.
        let fresh = runtime
            .runtime
            .session(SessionOptions::new())
            .await
            .unwrap();
        assert_eq!(fresh.history(), vec![Message::System(text.clone())]);
    }
}

fn next_provider() -> Arc<dyn ModelProvider> {
    Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "next"),
        Vec::new(),
    ))
}

// Covers: recovering saved history must not replace a launch-owned whole-prompt
// policy; /new must still honor --no-system-prompt or an explicit replacement.
// Owner: interactive lifecycle persistence and reset
#[tokio::test]
async fn durable_recovery_preserves_launch_owned_prompt_policy() {
    for system in [
        SystemPrompt::None,
        SystemPrompt::Custom("pinned replacement".into()),
    ] {
        let (mut runtime, _home, _path, stored) = configured_runtime().await;
        runtime.prompt_template = None;
        runtime.sessions.prompt =
            crate::app::active_prompt::ActivePrompt::new(system.clone(), None, Vec::new());
        runtime.attach_storage(stored);
        runtime.sessions.save_snapshot(&[]).unwrap();
        let saved_history = runtime.history();

        runtime.restore_durable_session(None).await.unwrap();
        assert_eq!(runtime.history(), saved_history);
        runtime.reset().await.unwrap();
        let expected = match system {
            SystemPrompt::None => Vec::new(),
            SystemPrompt::Custom(text) => vec![Message::System(text)],
            _ => unreachable!("test cases use known prompt policies"),
        };
        assert_eq!(runtime.history(), expected);
    }
}

// Covers: a model-switch notice save can fail after prompt replacement. Its
// staged prompt and dependent state must roll back with provider history.
// Owner: interactive model-switch transaction
#[tokio::test]
async fn failed_model_prompt_switch_save_restores_active_state() {
    let (mut runtime, _home, _path, stored) = configured_runtime().await;
    runtime.attach_storage(stored);
    runtime.sessions.save_snapshot(&[]).unwrap();
    let before = runtime.sessions.snapshot();
    let prompt = runtime.sessions.prompt.clone();
    let advisor_prompt = runtime.tools.advisor().unwrap().system_prompt();
    let sources = runtime.diagnostics.prompt_sources();
    runtime.may_rewrite_startup_prompt = true;
    crate::app::interactive_runtime::advisor::fail_next_advisor_switch_notice_for_tests();
    assert!(runtime
        .replace_provider(next_provider(), rho_sdk::ReasoningLevel::Low, "test")
        .is_err());
    let after = runtime.sessions.snapshot();
    // Rollback restores conversation state, not the monotonic SDK revision.
    assert_eq!(
        (after.history(), after.metadata(), after.provider()),
        (before.history(), before.metadata(), before.provider()),
    );
    assert_eq!(runtime.sessions.prompt, prompt);
    assert_eq!(
        runtime.tools.advisor().unwrap().system_prompt(),
        advisor_prompt
    );
    assert_eq!(runtime.diagnostics.prompt_sources(), sources);
    assert!(runtime.may_rewrite_startup_prompt);
}

// Covers: invalid files must fail before changing provider, reasoning, history,
// or the cached selection, both for model switches and resumed sessions.
// Owner: interactive lifecycle transaction
#[tokio::test]
async fn invalid_model_prompt_leaves_switch_and_resume_untouched() {
    let (mut runtime, _home, path, stored) = configured_runtime().await;
    stored
        .save_snapshot(&runtime.sessions.snapshot(), &[])
        .unwrap();
    std::fs::write(path, "---\nmode: invalid\n---\ninvalid").unwrap();
    let before = runtime.sessions.snapshot();
    let prompt = runtime.active_system_prompt();
    assert!(runtime
        .replace_provider(next_provider(), rho_sdk::ReasoningLevel::Low, "test")
        .is_err());
    assert!(runtime.resume(stored).await.is_err());
    assert!(runtime.reset().await.is_err());
    assert_eq!(runtime.sessions.snapshot(), before);
    assert_eq!(runtime.active_system_prompt(), prompt);
    assert_eq!(runtime.sessions.prompt.provenance, None);
}

// Covers: switching replaces one leading prompt, saves provenance, and resume
// reloads changed bytes instead of replaying the saved or cached overlay.
// Owner: interactive lifecycle persistence
#[tokio::test]
async fn model_prompt_switch_and_resume_replace_history_and_persist_provenance() {
    let (mut runtime, _home, path, stored) = configured_runtime().await;
    runtime.attach_storage(stored.clone());
    runtime
        .replace_provider(next_provider(), rho_sdk::ReasoningLevel::Low, "test")
        .unwrap();
    let selected = runtime.sessions.prompt.loaded.clone().unwrap();
    let saved = stored
        .snapshot_for_resume(runtime.provider.provider().identity(), "cache".into())
        .unwrap();
    assert_eq!(
        saved.metadata().get("rho.model_prompt"),
        Some(&crate::app::model_prompt_metadata::encode(Some(&selected)))
    );
    let old_history = runtime.history();
    std::fs::write(
        &path,
        "---\nprovider: test\nmodel: next\nmode: replace\n---\nsecond overlay",
    )
    .unwrap();
    runtime.resume(stored).await.unwrap();
    let running = PromptModel::from_sdk_identity(&runtime.provider.provider().identity());
    let expected = runtime
        .prompt_template
        .as_ref()
        .unwrap()
        .build(&running)
        .unwrap();
    let mut expected_history = old_history;
    expected_history[0] = Message::System(expected.text);
    assert_eq!(runtime.history(), expected_history);
    assert_eq!(runtime.sessions.prompt.loaded, expected.model_prompt);
    assert_eq!(runtime.sessions.take_notices().len(), 1);
    assert_ne!(
        runtime.sessions.prompt.provenance.as_ref().unwrap().sha256,
        selected.sha256
    );
}
