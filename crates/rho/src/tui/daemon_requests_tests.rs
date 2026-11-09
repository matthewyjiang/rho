use pretty_assertions::assert_eq;
use rho_protocol::{ConfigNamespace, ConfigRevision};

use super::OwnWrites;

// Covers: writes sent faster than the daemon's change notifications each
// build on the revision the last one saved, rather than repeating the
// cache's stale stamp and being refused as a conflict with this TUI's own
// change, also when a notification lands mid-burst and the cache holds a
// revision between; once the cache holds the latest, or a revision another
// client made, its stamp goes out unchanged; another namespace keeps its
// own chain.
// Owner: daemon settings writes (pure).
#[test]
fn quick_writes_chain_on_the_last_saved_revision() {
    let rev = |text: &str| ConfigRevision::new(text);
    let agent = ConfigNamespace::Agent;
    let mut writes = OwnWrites::default();
    let write = |writes: &mut OwnWrites, stamped: &str, saved: Option<&str>| {
        let sent = writes.revision_for(&agent, rev(stamped));
        if let Some(saved) = saved {
            writes.saved(agent.clone(), sent.clone(), rev(saved));
        }
        sent
    };
    let sent = vec![
        // A burst stamped with the cache's r0.
        write(&mut writes, "r0", Some("r1")),
        write(&mut writes, "r0", Some("r2")),
        write(&mut writes, "r0", Some("r3")),
        // r2's notification landed; the cache is still behind r3.
        write(&mut writes, "r2", Some("r4")),
        // The cache holds the latest.
        write(&mut writes, "r4", None),
        // Another client's revision is not this TUI's to remap.
        write(&mut writes, "x1", None),
    ];
    let tui = ConfigNamespace::Client {
        name: rho_protocol::ClientName::new("tui").unwrap(),
    };
    let other = writes.revision_for(&tui, rev("r0"));

    assert_eq!(
        (sent, other),
        (
            ["r0", "r1", "r2", "r3", "r4", "x1"].map(rev).to_vec(),
            rev("r0")
        )
    );
}

// Covers: a write refused as a conflict drops the namespace's chain, so
// when another client restores values whose revision the chain holds, the
// retry goes out against the cache instead of the stale tip forever.
// Owner: daemon settings writes (pure).
#[test]
fn a_refused_write_drops_the_chain() {
    let rev = |text: &str| ConfigRevision::new(text);
    let agent = ConfigNamespace::Agent;
    let mut writes = OwnWrites::default();
    writes.saved(agent.clone(), rev("r0"), rev("r1"));
    // Another client put the values back, so the daemon and cache hold r0.
    let refused = writes.revision_for(&agent, rev("r0"));
    writes.forget(&agent);
    let retry = writes.revision_for(&agent, rev("r0"));

    assert_eq!((refused, retry), (rev("r1"), rev("r0")));
}
