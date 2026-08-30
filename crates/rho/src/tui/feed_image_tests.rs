use std::io::Cursor;

use image::{DynamicImage, ImageFormat};
use ratatui::{backend::TestBackend, Terminal};
use ratatui_image::picker::{Picker, ProtocolType};
use rho_sdk::tool::ToolAsset;

use super::{
    feed_image_height_budget, kitty_graphics_environment, max_feed_image_height,
    picker_for_environment, quantize_content_image_cap, reserve_image_rows, FeedImage,
    COMPACT_IMAGE_HEIGHT, COMPOSER_IMAGE_HEIGHT, DEFAULT_IMAGE_HEIGHT, MAX_IMAGE_HEIGHT,
    MIN_IMAGE_HEIGHT, TALL_IMAGE_HEIGHT,
};
use crate::tui::{
    history_cache::{HistoryLineCache, HistoryLineSlice, HistoryRenderSettings},
    render::entry_lines,
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

fn image_tool_with_image(image: FeedImage) -> Entry {
    Entry::Tool(ToolEntry::new(
        rho_tools::tool_card::ToolCard::new(
            rho_tools::tool_card::ToolStatus::Ok,
            rho_tools::tool_card::ToolFamily::Default,
            rho_tools::tool_card::ToolHeader::call("read_file photo.png", None),
        ),
        false,
        Some(image),
        None,
    ))
}

fn image_tool() -> Entry {
    image_tool_with_image(FeedImage::load(&png_asset(300, 600), &kitty_picker()).unwrap())
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

// Covers: composer paste previews must accept full-resolution images by
// decoding under a larger bound and shrinking to the thumbnail box.
// Owner: pure decode policy
#[test]
fn composer_base64_preview_accepts_oversized_source_images() {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    let bytes = {
        let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1_800,
            1_200,
            image::Rgba([10, 20, 30, 255]),
        ));
        let mut encoded = Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, ImageFormat::Png)
            .expect("encode png");
        encoded.into_inner()
    };
    // Feed path still rejects this size at decode time.
    assert!(matches!(
        FeedImage::decode(&bytes),
        Err(image::ImageError::Limits(_))
    ));

    let decoded = FeedImage::decode_composer_base64(&STANDARD.encode(bytes))
        .expect("composer preview should thumbnail oversized pastes");
    let image = decoded.to_feed_image(&kitty_picker());
    assert_eq!(
        image.height_for_width(40, COMPOSER_IMAGE_HEIGHT),
        usize::from(COMPOSER_IMAGE_HEIGHT)
    );
}

// Covers: feed image max height tracks terminal height bands, with a compact
// floor that stays paintable after typical chrome on short terminals.
// Owner: pure layout policy
#[test]
fn feed_image_height_budget_uses_terminal_height_bands() {
    assert_eq!(max_feed_image_height(0), COMPACT_IMAGE_HEIGHT);
    assert_eq!(max_feed_image_height(24), COMPACT_IMAGE_HEIGHT);
    assert_eq!(max_feed_image_height(25), MIN_IMAGE_HEIGHT);
    assert_eq!(max_feed_image_height(37), DEFAULT_IMAGE_HEIGHT);
    assert_eq!(max_feed_image_height(53), TALL_IMAGE_HEIGHT);
    assert_eq!(max_feed_image_height(69), MAX_IMAGE_HEIGHT);
    // Compact reservation must stay at or below a short history content pane
    // (terminal 24 minus statusline/composer/dividers leaves ~16 content rows).
    assert!(usize::from(COMPACT_IMAGE_HEIGHT) <= 16);
}

// Covers: when composer chrome shrinks history content below the preferred band,
// the reservation caps to the content viewport so a full placement can paint.
// Owner: pure layout policy
#[test]
fn feed_image_height_budget_caps_to_history_content_viewport() {
    // Terminal 40 → preferred DEFAULT (24).
    assert_eq!(max_feed_image_height(40), DEFAULT_IMAGE_HEIGHT);
    // Unknown content keeps the preferred band (tests / recovery).
    assert_eq!(feed_image_height_budget(40, 0), DEFAULT_IMAGE_HEIGHT);
    // Content taller than preferred keeps preferred.
    assert_eq!(feed_image_height_budget(40, 30), DEFAULT_IMAGE_HEIGHT);
    // Composer attachment strips reduced content below preferred → cap.
    // Heights below the compact band stay exact; larger panes snap to bands.
    assert_eq!(feed_image_height_budget(40, 10), 10);
    assert_eq!(feed_image_height_budget(40, 1), 1);
    assert_eq!(feed_image_height_budget(40, 20), MIN_IMAGE_HEIGHT);
    assert_eq!(quantize_content_image_cap(20), MIN_IMAGE_HEIGHT);
    assert_eq!(quantize_content_image_cap(15), COMPACT_IMAGE_HEIGHT);
}

// Covers: one-row content-height jitter must not rewrite the image budget once
// the pane sits inside a discrete band (avoids long-transcript cache thrash).
// Owner: pure layout policy
#[test]
fn feed_image_height_budget_stable_across_one_row_content_jitter() {
    let terminal = 80usize; // preferred MAX (40)
    let budget_a = feed_image_height_budget(terminal, 37);
    let budget_b = feed_image_height_budget(terminal, 36);
    let budget_c = feed_image_height_budget(terminal, 33);
    assert_eq!(budget_a, TALL_IMAGE_HEIGHT);
    assert_eq!(budget_b, TALL_IMAGE_HEIGHT);
    assert_eq!(budget_c, TALL_IMAGE_HEIGHT);
    assert_ne!(
        feed_image_height_budget(terminal, 32),
        feed_image_height_budget(terminal, 31)
    );
}

// Covers: a tall feed image reserved under a content-capped budget is fully
// visible inside that content viewport (paintable under full-block paint rules).
// Owner: history cache image placement
#[test]
fn content_capped_budget_keeps_tall_image_paintable_in_shrunken_viewport() {
    let entries = vec![image_tool()];
    let mut cache = HistoryLineCache::default();
    let width = 40;
    // Preferred band for a tall terminal would be 24+, but composer image rows
    // left only 10 history content rows.
    let content_height = 10usize;
    let budget = feed_image_height_budget(48, content_height);
    assert_eq!(budget, 10);
    assert!(
        usize::from(budget) <= content_height,
        "reservation must not exceed the content viewport"
    );
    let settings = HistoryRenderSettings {
        width,
        max_tool_output_lines: 20,
        zen_mode: false,
        theme_generation: 0,
        max_image_height: budget,
    };
    let line_count = cache.line_count(&entries, settings, &no_images);
    let full = cache.visible_image_placements(&entries, settings, 0, line_count, &no_images);
    assert_eq!(full.len(), 1);
    assert_eq!(full[0].height, usize::from(budget));
    // Tool header sits above the image; scroll so the reserved block is fully
    // inside a content_height window.
    let image_start = full[0].row;
    let image_end = image_start + full[0].height;
    let scroll = image_start.min(line_count.saturating_sub(content_height));
    let placements =
        cache.visible_image_placements(&entries, settings, scroll, content_height, &no_images);
    assert_eq!(
        placements.len(),
        1,
        "capped reservation must fully fit some content_height window (scroll={scroll}, image={image_start}..{image_end}, lines={line_count})"
    );

    // Same image without the cap would reserve the preferred band and never
    // fully fit a 10-row content pane at any scroll.
    let uncapped = max_feed_image_height(48);
    assert!(usize::from(uncapped) > content_height);
    let mut lines = Vec::new();
    let tall = FeedImage::load(&png_asset(300, 600), &kitty_picker()).unwrap();
    let placement = reserve_image_rows(&mut lines, &tall, width, uncapped);
    assert!(
        placement.rows.end - placement.rows.start > content_height,
        "uncapped reservation must exceed the shrunken content pane"
    );
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

// Covers: the reserved feed-image height uses the same padded content width
// that the image renderer receives, avoiding trailing blank rows.
// Owner: feed image layout width
#[test]
fn reserved_feed_image_rows_match_the_painted_content_width() {
    let image = FeedImage::load(&png_asset(400, 102), &kitty_picker()).unwrap();
    let budget = DEFAULT_IMAGE_HEIGHT;
    let mut lines = Vec::new();
    let placement = reserve_image_rows(&mut lines, &image, 38, budget);

    assert_eq!(placement.rows.end - placement.rows.start, 5);
    assert_eq!(lines.len(), image.height_for_width(38, budget));
    assert_ne!(
        image.height_for_width(40, budget),
        image.height_for_width(38, budget)
    );

    let mut entry = image_tool_with_image(image.clone());
    let with_image = entry_lines(&entry, 40, 20, budget);
    let Entry::Tool(tool) = &mut entry else {
        unreachable!();
    };
    tool.image = None;
    let without_image = entry_lines(&entry, 40, 20, budget);
    assert_eq!(
        with_image.len() - without_image.len(),
        image.height_for_width(38, budget)
    );
}

// Covers: clipped image rendering preserves source rows at both viewport
// boundaries instead of fitting the whole image into the partial area.
// Owner: feed image protocol rendering
#[test]
fn partial_image_rendering_keeps_the_original_encoded_rows() {
    let image = FeedImage::load(&png_asset(20, 80), &Picker::halfblocks()).unwrap();

    let mut full_terminal = Terminal::new(TestBackend::new(10, 4)).unwrap();
    full_terminal
        .draw(|frame| image.render(frame, frame.area()))
        .unwrap();
    let full = full_terminal.backend().buffer().clone();

    let mut top_terminal = Terminal::new(TestBackend::new(10, 2)).unwrap();
    top_terminal
        .draw(|frame| image.render_partial(frame, frame.area(), 4, 1))
        .unwrap();
    let top = top_terminal.backend().buffer();
    for x in 0..2 {
        assert_eq!(top[(x, 0)], full[(x, 1)]);
        assert_eq!(top[(x, 1)], full[(x, 2)]);
    }

    let mut bottom_terminal = Terminal::new(TestBackend::new(10, 2)).unwrap();
    bottom_terminal
        .draw(|frame| image.render_partial(frame, frame.area(), 4, 0))
        .unwrap();
    let bottom = bottom_terminal.backend().buffer();
    for x in 0..2 {
        assert_eq!(bottom[(x, 0)], full[(x, 0)]);
        assert_eq!(bottom[(x, 1)], full[(x, 1)]);
    }
}

// Covers: partially scrolled image placements retain the visible image rows.
// Owner: history cache image placement
#[test]
fn tool_entry_history_cache_tracks_partial_image_placements_at_both_boundaries() {
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

    // Scrolling past the top keeps the image's visible rows and records how
    // many encoded rows must be skipped.
    let top_partial = cache.visible_image_placements(&entries, settings, 6, 4, &no_images);
    assert_eq!(top_partial.len(), 1);
    assert_eq!(top_partial[0].row, 0);
    assert_eq!(top_partial[0].height, 4);
    assert_eq!(top_partial[0].total_height, usize::from(budget));
    assert_eq!(top_partial[0].skip_rows, 5);

    // A viewport ending in the image keeps the leading rows and drops only
    // the rows below the viewport.
    let bottom_partial = cache.visible_image_placements(&entries, settings, 1, 4, &no_images);
    assert_eq!(bottom_partial.len(), 1);
    assert_eq!(bottom_partial[0].row, 0);
    assert_eq!(bottom_partial[0].height, 4);
    assert_eq!(bottom_partial[0].total_height, usize::from(budget));
    assert_eq!(bottom_partial[0].skip_rows, 0);

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
