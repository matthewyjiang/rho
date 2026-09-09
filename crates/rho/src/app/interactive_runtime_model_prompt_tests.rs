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
    let mut runtime = test_runtime(Vec::new()).await;
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

fn next_provider() -> Arc<dyn ModelProvider> {
    Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "next"),
        Vec::new(),
    ))
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
    let prompt = runtime.system_prompt.clone();
    assert!(runtime
        .replace_provider(next_provider(), rho_sdk::ReasoningLevel::Low, "test")
        .is_err());
    assert!(runtime.resume(stored).await.is_err());
    assert!(runtime.reset().await.is_err());
    assert_eq!(runtime.sessions.snapshot(), before);
    assert_eq!(runtime.system_prompt, prompt);
    assert_eq!(runtime.model_prompt, None);
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
    let selected = runtime.model_prompt.clone().unwrap();
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
    assert_eq!(runtime.model_prompt, expected.model_prompt);
    assert_eq!(runtime.sessions.take_notices().len(), 1);
    assert_ne!(
        runtime.model_prompt.as_ref().unwrap().sha256,
        selected.sha256
    );
}
