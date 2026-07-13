use super::*;

#[test]
fn code_block_state_scan_matches_markdown_rendering() {
    let fragments = ["before\n  ```rust\n", "fn main() {}\n", "```\nafter"];
    let mut scanned = false;
    let mut rendered = false;

    for fragment in fragments {
        markdown::update_code_block_state(fragment, &mut scanned);
        let mut lines = Vec::new();
        markdown::push_wrapped_markdown(&mut lines, fragment, 40, &mut rendered);
        assert_eq!(scanned, rendered);
    }
}

#[test]
fn picker_cache_invalidates_when_filter_changes() {
    let mut picker = UiPicker::new(
        "test",
        "help",
        vec![
            PickerItem {
                label: "alpha".into(),
                detail: None,
                preview: None,
                badge: None,
                value: "alpha".into(),
            },
            PickerItem {
                label: "beta".into(),
                detail: None,
                preview: None,
                badge: None,
                value: "beta".into(),
            },
        ],
        PickerAction::Config,
    );

    picker.filter = "alpha".into();
    assert_eq!(*picker.matching_indices(), vec![0]);
    assert_eq!(*picker.matching_indices(), vec![0]);
    picker.filter = "beta".into();
    assert_eq!(*picker.matching_indices(), vec![1]);
}

#[test]
fn session_header_cache_tracks_width_and_notice() {
    let mut app = test_app();
    let original = app.session_header_lines(40).to_vec();
    assert_eq!(app.session_header_cache.as_ref().unwrap().width, 40);

    app.info.update_notice = Some("new release".into());
    let updated = app.session_header_lines(20).to_vec();

    assert_ne!(original, updated);
    assert_eq!(app.session_header_cache.as_ref().unwrap().width, 20);
    assert_eq!(
        app.session_header_cache.as_ref().unwrap().update_notice,
        Some("new release".into())
    );
}
