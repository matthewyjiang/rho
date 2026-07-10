use pretty_assertions::assert_eq;

use super::*;

#[test]
fn caches_code_block_copy_target_and_raw_contents() {
    let mut cache = HistoryLineCache::default();
    let entries = vec![Entry::Assistant(
        "before\n```rust\nlet x = 1;\nprintln!(\"{x}\");\n```\nafter".into(),
    )];

    let blocks = cache.code_blocks(&entries, 40, 10);

    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].text, "let x = 1;\nprintln!(\"{x}\");");
    assert_eq!(blocks[0].line, 2);
    assert_eq!(blocks[0].copy_columns, 32..38);
}

#[test]
fn invalidating_an_assistant_entry_refreshes_code_block_contents() {
    let mut cache = HistoryLineCache::default();
    let mut entries = vec![Entry::Assistant("```\nfirst\n```".into())];
    assert_eq!(cache.code_blocks(&entries, 30, 10)[0].text, "first");

    entries[0] = Entry::Assistant("```\nsecond\n```".into());
    cache.invalidate_from(0);

    assert_eq!(cache.code_blocks(&entries, 30, 10)[0].text, "second");
}
