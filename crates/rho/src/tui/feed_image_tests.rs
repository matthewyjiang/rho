use std::io::Cursor;

use image::{DynamicImage, ImageFormat};
use ratatui_image::picker::{Picker, ProtocolType};
use rho_sdk::tool::ToolAsset;
use rho_tools::tool::ToolDisplayStyle;

use super::{kitty_graphics_environment, picker_for_environment, FeedImage, IMAGE_HEIGHT};
use crate::tui::{
    history_cache::{HistoryLineCache, HistoryLineSlice},
    Entry, ToolEntry, ToolEntryState,
};

fn no_images(
    _: usize,
    _: &[crate::tui::markdown_image::MarkdownImageSource],
) -> Vec<(usize, FeedImage)> {
    Vec::new()
}

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
fn herdr_without_paintable_kitty_uses_halfblocks() {
    let picker = picker_for_environment(
        /*host_supports_kitty*/ true,
        crate::herdr::HerdrGraphicsCapability::Unpaintable,
    )
    .unwrap();
    assert_eq!(picker.protocol_type(), ProtocolType::Halfblocks);
}

#[test]
fn herdr_with_paintable_kitty_keeps_kitty_protocol() {
    let picker = picker_for_environment(
        /*host_supports_kitty*/ true,
        crate::herdr::HerdrGraphicsCapability::Paintable,
    )
    .unwrap();
    assert_eq!(picker.protocol_type(), ProtocolType::Kitty);
}

#[test]
fn direct_kitty_host_keeps_kitty_protocol() {
    let picker = picker_for_environment(
        /*host_supports_kitty*/ true,
        crate::herdr::HerdrGraphicsCapability::NotHerdr,
    )
    .unwrap();
    assert_eq!(picker.protocol_type(), ProtocolType::Kitty);
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
fn tool_entry_history_cache_omits_partially_visible_image_placement() {
    let entries = vec![image_tool()];
    let mut cache = HistoryLineCache::default();
    let width = 40;
    let line_count = cache.line_count(&entries, width, 20, &no_images);

    // A one-line tool has a leading block row and one text row before its image.
    let full = cache.visible_image_placements(&entries, width, 20, 0, line_count, &no_images);
    assert_eq!(full.len(), 1);
    assert_eq!(full[0].row, 2);
    assert_eq!(full[0].height, IMAGE_HEIGHT as usize);

    // Avoid resizing an image into a partial viewport. Reserved rows remain
    // blank until the full image fits in the visible history window.
    let partial = cache.visible_image_placements(&entries, width, 20, 6, 4, &no_images);
    assert!(partial.is_empty());

    let mut visible_lines = Vec::new();
    cache.extend_visible_lines(
        &entries,
        width,
        20,
        HistoryLineSlice { start: 6, count: 4 },
        &mut visible_lines,
        &no_images,
    );
    assert_eq!(visible_lines.len(), 4);
    assert!(visible_lines
        .iter()
        .all(|line| line.to_string().trim().is_empty()));
}

#[test]
fn markdown_image_placements_reserve_rows_for_ready_images() {
    use crate::tui::feed_image::reserve_markdown_image_rows;

    let image = FeedImage::load(&png_asset(300, 600), &kitty_picker()).unwrap();
    let mut lines: Vec<ratatui::text::Line<'static>> =
        (0..3).map(|_| ratatui::text::Line::raw("x")).collect();
    // One placeholder row at index 1 (the standalone image row).
    let placements = reserve_markdown_image_rows(&mut lines, &[1], &[(0, image)], 40).unwrap();

    let height = IMAGE_HEIGHT as usize;
    assert_eq!(lines.len(), 3 + height - 1);
    let placements: Vec<_> = placements.iter().collect();
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].rows, 1..1 + height);
}

#[test]
fn markdown_image_rows_not_ready_keep_their_placeholder() {
    use crate::tui::feed_image::reserve_markdown_image_rows;

    let mut lines: Vec<ratatui::text::Line<'static>> =
        (0..2).map(|_| ratatui::text::Line::raw("x")).collect();
    // No ready images -> no placements and no inserted rows.
    assert!(reserve_markdown_image_rows(&mut lines, &[1], &[], 40).is_none());
    assert_eq!(lines.len(), 2);
}
