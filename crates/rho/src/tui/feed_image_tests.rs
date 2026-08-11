use std::io::Cursor;

use image::{DynamicImage, ImageFormat};
use ratatui_image::picker::{Picker, ProtocolType};
use rho_sdk::tool::ToolAsset;

use super::{
    kitty_graphics_environment, max_feed_image_height, picker_for_environment, FeedImage,
    DEFAULT_IMAGE_HEIGHT, MAX_IMAGE_HEIGHT, MIN_IMAGE_HEIGHT,
};
use crate::tui::{
    history_cache::{HistoryLineCache, HistoryLineSlice, HistoryRenderSettings},
    Entry, ToolEntry,
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
        card: rho_tools::tool_card::ToolCard::new(
            rho_tools::tool_card::ToolStatus::Ok,
            rho_tools::tool_card::ToolFamily::Default,
            rho_tools::tool_card::ToolHeader::call("read_file photo.png", None),
        ),
        expanded: false,
        image: Some(FeedImage::load(&png_asset(300, 600), &kitty_picker()).unwrap()),
    })
}

#[test]
fn loads_a_valid_bounded_asset_for_kitty_rendering() {
    let image = FeedImage::load(&png_asset(2, 1), &kitty_picker()).unwrap();

    let backend = ratatui::backend::TestBackend::new(20, DEFAULT_IMAGE_HEIGHT);
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
fn rejects_assets_larger_than_the_thumbnail_dimension_bound() {
    let error = FeedImage::load(&png_asset(1_025, 1), &kitty_picker()).unwrap_err();
    assert!(matches!(error, image::ImageError::Limits(_)));
}

// Covers: feed image max height tracks viewport then clamps.
// Owner: pure layout policy
#[test]
fn feed_image_height_budget_scales_with_viewport_then_clamps() {
    assert_eq!(max_feed_image_height(0), MIN_IMAGE_HEIGHT);
    assert_eq!(max_feed_image_height(20), MIN_IMAGE_HEIGHT);
    assert_eq!(max_feed_image_height(80), 36);
    assert_eq!(max_feed_image_height(200), MAX_IMAGE_HEIGHT);
}

// Covers: reserved rows honor the layout budget and aspect ratio.
// Owner: pure layout policy
#[test]
fn derives_reserved_rows_from_the_thumbnail_aspect_ratio() {
    let wide = FeedImage::load(&png_asset(600, 100), &kitty_picker()).unwrap();
    let tall = FeedImage::load(&png_asset(300, 600), &kitty_picker()).unwrap();
    let budget = DEFAULT_IMAGE_HEIGHT;

    assert!(wide.height_for_width(40, budget) < usize::from(budget));
    assert_eq!(tall.height_for_width(40, budget), usize::from(budget));
}

// Covers: partially scrolled image placements stay blank until fully visible.
// Owner: history cache image placement
#[test]
fn tool_entry_history_cache_omits_partially_visible_image_placement() {
    let entries = vec![image_tool()];
    let mut cache = HistoryLineCache::default();
    let width = 40;
    let budget = DEFAULT_IMAGE_HEIGHT;
    let settings = HistoryRenderSettings {
        width,
        max_tool_output_lines: 20,
        zen_mode: false,
        theme_generation: 0,
        max_image_height: budget,
    };
    let line_count = cache.line_count(&entries, settings, &no_images);

    // A one-line tool has one text row before its image; the trailing spacer is after.
    let full = cache.visible_image_placements(&entries, settings, 0, line_count, &no_images);
    assert_eq!(full.len(), 1);
    assert_eq!(full[0].row, 1);
    assert_eq!(full[0].height, usize::from(budget));

    // Avoid resizing an image into a partial viewport. Reserved rows remain
    // blank until the full image fits in the visible history window.
    let partial = cache.visible_image_placements(&entries, settings, 6, 4, &no_images);
    assert!(partial.is_empty());

    let mut visible_lines = Vec::new();
    cache.extend_visible_lines(
        &entries,
        settings,
        HistoryLineSlice { start: 6, count: 4 },
        &mut visible_lines,
        &no_images,
    );
    assert_eq!(visible_lines.len(), 4);
    assert!(visible_lines
        .iter()
        .all(|line| line.to_string().trim().is_empty()));
}
