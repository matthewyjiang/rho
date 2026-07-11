use pretty_assertions::assert_eq;

use super::*;
use crate::tui::render::{entry_lines, render_entry};

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
#[ignore]
fn measure_single_pass_assistant_cache_rendering() {
    use std::{hint::black_box, time::Instant};

    let text = format!(
        "{}\n```rust\n{}\n```\n{}",
        "Unicode prose 你好 **bold** and `code`. ".repeat(250),
        "println!(\"你好🙂\");\n".repeat(100),
        "More wrapped prose with [docs](https://example.com). ".repeat(250)
    );
    let entry = Entry::Assistant(text);
    let iterations = 100;
    let started = Instant::now();
    for _ in 0..iterations {
        let Entry::Assistant(text) = black_box(&entry) else {
            unreachable!();
        };
        let mut in_code_block = false;
        black_box(crate::tui::markdown::render_markdown(
            text,
            78,
            &mut in_code_block,
        ));
        black_box(entry_lines(black_box(&entry), 80, 10));
    }
    let two_pass = started.elapsed();
    let started = Instant::now();
    for _ in 0..iterations {
        black_box(render_entry(black_box(&entry), 80, 10));
    }
    let single_pass = started.elapsed();
    eprintln!(
        "two_pass={two_pass:?} single_pass={single_pass:?} speedup={:.2}x",
        two_pass.as_secs_f64() / single_pass.as_secs_f64()
    );
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
