use pretty_assertions::assert_eq;
use rho_protocol::{ConfigNamespace, ConfigRevision};

use super::OwnWrites;

// Covers: writes sent faster than the daemon's change notifications each
// build on the revision the last one saved, rather than repeating the
// cache's stale stamp and being refused as a conflict with this TUI's own
// change; once the cache catches up, its stamp is sent as is, and another
// namespace keeps its own chain.
// Owner: daemon settings writes (pure).
#[test]
fn quick_writes_chain_on_the_last_saved_revision() {
    let rev = |text: &str| ConfigRevision::new(text);
    let agent = ConfigNamespace::Agent;
    let mut writes = OwnWrites::default();
    let mut sent = Vec::new();
    // Three writes stamped with the cache's r0, each saving the next revision.
    for saved in ["r1", "r2", "r3"] {
        sent.push(writes.revision_for(&agent, rev("r0")));
        writes.saved(agent.clone(), rev("r0"), rev(saved));
    }
    // The cache caught up to r3: its stamp goes out unchanged.
    sent.push(writes.revision_for(&agent, rev("r3")));
    let tui = ConfigNamespace::Client {
        name: rho_protocol::ClientName::new("tui").unwrap(),
    };
    sent.push(writes.revision_for(&tui, rev("r0")));

    assert_eq!(sent, ["r0", "r1", "r2", "r3", "r0"].map(rev).to_vec());
}
