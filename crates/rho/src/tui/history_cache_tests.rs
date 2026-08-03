use pretty_assertions::assert_eq;

use super::*;
use crate::tui::render::entry_lines;

fn no_images(_: usize, _: &[MarkdownImageSource]) -> Vec<(usize, FeedImage)> {
    Vec::new()
}

fn settings(width: usize) -> HistoryRenderSettings {
    HistoryRenderSettings::new(width, 10, false)
}

fn settings_with(
    width: usize,
    max_tool_output_lines: usize,
    zen_mode: bool,
) -> HistoryRenderSettings {
    HistoryRenderSettings::new(width, max_tool_output_lines, zen_mode)
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
    let expected_lines = entry_lines(&entries[0], 12, 10);

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
    assert_eq!(cached_lines, entry_lines(&entries[0], 32, 10));

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
    assert_eq!(cached_lines, entry_lines(&entries[0], 32, 10));

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
    assert_eq!(cached_lines, entry_lines(&entries[0], 32, 10));

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

    assert_eq!(cached_lines, entry_lines(&entries[0], 32, 10));
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
    assert_eq!(cached_lines, entry_lines(&entries[0], 80, 10));

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

    assert_eq!(cached_lines, entry_lines(&entries[0], 80, 10));
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
    assert_eq!(narrow_lines, entry_lines(&entries[0], 36, 10));
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
    assert_eq!(wide, entry_lines(&entries[0], 100, 10));
    assert_eq!(narrow, entry_lines(&entries[0], 40, 10));
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
        render_entry_with_options(&entries[0], 60, 10, TrailingBlank::Omit).lines
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
    assert_eq!(closed_lines, entry_lines(&entries[0], 60, 10));
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
    let user_lines = entry_lines(&entries[0], 40, 10).len();
    let assistant_lines = entry_lines(&entries[3], 40, 10).len();
    assert_eq!(zen_count, user_lines + assistant_lines);

    // Toggling zen off rebuilds the suppressed entries.
    let restored = cache.line_count(&entries, settings(40), &no_images);
    assert_eq!(restored, full);
}
