use pretty_assertions::assert_eq;

use super::effective_permission_mode_for;
use crate::permission::PermissionMode;

// Covers: every executor in a workflow run must use one mode no broader than
// either current policy or any frozen agent ceiling.
// Owner: workflow runtime authorization composition.
#[test]
fn effective_mode_is_the_narrowest_run_wide_ceiling() {
    for (current, frozen, expected) in [
        (PermissionMode::Auto, &[][..], PermissionMode::Auto),
        (
            PermissionMode::Auto,
            &["supervised", "auto"][..],
            PermissionMode::Supervised,
        ),
        (
            PermissionMode::Supervised,
            &["auto", "plan"][..],
            PermissionMode::Plan,
        ),
        (
            PermissionMode::Plan,
            &["auto", "supervised"][..],
            PermissionMode::Plan,
        ),
    ] {
        assert_eq!(
            effective_permission_mode_for(current, frozen.iter().copied()).unwrap(),
            expected
        );
    }
    assert!(effective_permission_mode_for(PermissionMode::Auto, ["invalid"]).is_err());
}
