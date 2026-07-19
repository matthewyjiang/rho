use std::io::Cursor;

use image::{DynamicImage, ImageFormat};
use ratatui_image::picker::{Picker, ProtocolType};
use rho_sdk::tool::ToolAsset;
use rho_tools::tool::ToolDisplayStyle;

use super::{kitty_graphics_environment, FeedImage, IMAGE_HEIGHT};
use crate::tui::{history_cache::HistoryLineCache, Entry, ToolEntry, ToolEntryState};

fn png_asset(width: u32, height: u32) -> ToolAsset {
    let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        width,
        height,
        image::Rgba([20, 40, 60, 255]),
    ));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, ImageFormat::Png).unwrap();
    ToolAsset::new("image/png", bytes.into_inner())
}

fn kitty_picker() -> Picker {
    let mut picker = Picker::halfblocks();
    picker.set_protocol_type(ProtocolType::Kitty);
    picker
}

fn image_tool() -> Entry {
    Entry::Tool(ToolEntry {
        state: ToolEntryState::Finished {
            ok: true,
            display_style: ToolDisplayStyle::FileOrCommand,
        },
        display_lines: vec!["read_file photo.png".into()],
        expanded: false,
        image: Some(FeedImage::load(&png_asset(300, 600), &kitty_picker()).unwrap()),
    })
}

#[test]
fn loads_a_valid_bounded_asset_for_kitty_rendering() {
    let image = FeedImage::load(&png_asset(2, 1), &kitty_picker()).unwrap();

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
fn terminal_hints_enable_direct_kitty_and_ghostty_but_not_tmux() {
    assert!(!kitty_graphics_environment(true, false, false, None, None));
    assert!(kitty_graphics_environment(false, true, false, None, None));
    assert!(kitty_graphics_environment(
        false,
        false,
        false,
        Some("Ghostty"),
        None
    ));
    assert!(!kitty_graphics_environment(
        true,
        true,
        true,
        Some("kitty"),
        Some("xterm-kitty")
    ));
    assert!(!kitty_graphics_environment(
        false,
        false,
        false,
        Some("Apple_Terminal"),
        Some("xterm-256color")
    ));
}

#[test]
fn rejects_assets_larger_than_the_thumbnail_dimension_bound() {
    let error = FeedImage::load(&png_asset(1_025, 1), &kitty_picker()).unwrap_err();
    assert!(matches!(error, image::ImageError::Limits(_)));
}

#[test]
fn derives_reserved_rows_from_the_thumbnail_aspect_ratio() {
    let wide = FeedImage::load(&png_asset(600, 100), &kitty_picker()).unwrap();
    let tall = FeedImage::load(&png_asset(300, 600), &kitty_picker()).unwrap();

    assert!(wide.height_for_width(40) < IMAGE_HEIGHT as usize);
    assert_eq!(tall.height_for_width(40), IMAGE_HEIGHT as usize);
}

#[test]
fn tool_entry_history_cache_preserves_partially_visible_image_placement() {
    let entries = vec![image_tool()];
    let mut cache = HistoryLineCache::default();
    let width = 40;
    let line_count = cache.line_count(&entries, width, 20);

    // A one-line tool has a leading block row and one text row before its image.
    let full = cache.visible_image_placements(&entries, width, 20, 0, line_count);
    assert_eq!(full.len(), 1);
    assert_eq!(full[0].row, 2);
    assert_eq!(full[0].height, IMAGE_HEIGHT as usize);

    // Slice into the middle of the reserved image rows. No marker row is needed.
    let partial = cache.visible_image_placements(&entries, width, 20, 6, 4);
    assert_eq!(partial.len(), 1);
    assert_eq!(partial[0].row, 0);
    assert_eq!(partial[0].height, 4);

    let mut visible_lines = Vec::new();
    cache.extend_visible_lines(&entries, width, 20, 6, 4, &mut visible_lines);
    assert_eq!(visible_lines.len(), 4);
    assert!(visible_lines
        .iter()
        .all(|line| line.to_string().trim().is_empty()));
}
