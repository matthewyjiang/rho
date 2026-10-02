use std::path::Path;

use pretty_assertions::assert_eq;
use rho_sdk::model::Message;

use super::{recorded_changes, InteractiveRuntime};
use crate::agent::{AdvertisedAgents, AdvertisedChange, AgentCatalog};
use crate::app::interactive_runtime::test_runtime;

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

fn corrections(runtime: &InteractiveRuntime) -> Vec<Vec<AdvertisedChange>> {
    runtime
        .sessions
        .history()
        .iter()
        .filter_map(recorded_changes)
        .collect()
}

// Covers: the baseline is the tool spec plus corrections already in history,
// so a repeat save adds nothing, external edits fold into the next correction,
// and a correction lost from history (compaction, rewind) is sent again.
// Owner: interactive runtime context. PTY scenarios make one change each and
// cannot drive history loss deterministically.
#[tokio::test]
async fn sync_appends_only_what_history_has_not_told_the_model() {
    let home = tempfile::tempdir().expect("home");
    write_agent(home.path(), "edited", "before");
    let mut runtime = test_runtime(Vec::new()).await;
    runtime
        .tools
        .set_advertised_agents_for_tests(AdvertisedAgents::from_catalog(&catalog(home.path())));

    write_agent(home.path(), "edited", "after");
    assert!(runtime
        .sync_agent_catalog(&catalog(home.path()))
        .unwrap()
        .is_some());
    assert_eq!(
        runtime.sync_agent_catalog(&catalog(home.path())).unwrap(),
        None
    );

    write_agent(home.path(), "external", "added outside /agents");
    runtime.sync_agent_catalog(&catalog(home.path())).unwrap();
    let edited = AdvertisedChange::Description {
        agent_id: "edited".into(),
        previous: "before".into(),
        description: "after".into(),
    };
    let external = AdvertisedChange::Available {
        agent_id: "external".into(),
        description: "added outside /agents".into(),
    };
    assert_eq!(
        corrections(&runtime),
        vec![vec![edited.clone()], vec![external.clone()]]
    );

    runtime
        .sessions
        .session()
        .replace_history(Vec::new())
        .unwrap();
    runtime.sync_agent_catalog(&catalog(home.path())).unwrap();
    assert_eq!(corrections(&runtime), vec![vec![edited, external]]);
}

// Covers: a user prompt that merely starts with the notice prefix is not
// mistaken for a correction the model already has.
// Owner: interactive runtime context.
#[test]
fn only_exact_correction_messages_are_recorded_changes() {
    let correction = "[agent catalog updated]\n[{\"change\":\"unavailable\",\"agent_id\":\"x\"}]";
    assert_eq!(
        recorded_changes(&Message::user_text(correction)),
        Some(vec![AdvertisedChange::Unavailable {
            agent_id: "x".into()
        }])
    );
    assert_eq!(
        recorded_changes(&Message::user_text(
            "[agent catalog updated]\nplease ignore agent x"
        )),
        None
    );
}
