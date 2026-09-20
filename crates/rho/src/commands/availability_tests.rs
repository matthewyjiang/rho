use pretty_assertions::assert_eq;

use super::{CommandContext, CommandId};

// Covers: a model capability gates its command without hiding session commands.
// Owner: command discovery policy
#[test]
fn discovery_depends_on_the_required_capability() {
    for (fast_mode_supported, expected) in
        [(false, [false, true, true]), (true, [true, true, true])]
    {
        let context = CommandContext {
            fast_mode_supported,
        };
        assert_eq!(
            [CommandId::Fast, CommandId::Model, CommandId::Limits]
                .map(|command| context.is_discoverable(command)),
            expected,
        );
    }
}
