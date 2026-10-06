use pretty_assertions::assert_eq;

use super::Keybindings;

// Covers: remapping the cycle key must not silently shadow another shortcut,
// including reserved fallbacks and the unchanged Shift+Tab reasoning chord.
// Owner: keybinding configuration parsing
#[test]
fn permission_cycle_remaps_reject_collisions() {
    let cases = [
        ("cycle_permission_mode = \"alt+n\"", Ok("alt+n")),
        ("cycle_permission_mode = \"ctrl+p\"", Err(())),
        ("cycle_permission_mode = \"alt+s\"", Err(())),
        ("cycle_permission_mode = \"ctrl+enter\"", Err(())),
        ("cycle_permission_mode = \"shift+tab\"", Err(())),
        ("cycle_permission_mode = \"alt+v\"", Err(())),
        ("reset_conversation = \"alt+m\"", Err(())),
        ("toggle_tool_output = \"alt+m\"", Err(())),
        (
            "toggle_tool_output = \"alt+m\"\ncycle_permission_mode = \"alt+n\"",
            Ok("alt+n"),
        ),
    ];
    for (text, expected) in cases {
        let parsed = toml::from_str::<Keybindings>(text)
            .map(|keys| keys.cycle_permission_mode.to_string())
            .map_err(|_| ());
        assert_eq!(parsed, expected.map(str::to_owned), "{text}");
    }
}
