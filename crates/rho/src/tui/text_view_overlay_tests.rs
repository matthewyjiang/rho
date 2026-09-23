use super::*;

// Covers: opening the prompt view without a parent picker leaves the composer
// untouched instead of panicking (debug builds flag the misuse).
// Owner: text view overlay
#[test]
#[cfg_attr(debug_assertions, should_panic(expected = "open parent picker"))]
fn text_view_without_parent_picker_keeps_composer() {
    let mut app = super::super::tests::test_app();
    app.open_text_view_over_picker("title".into(), "body".into());
    assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
}
