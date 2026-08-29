use super::{side_command_action, SideCommandAction};

// Covers: /side and /btw share one action table so toggle-close and inline
// send cannot drift between the two names.
// Owner: command table
#[test]
fn side_command_action_opens_closes_and_sends() {
    let cases = [
        (false, "", SideCommandAction::Open),
        (true, "", SideCommandAction::ToggleClose),
        (
            false,
            "  what is this lock  ",
            SideCommandAction::Submit("what is this lock".into()),
        ),
        (
            true,
            "follow up",
            SideCommandAction::Submit("follow up".into()),
        ),
        (false, "\t", SideCommandAction::Open),
        (true, "   ", SideCommandAction::ToggleClose),
    ];
    for (open, args, expected) in cases {
        pretty_assertions::assert_eq!(
            side_command_action(open, args),
            expected,
            "open={open} args={args:?}"
        );
    }
}
