use pretty_assertions::assert_eq;
use rho_protocol::{ConfigRevision, ConfigWriter};

use super::WriterRun;

fn rev(text: &str) -> ConfigRevision {
    ConfigRevision::new(text)
}

/// A writer by name; `""` is none.
fn writer(name: &str) -> Option<ConfigWriter> {
    (!name.is_empty()).then(|| ConfigWriter::new(name))
}

/// Saves a write by `by` from `before` to `after`.
fn write(run: &mut WriterRun, by: &str, before: &str, after: &str) {
    let admitted = run
        .admit(rev(before), &rev(before), writer(by))
        .expect("a write against the current revision lands");
    run.landed(admitted, rev(after));
}

/// A write `by` sends against `sent`, landing on `current`, after a's run
/// r0 -> r1 -> r2 and then `writes`, each (writer, before, after).
#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    writes: &'static [(&'static str, &'static str, &'static str)],
    current: &'static str,
    sent: &'static str,
    by: &'static str,
    lands: bool,
}

fn case(
    name: &'static str,
    writes: &'static [(&'static str, &'static str, &'static str)],
    current: &'static str,
    sent: &'static str,
    by: &'static str,
    lands: bool,
) -> Case {
    Case {
        name,
        writes,
        current,
        sent,
        by,
        lands,
    }
}

// Covers: a write against a revision its writer's own unbroken run passed
// through lands, however far the run went since; anyone else's write
// against it, or a write with no writer, is a conflict; any change outside
// the run (another writer's, even one restoring a state the run passed
// through, a writer-less one, or a hand edit) ends the run; a write that
// changed nothing does not.
// Owner: daemon settings store (pure).
#[test]
fn only_the_writers_own_run_keeps_a_stale_revision_good() {
    let cases = [
        case("run start", &[], "r2", "r0", "a", true),
        case("mid run", &[], "r2", "r1", "a", true),
        case("another writer", &[], "r2", "r0", "b", false),
        case("no writer", &[], "r2", "r0", "", false),
        case("hand edit", &[], "h", "r0", "a", false),
        case(
            "b restores r1",
            &[("b", "r2", "r1")],
            "r1",
            "r0",
            "a",
            false,
        ),
        case(
            "writer-less change",
            &[("", "r2", "r3")],
            "r3",
            "r0",
            "a",
            false,
        ),
        // Back to the run's tip, so only the reset refuses it.
        case(
            "writer-less round trip",
            &[("", "r2", "r3"), ("", "r3", "r2")],
            "r2",
            "r0",
            "a",
            false,
        ),
        case(
            "b changes nothing",
            &[("b", "r2", "r2")],
            "r2",
            "r0",
            "a",
            true,
        ),
        case(
            "current is always good",
            &[("b", "r2", "r3")],
            "r3",
            "r3",
            "a",
            true,
        ),
    ];
    let lands = |case: &Case| {
        let mut run = WriterRun::default();
        write(&mut run, "a", "r0", "r1");
        write(&mut run, "a", "r1", "r2");
        for (other, before, after) in case.writes {
            write(&mut run, other, before, after);
        }
        run.admit(rev(case.current), &rev(case.sent), writer(case.by))
            .is_ok()
    };

    assert_eq!(
        cases.map(|case| (case.name, lands(&case))),
        cases.map(|case| (case.name, case.lands)),
    );
}
