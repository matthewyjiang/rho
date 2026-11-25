use pretty_assertions::assert_eq;

use super::{AdvertisedAgents, AdvertisedChange};
use crate::agent::AgentCatalog;

fn advertised(entries: &[(&str, &str)]) -> AdvertisedAgents {
    AdvertisedAgents(
        entries
            .iter()
            .map(|(id, description)| ((*id).to_string(), (*description).to_string()))
            .collect(),
    )
}

// Covers: the parent is told exactly which advertised fields changed, and
// replaying those corrections reconstructs the new catalog (resume relies on it).
// Owner: agent catalog advertisement.
#[test]
fn changes_describe_and_replay_catalog_edits() {
    let before = advertised(&[("kept", "same"), ("gone", "old"), ("edited", "before")]);
    let after = advertised(&[("kept", "same"), ("edited", "after"), ("new", "fresh")]);

    let changes = before.changes_to(&after);

    assert_eq!(
        changes,
        vec![
            AdvertisedChange::Description {
                agent_id: "edited".into(),
                previous: "before".into(),
                description: "after".into(),
            },
            AdvertisedChange::Unavailable {
                agent_id: "gone".into(),
            },
            AdvertisedChange::Available {
                agent_id: "new".into(),
                description: "fresh".into(),
            },
        ]
    );
    let mut replayed = before.clone();
    for change in &changes {
        replayed.apply(change);
    }
    assert_eq!(replayed, after);
    assert_eq!(after.changes_to(&after), Vec::new());
}

// Covers: the built-in lead is a session's own agent and is not offered for
// delegation, but a user's own agent under that id still is, as it was
// before rho shipped one.
// Owner: agent catalog advertisement.
#[test]
fn the_built_in_lead_is_not_advertised_but_a_users_own_is() {
    let root = tempfile::tempdir().unwrap();
    let advertised = |catalog: &AgentCatalog| {
        AdvertisedAgents::from_catalog(catalog)
            .iter()
            .find(|(id, _)| *id == "lead")
            .map(|(_, description)| description.to_owned())
    };

    let built_in = AgentCatalog::discover_with_home(root.path(), Some(root.path())).unwrap();
    assert_eq!(advertised(&built_in), None);

    let agents = root.path().join(".rho/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join("lead.md"), "---\ndescription: mine\n---\n").unwrap();
    let with_own = AgentCatalog::discover_with_home(root.path(), Some(root.path())).unwrap();
    assert_eq!(advertised(&with_own), Some("mine".to_owned()));
}
