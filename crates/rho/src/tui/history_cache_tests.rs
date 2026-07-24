use pretty_assertions::assert_eq;

use super::*;
use crate::tui::render::{entry_lines, render_entry_with_images, render_entry_with_options};

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn no_images(_: usize, _: &[MarkdownImageSource]) -> Vec<(usize, FeedImage)> {
    Vec::new()
}

#[test]
fn caches_code_block_copy_target_and_raw_contents() {
    let mut cache = HistoryLineCache::default();
    let entries = vec![Entry::Assistant(
        "before\n```rust\nlet x = 1;\nprintln!(\"{x}\");\n```\nafter".into(),
    )];

    let blocks = cache.code_blocks(&entries, 40, 10, &no_images);

    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].text.as_ref(), "let x = 1;\nprintln!(\"{x}\");");
    assert_eq!(blocks[0].line, 2);
    assert_eq!(blocks[0].copy_columns, 32..38);
}

#[test]
fn caches_unicode_wrapped_lines_and_code_copy_target_without_rendering_drift() {
    let mut cache = HistoryLineCache::default();
    let entries = vec![Entry::Assistant("你好你好你好\n```text\nλ🙂\n```".into())];
    let expected_lines = entry_lines(&entries[0], 12, 10);

    let mut cached_lines = Vec::new();
    cache.extend_visible_lines(
        &entries,
        12,
        10,
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut cached_lines,
        &no_images,
    );
    let blocks = cache.code_blocks(&entries, 12, 10, &no_images);

    assert_eq!(cached_lines, expected_lines);
    assert_eq!(
        blocks,
        &[CachedCodeBlock {
            line: 3,
            copy_columns: 4..10,
            text: Arc::from("λ🙂"),
        }]
    );
}

#[test]
fn incrementally_extends_assistant_markdown_without_rendering_drift() {
    let mut cache = HistoryLineCache::default();
    let mut entries = vec![Entry::Assistant("intro\n\nheader | value\n".into())];
    let mut cached_lines = Vec::new();
    cache.extend_visible_lines(
        &entries,
        32,
        10,
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut cached_lines,
        &no_images,
    );

    let Entry::Assistant(text) = &mut entries[0] else {
        unreachable!();
    };
    text.push_str("--- | ---\nrow | `one`\n");
    cache.assistant_appended(0);
    cached_lines.clear();
    cache.extend_visible_lines(
        &entries,
        32,
        10,
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut cached_lines,
        &no_images,
    );
    assert_eq!(cached_lines, entry_lines(&entries[0], 32, 10));

    let Entry::Assistant(text) = &mut entries[0] else {
        unreachable!();
    };
    text.push_str("\n## streamed heading\n");
    cache.assistant_appended(0);
    cached_lines.clear();
    cache.extend_visible_lines(
        &entries,
        32,
        10,
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut cached_lines,
        &no_images,
    );
    assert_eq!(cached_lines, entry_lines(&entries[0], 32, 10));

    let Entry::Assistant(text) = &mut entries[0] else {
        unreachable!();
    };
    text.push_str("\n```rust\nlet answer = 42;\n");
    cache.assistant_appended(0);
    cached_lines.clear();
    cache.extend_visible_lines(
        &entries,
        32,
        10,
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut cached_lines,
        &no_images,
    );
    assert_eq!(cached_lines, entry_lines(&entries[0], 32, 10));

    let Entry::Assistant(text) = &mut entries[0] else {
        unreachable!();
    };
    text.push_str("println!(\"{answer}\");\n```\ndone\n");
    cache.assistant_appended(0);
    cached_lines.clear();
    cache.extend_visible_lines(
        &entries,
        32,
        10,
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut cached_lines,
        &no_images,
    );

    assert_eq!(cached_lines, entry_lines(&entries[0], 32, 10));
    assert_eq!(cache.code_blocks(&entries, 32, 10, &no_images).len(), 1);
    assert!(cache.assistant_caches[0]
        .is_some_and(|cached| cached.stable_source_len > "intro\n\n".len()));
}

#[test]
fn streams_mermaid_as_source_then_caches_the_closed_diagram_by_width() {
    let mut cache = HistoryLineCache::default();
    let mut entries = vec![Entry::Assistant(
        "```mermaid\nflowchart LR\nA[Parse] --> B[Render]".into(),
    )];
    let mut cached_lines = Vec::new();
    cache.extend_visible_lines(
        &entries,
        80,
        10,
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut cached_lines,
        &no_images,
    );
    assert_eq!(cached_lines, entry_lines(&entries[0], 80, 10));
    assert!(cached_lines
        .iter()
        .any(|line| line_text(line).contains("flowchart LR")));

    let Entry::Assistant(text) = &mut entries[0] else {
        unreachable!();
    };
    text.push_str("\n```");
    cache.assistant_appended(0);
    cached_lines.clear();
    cache.extend_visible_lines(
        &entries,
        80,
        10,
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut cached_lines,
        &no_images,
    );

    assert_eq!(cached_lines, entry_lines(&entries[0], 80, 10));
    assert!(cached_lines
        .iter()
        .any(|line| line_text(line).contains("MERMAID")));
    assert!(!cached_lines
        .iter()
        .any(|line| line_text(line).contains("flowchart LR")));
    assert_eq!(
        cache.code_blocks(&entries, 80, 10, &no_images)[0]
            .text
            .as_ref(),
        "flowchart LR\nA[Parse] --> B[Render]"
    );

    let mut narrow_lines = Vec::new();
    cache.extend_visible_lines(
        &entries,
        36,
        10,
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut narrow_lines,
        &no_images,
    );
    assert_eq!(narrow_lines, entry_lines(&entries[0], 36, 10));
    assert_ne!(cached_lines, narrow_lines);
}

#[test]
fn invalidating_an_assistant_entry_refreshes_code_block_contents() {
    let mut cache = HistoryLineCache::default();
    let mut entries = vec![Entry::Assistant("```\nfirst\n```".into())];
    assert_eq!(
        cache.code_blocks(&entries, 30, 10, &no_images)[0]
            .text
            .as_ref(),
        "first"
    );

    entries[0] = Entry::Assistant("```\nsecond\n```".into());
    cache.invalidate_from(0);

    assert_eq!(
        cache.code_blocks(&entries, 30, 10, &no_images)[0]
            .text
            .as_ref(),
        "second"
    );
}

#[test]
fn markdown_image_placement_renders_after_images_resolve() {
    use image::{DynamicImage, ImageFormat};
    use rho_sdk::tool::ToolAsset;
    use std::io::Cursor;

    let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        300,
        600,
        image::Rgba([20, 40, 60, 255]),
    ));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, ImageFormat::Png).unwrap();
    let asset = ToolAsset::new("image/png", bytes.into_inner());
    let mut picker = ratatui_image::picker::Picker::halfblocks();
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    let feed = FeedImage::load(&asset, &picker).unwrap();

    let mut cache = HistoryLineCache::default();
    let mut entries = vec![Entry::Assistant(format!(
        "before\n\n![photo](photo.png)\n\n{}\n```\ncopy me\n```",
        (0..15)
            .map(|index| format!("stable {index}"))
            .collect::<Vec<_>>()
            .join("\n")
    ))];

    // Before any images are marked ready there is no placement.
    let placements = cache.visible_image_placements(&entries, 40, 20, 0, usize::MAX, &no_images);
    assert!(placements.is_empty());

    // Re-render with a resolver after the load invalidates this entry.
    cache.invalidate_from(0);
    let feed_clone = feed.clone();
    let mut lines = Vec::new();
    cache.extend_visible_lines(
        &entries,
        40,
        20,
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut lines,
        &|_index, _sources| vec![(0, feed_clone.clone())],
    );

    let placements =
        cache.visible_image_placements(&entries, 40, 20, 0, usize::MAX, &|_index, _sources| {
            vec![(0, feed.clone())]
        });
    assert_eq!(placements.len(), 1, "expected one markdown image placement");
    assert_eq!(placements[0].row, 3);
    let fallback = render_entry_with_images(&entries[0], 40, 20, None);
    let expected_code_line = fallback.code_blocks[0].top_line + placements[0].height;
    assert_eq!(
        cache.code_blocks(&entries, 40, 20, &|_index, _sources| vec![(
            0,
            feed.clone()
        )])[0]
            .line,
        expected_code_line
    );

    let Entry::Assistant(text) = &mut entries[0] else {
        unreachable!();
    };
    text.push_str("\nstreamed tail");
    cache.assistant_appended(0);
    lines.clear();
    cache.extend_visible_lines(
        &entries,
        40,
        20,
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut lines,
        &|_index, _sources| vec![(0, feed.clone())],
    );
    let expected = render_entry_with_images(&entries[0], 40, 20, Some(&[(0, feed.clone())])).lines;
    assert_eq!(lines, expected);
}

#[test]
fn open_stream_tail_omits_trailing_blank_until_closed() {
    let mut cache = HistoryLineCache::default();
    let entries = vec![Entry::Assistant("Hello committed line\n".into())];

    cache.set_open_stream_tail(true);
    let open_count = cache.line_count(&entries, 60, 10, &no_images);
    let mut open_lines = Vec::new();
    cache.extend_visible_lines(
        &entries,
        60,
        10,
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut open_lines,
        &no_images,
    );
    assert_eq!(open_lines.len(), open_count);
    assert_eq!(
        open_lines,
        render_entry_with_options(&entries[0], 60, 10, false).lines
    );

    cache.set_open_stream_tail(false);
    let closed_count = cache.line_count(&entries, 60, 10, &no_images);
    assert_eq!(closed_count, open_count + 1);
    let mut closed_lines = Vec::new();
    cache.extend_visible_lines(
        &entries,
        60,
        10,
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut closed_lines,
        &no_images,
    );
    assert_eq!(closed_lines, entry_lines(&entries[0], 60, 10));
}
