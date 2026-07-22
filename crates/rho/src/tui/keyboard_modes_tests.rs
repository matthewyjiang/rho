use super::should_request_extended_keyboard_protocols;

#[test]
fn extended_keyboard_protocols_follow_windows_conpty_policy() {
    // Keep Shift+Tab representable under ConPTY by skipping Kitty enhancements
    // and modifyOtherKeys on Windows. See the policy comment in keyboard_modes.
    assert_eq!(
        should_request_extended_keyboard_protocols(),
        !cfg!(windows),
        "extended keyboard protocols must stay disabled on Windows"
    );
}
