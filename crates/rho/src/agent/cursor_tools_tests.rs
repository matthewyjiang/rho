use std::str::FromStr;

use pretty_assertions::assert_eq;
use rho_sdk::CapabilityKind;

use super::CursorTool;

// Covers: every classified Cursor tool round-trips through the closed flag set,
// and unclassified names never become argv.
// Owner: agent cursor tools
#[test]
fn cursor_tool_flags_round_trip_and_reject_unclassified_names() {
    for tool in CursorTool::ALL {
        assert_eq!(CursorTool::from_str(tool.as_flag()), Ok(*tool));
        assert_eq!(
            tool.is_read_only(),
            tool.capability_kind() == CapabilityKind::Read
        );
    }

    for rejected in ["readToolCall", "task_tool_call", ""] {
        assert!(
            CursorTool::from_str(rejected).is_err(),
            "{rejected} must stay outside the closed set"
        );
    }
}
