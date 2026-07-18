use super::{
    kitty_graphics_environment, kitty_picker, FeedImage, IMAGE_HEIGHT, IMAGE_MARKER_PREFIX,
};

#[test]
fn loads_a_valid_feed_image_for_kitty_rendering() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("photo.png");
    image::RgbaImage::from_pixel(2, 1, image::Rgba([20, 40, 60, 255]))
        .save(&path)
        .unwrap();

    let image = FeedImage::load(42, &path, &kitty_picker()).unwrap();

    assert_eq!(image.id(), 42);
    assert_eq!(image.marker(), format!("{IMAGE_MARKER_PREFIX}42"));

    let backend = ratatui::backend::TestBackend::new(20, IMAGE_HEIGHT);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| image.render(frame, frame.area()))
        .unwrap();
    assert!(terminal
        .backend()
        .buffer()
        .content
        .iter()
        .any(|cell| cell.symbol().contains("\x1b_G")));
}

#[test]
fn detects_kitty_and_ghostty_environments() {
    assert!(kitty_graphics_environment(true, false, None, None));
    assert!(kitty_graphics_environment(false, true, None, None));
    assert!(kitty_graphics_environment(
        false,
        false,
        Some("kitty"),
        None
    ));
    assert!(kitty_graphics_environment(
        false,
        false,
        Some("Ghostty"),
        None
    ));
    assert!(kitty_graphics_environment(
        false,
        false,
        None,
        Some("xterm-ghostty")
    ));
}

#[test]
fn leaves_other_terminals_on_text_fallback() {
    assert!(!kitty_graphics_environment(false, false, None, None));
    assert!(!kitty_graphics_environment(
        false,
        false,
        Some("Apple_Terminal"),
        Some("xterm-256color")
    ));
}
