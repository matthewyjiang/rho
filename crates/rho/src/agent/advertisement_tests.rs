use pretty_assertions::assert_eq;

use super::{AdvertisedAgents, AdvertisedChange};

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
