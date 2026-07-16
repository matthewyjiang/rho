use pretty_assertions::assert_eq;

use super::*;
use crate::tui::render::entry_lines;

#[test]
fn caches_code_block_copy_target_and_raw_contents() {
    let mut cache = HistoryLineCache::default();
    let entries = vec![Entry::Assistant(
        "before\n```rust\nlet x = 1;\nprintln!(\"{x}\");\n```\nafter".into(),
    )];

    let blocks = cache.code_blocks(&entries, 40, 10);

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
    cache.extend_visible_lines(&entries, 12, 10, 0, usize::MAX, &mut cached_lines);
    let blocks = cache.code_blocks(&entries, 12, 10);

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
    cache.extend_visible_lines(&entries, 32, 10, 0, usize::MAX, &mut cached_lines);

    let Entry::Assistant(text) = &mut entries[0] else {
        unreachable!();
    };
    text.push_str("--- | ---\nrow | `one`\n");
    cache.assistant_appended(0);
    cached_lines.clear();
    cache.extend_visible_lines(&entries, 32, 10, 0, usize::MAX, &mut cached_lines);
    assert_eq!(cached_lines, entry_lines(&entries[0], 32, 10));

    let Entry::Assistant(text) = &mut entries[0] else {
        unreachable!();
    };
    text.push_str("\n## streamed heading\n");
    cache.assistant_appended(0);
    cached_lines.clear();
    cache.extend_visible_lines(&entries, 32, 10, 0, usize::MAX, &mut cached_lines);
    assert_eq!(cached_lines, entry_lines(&entries[0], 32, 10));

    let Entry::Assistant(text) = &mut entries[0] else {
        unreachable!();
    };
    text.push_str("\n```rust\nlet answer = 42;\n");
    cache.assistant_appended(0);
    cached_lines.clear();
    cache.extend_visible_lines(&entries, 32, 10, 0, usize::MAX, &mut cached_lines);
    assert_eq!(cached_lines, entry_lines(&entries[0], 32, 10));

    let Entry::Assistant(text) = &mut entries[0] else {
        unreachable!();
    };
    text.push_str("println!(\"{answer}\");\n```\ndone\n");
    cache.assistant_appended(0);
    cached_lines.clear();
    cache.extend_visible_lines(&entries, 32, 10, 0, usize::MAX, &mut cached_lines);

    assert_eq!(cached_lines, entry_lines(&entries[0], 32, 10));
    assert_eq!(cache.code_blocks(&entries, 32, 10).len(), 1);
    assert!(cache.assistant_caches[0]
        .is_some_and(|cached| cached.stable_source_len > "intro\n\n".len()));
}

#[test]
fn invalidating_an_assistant_entry_refreshes_code_block_contents() {
    let mut cache = HistoryLineCache::default();
    let mut entries = vec![Entry::Assistant("```\nfirst\n```".into())];
    assert_eq!(
        cache.code_blocks(&entries, 30, 10)[0].text.as_ref(),
        "first"
    );

    entries[0] = Entry::Assistant("```\nsecond\n```".into());
    cache.invalidate_from(0);

    assert_eq!(
        cache.code_blocks(&entries, 30, 10)[0].text.as_ref(),
        "second"
    );
}
