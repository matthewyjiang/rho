use pretty_assertions::assert_eq;

use super::*;
use crate::tui::render::entry_lines;

fn no_images(_: usize, _: &[MarkdownImageSource]) -> Vec<(usize, FeedImage)> {
    Vec::new()
}

fn settings(width: usize) -> HistoryRenderSettings {
    HistoryRenderSettings {
        width,
        max_tool_output_lines: 10,
        zen_mode: false,
        theme_generation: 0,
        max_image_height: crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT,
    }
}

fn settings_with(
    width: usize,
    max_tool_output_lines: usize,
    zen_mode: bool,
) -> HistoryRenderSettings {
    HistoryRenderSettings {
        width,
        max_tool_output_lines,
        zen_mode,
        theme_generation: 0,
        max_image_height: crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT,
    }
}

#[test]
fn caches_code_block_copy_target_and_raw_contents() {
    let mut cache = HistoryLineCache::default();
    let entries = vec![Entry::Assistant(
        "before\n```rust\nlet x = 1;\nprintln!(\"{x}\");\n```\nafter".into(),
    )];

    let blocks = cache.code_blocks(&entries, settings(40), &no_images);

    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].text.as_ref(), "let x = 1;\nprintln!(\"{x}\");");
    assert_eq!(blocks[0].line, 1);
    assert_eq!(blocks[0].copy_columns, 32..38);
}

#[test]
fn caches_unicode_wrapped_lines_and_code_copy_target_without_rendering_drift() {
    let mut cache = HistoryLineCache::default();
    let entries = vec![Entry::Assistant("你好你好你好\n```text\nλ🙂\n```".into())];
    let expected_lines = entry_lines(
        &entries[0],
        12,
        10,
        crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT,
    );

    let mut cached_lines = Vec::new();
    cache.extend_visible_lines(
        &entries,
        settings(12),
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut cached_lines,
        &no_images,
    );
    let blocks = cache.code_blocks(&entries, settings(12), &no_images);

    assert_eq!(cached_lines, expected_lines);
    assert_eq!(
        blocks,
        &[CachedCodeBlock {
            line: 2,
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
        settings(32),
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
        settings(32),
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut cached_lines,
        &no_images,
    );
    assert_eq!(
        cached_lines,
        entry_lines(
            &entries[0],
            32,
            10,
            crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT
        )
    );

    let Entry::Assistant(text) = &mut entries[0] else {
        unreachable!();
    };
    text.push_str("\n## streamed heading\n");
    cache.assistant_appended(0);
    cached_lines.clear();
    cache.extend_visible_lines(
        &entries,
        settings(32),
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut cached_lines,
        &no_images,
    );
    assert_eq!(
        cached_lines,
        entry_lines(
            &entries[0],
            32,
            10,
            crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT
        )
    );

    let Entry::Assistant(text) = &mut entries[0] else {
        unreachable!();
    };
    text.push_str("\n```rust\nlet answer = 42;\n");
    cache.assistant_appended(0);
    cached_lines.clear();
    cache.extend_visible_lines(
        &entries,
        settings(32),
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut cached_lines,
        &no_images,
    );
    assert_eq!(
        cached_lines,
        entry_lines(
            &entries[0],
            32,
            10,
            crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT
        )
    );

    let Entry::Assistant(text) = &mut entries[0] else {
        unreachable!();
    };
    text.push_str("println!(\"{answer}\");\n```\ndone\n");
    cache.assistant_appended(0);
    cached_lines.clear();
    cache.extend_visible_lines(
        &entries,
        settings(32),
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut cached_lines,
        &no_images,
    );

    assert_eq!(
        cached_lines,
        entry_lines(
            &entries[0],
            32,
            10,
            crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT
        )
    );
    assert_eq!(
        cache.code_blocks(&entries, settings(32), &no_images).len(),
        1
    );
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
        settings(80),
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut cached_lines,
        &no_images,
    );
    assert_eq!(
        cached_lines,
        entry_lines(
            &entries[0],
            80,
            10,
            crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT
        )
    );

    let Entry::Assistant(text) = &mut entries[0] else {
        unreachable!();
    };
    text.push_str("\n```");
    cache.assistant_appended(0);
    cached_lines.clear();
    cache.extend_visible_lines(
        &entries,
        settings(80),
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut cached_lines,
        &no_images,
    );

    assert_eq!(
        cached_lines,
        entry_lines(
            &entries[0],
            80,
            10,
            crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT
        )
    );
    assert_eq!(
        cache.code_blocks(&entries, settings(80), &no_images)[0]
            .text
            .as_ref(),
        "flowchart LR\nA[Parse] --> B[Render]"
    );

    let mut narrow_lines = Vec::new();
    cache.extend_visible_lines(
        &entries,
        settings(36),
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut narrow_lines,
        &no_images,
    );
    assert_eq!(
        narrow_lines,
        entry_lines(
            &entries[0],
            36,
            10,
            crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT
        )
    );
    assert_ne!(cached_lines, narrow_lines);
}

#[test]
fn resizing_keeps_mermaid_code_block_source_stable() {
    let source = crate::tui::markdown::PHASE_CHAIN_FLOWCHART;
    let entries = vec![Entry::Assistant(format!("```mermaid\n{source}\n```"))];
    let mut cache = HistoryLineCache::default();

    let mut wide = Vec::new();
    cache.extend_visible_lines(
        &entries,
        settings(100),
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut wide,
        &no_images,
    );
    let mut narrow = Vec::new();
    cache.extend_visible_lines(
        &entries,
        settings(40),
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut narrow,
        &no_images,
    );

    assert_ne!(wide, narrow);
    assert_eq!(
        wide,
        entry_lines(
            &entries[0],
            100,
            10,
            crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT
        )
    );
    assert_eq!(
        narrow,
        entry_lines(
            &entries[0],
            40,
            10,
            crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT
        )
    );
    for width in [100, 40] {
        assert_eq!(
            cache.code_blocks(&entries, settings(width), &no_images)[0]
                .text
                .as_ref(),
            source
        );
    }
}

#[test]
fn invalidating_an_assistant_entry_refreshes_code_block_contents() {
    let mut cache = HistoryLineCache::default();
    let mut entries = vec![Entry::Assistant("```\nfirst\n```".into())];
    assert_eq!(
        cache.code_blocks(&entries, settings(30), &no_images)[0]
            .text
            .as_ref(),
        "first"
    );

    entries[0] = Entry::Assistant("```\nsecond\n```".into());
    cache.invalidate_from(0);

    assert_eq!(
        cache.code_blocks(&entries, settings(30), &no_images)[0]
            .text
            .as_ref(),
        "second"
    );
}

#[test]
fn open_stream_tail_omits_trailing_blank_until_closed() {
    use crate::tui::render::{render_entry_with_options, TrailingBlank};

    let mut cache = HistoryLineCache::default();
    let entries = vec![Entry::Assistant("Hello committed line\n".into())];

    cache.set_open_stream_tail(true);
    let open_count = cache.line_count(&entries, settings(60), &no_images);
    let mut open_lines = Vec::new();
    cache.extend_visible_lines(
        &entries,
        settings(60),
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
        render_entry_with_options(
            &entries[0],
            60,
            10,
            crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT,
            TrailingBlank::Omit
        )
        .lines
    );

    cache.set_open_stream_tail(false);
    let closed_count = cache.line_count(&entries, settings(60), &no_images);
    assert_eq!(closed_count, open_count + 1);
    let mut closed_lines = Vec::new();
    cache.extend_visible_lines(
        &entries,
        settings(60),
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut closed_lines,
        &no_images,
    );
    assert_eq!(
        closed_lines,
        entry_lines(
            &entries[0],
            60,
            10,
            crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT
        )
    );
}

// Covers: zen mode suppresses tool/reasoning lines while keeping entry indices stable.
// Owner: history line cache display policy.
#[test]
fn zen_mode_hides_tool_and_reasoning_lines_and_restores_them() {
    use crate::tui::{ReasoningEntry, ToolEntry};

    let tool = Entry::Tool(ToolEntry {
        card: rho_tools::tool_card::ToolCard::new(
            rho_tools::tool_card::ToolStatus::Running,
            rho_tools::tool_card::ToolFamily::Default,
            rho_tools::tool_card::ToolHeader::call("read_file(a.rs)", None),
        ),
        expanded: false,
        image: None,
    });
    let entries = vec![
        Entry::User("hi".into()),
        tool,
        Entry::Reasoning(ReasoningEntry::new("secret plan")),
        Entry::Assistant("hello".into()),
    ];

    let mut cache = HistoryLineCache::default();
    let full = cache.line_count(&entries, settings(40), &no_images);
    assert!(full > 2);

    let zen_count = cache.line_count(&entries, settings_with(40, 10, true), &no_images);
    let user_lines = entry_lines(
        &entries[0],
        40,
        10,
        crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT,
    )
    .len();
    let assistant_lines = entry_lines(
        &entries[3],
        40,
        10,
        crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT,
    )
    .len();
    assert_eq!(zen_count, user_lines + assistant_lines);

    // Toggling zen off rebuilds the suppressed entries.
    let restored = cache.line_count(&entries, settings(40), &no_images);
    assert_eq!(restored, full);
}

// Covers: tool expand/collapse resplices only the toggled card; later assistant
// markdown is not re-rendered (line identity of the suffix is preserved).
// Owner: history line cache surgical update
#[test]
fn resplice_tool_expand_preserves_later_assistant_lines() {
    use crate::tui::ToolEntry;
    use rho_tools::tool_card::{
        DiffRow, DiffRowKind, ToolBody, ToolCard, ToolFamily, ToolHeader, ToolStatus,
    };

    // Long body so collapsed vs expanded heights differ under max_tool_output_lines=2.
    let rows: Vec<_> = (0..20)
        .map(|i| DiffRow::new(DiffRowKind::Added, Some(i + 1), format!("line_{i}")))
        .collect();
    let card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::FileDiff,
        ToolHeader::call("str_replace", Some("f.rs".into())),
    )
    .with_body(ToolBody::Diff(rows));

    let mut entries = vec![
        Entry::User("go".into()),
        Entry::Tool(ToolEntry {
            card,
            expanded: false,
            image: None,
        }),
        Entry::Assistant("# big\n\n".to_string() + &"paragraph\n\n".repeat(30)),
    ];

    let mut cache = HistoryLineCache::default();
    let width = 40usize;
    let max_lines = 2usize;
    let s = settings_with(width, max_lines, false);

    let mut before = Vec::new();
    cache.extend_visible_lines(
        &entries,
        s,
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut before,
        &no_images,
    );
    let assistant_range = cache.entry_ranges[2].clone();
    let assistant_before = before[assistant_range.clone()].to_vec();
    let total_before = before.len();

    // Expand the tool surgically.
    if let Entry::Tool(tool) = &mut entries[1] {
        tool.expanded = true;
    }
    cache.resplice_entries([1]);
    let mut after = Vec::new();
    cache.extend_visible_lines(
        &entries,
        s,
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut after,
        &no_images,
    );

    let assistant_range_after = cache.entry_ranges[2].clone();
    assert!(
        after.len() > total_before,
        "expanded tool should grow the transcript"
    );
    assert_eq!(
        &after[assistant_range_after.clone()],
        &assistant_before[..],
        "assistant suffix lines must be preserved by content"
    );
    // Range must have shifted by the tool height delta.
    let delta = after.len() as isize - total_before as isize;
    assert_eq!(
        assistant_range_after.start as isize,
        assistant_range.start as isize + delta
    );

    // Full rebuild must match surgical result (correctness oracle).
    cache.invalidate_from(0);
    let mut rebuilt = Vec::new();
    cache.extend_visible_lines(
        &entries,
        s,
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut rebuilt,
        &no_images,
    );
    assert_eq!(after, rebuilt);

    // Collapse again.
    if let Entry::Tool(tool) = &mut entries[1] {
        tool.expanded = false;
    }
    cache.resplice_entries([1]);
    let mut collapsed = Vec::new();
    cache.extend_visible_lines(
        &entries,
        s,
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut collapsed,
        &no_images,
    );
    assert_eq!(collapsed.len(), total_before);
}

// Covers: tracked image-height deps keep soft image-budget updates O(deps),
// not a markdown re-parse of every assistant entry.
// Owner: history line cache
#[test]
fn image_height_only_change_uses_cached_dependency_flags() {
    let mut cache = HistoryLineCache::default();
    let entries = vec![
        Entry::User("prompt".into()),
        Entry::Assistant("text only".into()),
        Entry::Assistant("has an image\n\n![shot](./a.png)\n".into()),
    ];
    let mut base = settings(80);
    let _ = cache.line_count(&entries, base, &no_images);
    assert_eq!(cache.image_height_dep_count, 1);
    assert_eq!(cache.image_height_deps, vec![false, false, true]);

    let renders_before = cache.entry_render_count();
    base.max_image_height = base.max_image_height.saturating_add(8);
    // Force resplice path; only the image entry should render again.
    // Without a resolver the markdown image stays a placeholder, but the
    // entry still depends on max_image_height.
    let _ = cache.line_count(&entries, base, &no_images);
    assert!(
        cache.entry_render_count() > renders_before,
        "image-bearing entries must resplice when the budget moves"
    );
}

// Covers: composer/activity height changes must not re-render a text-only
// transcript when only the feed image budget moved.
// Owner: history line cache
#[test]
fn image_height_only_change_skips_text_only_entry_renders() {
    let mut cache = HistoryLineCache::default();
    let entries = vec![
        Entry::User("prompt".into()),
        Entry::Assistant("reply with `code` and **bold**".into()),
        Entry::Reasoning(crate::tui::ReasoningEntry::new("plan")),
    ];
    let mut base = settings(80);
    let _ = cache.line_count(&entries, base, &no_images);
    let renders_after_cold = cache.entry_render_count();
    assert!(renders_after_cold >= 3);

    base.max_image_height = base.max_image_height.saturating_add(8);
    let mut lines = Vec::new();
    cache.extend_visible_lines(
        &entries,
        base,
        HistoryLineSlice {
            start: 0,
            count: usize::MAX,
        },
        &mut lines,
        &no_images,
    );
    assert_eq!(
        cache.entry_render_count(),
        renders_after_cold,
        "text-only soft image-budget updates must not re-render entries"
    );
    assert!(!lines.is_empty());
}

// Covers: mouse hit-testing still maps lines to entries after binary search.
// Owner: history line cache
#[test]
fn entry_index_at_line_finds_ranges_across_transcript() {
    let mut cache = HistoryLineCache::default();
    let entries = vec![
        Entry::User("one".into()),
        Entry::Assistant("two\nthree".into()),
        Entry::Notice("four".into()),
    ];
    let s = settings(40);
    let total = cache.line_count(&entries, s, &no_images);
    assert!(total > 3);
    assert_eq!(
        cache.entry_index_at_line(&entries, s, 0, &no_images),
        Some(0)
    );
    let assistant_start = cache.entry_ranges[1].start;
    assert_eq!(
        cache.entry_index_at_line(&entries, s, assistant_start, &no_images),
        Some(1)
    );
    let last_line = total.saturating_sub(1);
    assert_eq!(
        cache.entry_index_at_line(&entries, s, last_line, &no_images),
        Some(2)
    );
    assert_eq!(
        cache.entry_index_at_line(&entries, s, total + 5, &no_images),
        None
    );
}
