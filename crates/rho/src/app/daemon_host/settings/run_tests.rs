use pretty_assertions::assert_eq;
use rho_protocol::{ConfigRevision, ConfigWriter};

use super::WriterRun;

fn rev(text: &str) -> ConfigRevision {
    ConfigRevision::new(text)
}

fn writer(name: &str) -> Option<ConfigWriter> {
    Some(ConfigWriter::new(name))
}

// Covers: a write against a revision its writer's own unbroken run passed
// through lands, however far the run went since; anyone else's write
// against it, or a write with no writer, is a conflict; and any change
// outside the run (another writer's, even one restoring an earlier state
// of the run, or a hand edit) ends the run.
// Owner: daemon settings store (pure).
#[test]
fn only_the_writers_own_run_keeps_a_stale_revision_good() {
    let mut run = WriterRun::default();
    run.wrote(writer("a"), rev("r0"), rev("r1"));
    run.wrote(writer("a"), rev("r1"), rev("r2"));
    let in_run = [
        run.check(&rev("r2"), &rev("r0"), writer("a").as_ref()),
        run.check(&rev("r2"), &rev("r1"), writer("a").as_ref()),
        run.check(&rev("r2"), &rev("r0"), writer("b").as_ref()),
        run.check(&rev("r2"), &rev("r0"), None),
        // A hand edit moved the namespace to h.
        run.check(&rev("h"), &rev("r0"), writer("a").as_ref()),
    ];
    // b restores r1, a state a's run passed through.
    run.wrote(writer("b"), rev("r2"), rev("r1"));
    let after_b = [
        run.check(&rev("r1"), &rev("r0"), writer("a").as_ref()),
        run.check(&rev("r1"), &rev("r1"), writer("a").as_ref()),
    ];

    assert_eq!(
        (
            in_run.map(|check| check.is_ok()),
            after_b.map(|check| check.is_ok())
        ),
        ([true, true, false, false, false], [false, true])
    );
}
