use super::windows_mouse_input_mode;

// Console mode bits mirrored from mouse_capture.rs for assertion clarity.
const ENABLE_MOUSE_INPUT: u32 = 0x0010;
const ENABLE_WINDOW_INPUT: u32 = 0x0008;
const ENABLE_EXTENDED_FLAGS: u32 = 0x0080;
const ENABLE_QUICK_EDIT_MODE: u32 = 0x0040;
const ENABLE_PROCESSED_INPUT: u32 = 0x0004;

#[test]
fn enables_mouse_input_and_clears_quick_edit() {
    let current = ENABLE_QUICK_EDIT_MODE | ENABLE_PROCESSED_INPUT;
    let mode = windows_mouse_input_mode(current);

    assert_eq!(
        mode,
        ENABLE_PROCESSED_INPUT | ENABLE_MOUSE_INPUT | ENABLE_WINDOW_INPUT | ENABLE_EXTENDED_FLAGS
    );
}

#[test]
fn is_idempotent_when_already_configured() {
    let configured = ENABLE_MOUSE_INPUT | ENABLE_WINDOW_INPUT | ENABLE_EXTENDED_FLAGS;
    assert_eq!(windows_mouse_input_mode(configured), configured);
}
