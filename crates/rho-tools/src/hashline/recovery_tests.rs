use pretty_assertions::assert_eq;

use super::*;

// Covers: uniform insert-above drift must remap anchors and keep apply safe
// Owner: hashline recovery
#[test]
fn remaps_replace_when_prefix_lines_are_inserted() {
    let previous = "alpha\ntarget\ngamma\n";
    let current = "intro\nalpha\ntarget\ngamma\n";
    let ops = vec![Op::Replace {
        start: 2,
        end: 2,
        body: vec!["TARGET".into()],
    }];
    let remapped = remap_ops(previous, current, &ops).unwrap();
    assert_eq!(
        remapped,
        vec![Op::Replace {
            start: 3,
            end: 3,
            body: vec!["TARGET".into()],
        }]
    );
    let outcome = try_recover(previous, current, &ops).unwrap();
    assert_eq!(outcome.text, "intro\nalpha\nTARGET\ngamma\n");
}

// Covers: changed anchor text must refuse recovery
// Owner: hashline recovery
#[test]
fn refuses_when_anchor_content_changed() {
    let previous = "alpha\ntarget\ngamma\n";
    let current = "alpha\nchanged\ngamma\n";
    let ops = vec![Op::Replace {
        start: 2,
        end: 2,
        body: vec!["TARGET".into()],
    }];
    assert!(remap_ops(previous, current, &ops).is_none());
}
