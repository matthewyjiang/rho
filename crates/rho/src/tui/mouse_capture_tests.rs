use super::windows_mouse_input_mode;

// Console mode bits mirrored from mouse_capture.rs for assertion clarity.
const ENABLE_MOUSE_INPUT: u32 = 0x0010;
const ENABLE_WINDOW_INPUT: u32 = 0x0008;
const ENABLE_EXTENDED_FLAGS: u32 = 0x0080;
const ENABLE_QUICK_EDIT_MODE: u32 = 0x0040;
const ENABLE_PROCESSED_INPUT: u32 = 0x0004;

#[test]
fn enables_mouse_input_and_clears_quick_edit() {
    let configured = ENABLE_MOUSE_INPUT | ENABLE_WINDOW_INPUT | ENABLE_EXTENDED_FLAGS;
    for (case, current, expected) in [
        (
            "quick edit cleared, other bits kept",
            ENABLE_QUICK_EDIT_MODE | ENABLE_PROCESSED_INPUT,
            ENABLE_PROCESSED_INPUT | configured,
        ),
        ("already configured is idempotent", configured, configured),
    ] {
        assert_eq!(windows_mouse_input_mode(current), expected, "{case}");
    }
}
