use std::{path::Path, sync::Arc};

use pretty_assertions::assert_eq;

use super::recorded_changes;
use crate::{
    agent::{AdvertisedAgents, AdvertisedChange, AgentCatalog},
    app::session_runtime::{unattended_runtime_on, HangUntilCancel, QuestionSupport},
    config::Config,
};

fn write_agent(home: &Path, id: &str, description: &str) {
    let dir = home.join(".rho/agents");
    std::fs::create_dir_all(&dir).expect("agents dir");
    std::fs::write(
        dir.join(format!("{id}.md")),
        format!("---\ndescription: {description}\n---\nbody\n"),
    )
    .expect("write agent");
}

fn catalog(home: &Path) -> AgentCatalog {
    AgentCatalog::discover_with_home(home, Some(home)).expect("catalog")
}

// Covers: a daemon session tells the model only what it has not been told
// about its agents: the tool spec plus the corrections in history is the
// baseline, so a repeat adds nothing and a later edit adds only itself.
// Owner: session runtime context.
#[tokio::test]
async fn the_model_is_told_each_agent_change_once() {
    let (home, root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_agent(home.path(), "edited", "before");
    let provider: Arc<dyn rho_sdk::provider::ModelProvider> = Arc::new(HangUntilCancel);
    let sdk = rho_sdk::Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .build()
        .unwrap();
    let mut runtime = unattended_runtime_on(
        root.path(),
        &Config::default(),
        QuestionSupport::Unavailable,
        sdk,
        provider,
    )
    .await;
    runtime
        .built
        .tools
        .set_advertised_agents_for_tests(AdvertisedAgents::from_catalog(&catalog(home.path())));

    write_agent(home.path(), "edited", "after");
    let told = runtime.tell_agent_catalog(&catalog(home.path())).unwrap();
    let repeated = runtime.tell_agent_catalog(&catalog(home.path())).unwrap();
    write_agent(home.path(), "external", "added outside /agents");
    runtime.tell_agent_catalog(&catalog(home.path())).unwrap();

    let corrections: Vec<_> = runtime
        .history_for_tests()
        .iter()
        .filter_map(recorded_changes)
        .collect();
    assert_eq!(
        (told.is_some(), repeated, corrections),
        (
            true,
            None,
            vec![
                vec![AdvertisedChange::Description {
                    agent_id: "edited".into(),
                    previous: "before".into(),
                    description: "after".into(),
                }],
                vec![AdvertisedChange::Available {
                    agent_id: "external".into(),
                    description: "added outside /agents".into(),
                }],
            ],
        )
    );
}
