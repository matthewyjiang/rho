use super::*;
use pretty_assertions::assert_eq;

// Covers: never persist approvals/denials or approve when allow_once is absent.
// Owner: generic permission selector, independent of agent-specific option ids.
#[test]
fn one_shot_selection_fails_closed() {
    use PermissionOptionKind::{
        AllowAlways as A, AllowOnce as O, RejectAlways as R, RejectOnce as N,
    };
    let cases = [
        (vec![], None, None),
        (vec![O], Some(0), None),
        (vec![A], None, None),
        (vec![N], Some(0), Some(0)),
        (vec![R], None, None),
        (vec![A, R], None, None),
        (vec![A, R, N], Some(2), Some(2)),
        (vec![N, A, O, R], Some(2), Some(0)),
        (vec![R, O, A], Some(1), None),
        (vec![O, O, N], Some(0), Some(2)),
    ];
    for (kinds, allow, reject) in cases {
        // Deliberately misleading labels and arbitrary ids: only kind matters.
        let options: Vec<_> = kinds
            .into_iter()
            .enumerate()
            .map(|(index, kind)| {
                PermissionOption::new(format!("opaque-{index}"), "allow always", kind)
            })
            .collect();
        for (decision, index) in [
            (PermissionDecision::AllowOnce, allow),
            (PermissionDecision::Reject, reject),
        ] {
            let expected = index.map_or(RequestPermissionOutcome::Cancelled, |index| {
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                    options[index].option_id.clone(),
                ))
            });
            assert_eq!(
                choose_option(decision, &options),
                expected,
                "{decision:?}: {options:?}"
            );
        }
    }
}
