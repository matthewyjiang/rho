use pretty_assertions::assert_eq;

use super::CommandBlock;

fn text(block: CommandBlock) -> Vec<String> {
    block
        .finish()
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

// Covers: a field label as wide as the label column or wider, such as
// `/info`'s "Last completed" or "Generation tokens", ran into its value
// ("Last completed8184 → 7394"), and moving only that value right left the
// section's values ragged. Each section's values share one column, set by
// its widest label; other sections keep the standard column.
// Owner: command block field layout.
#[test]
fn a_section_aligns_its_values_past_its_widest_label() {
    let mut block = CommandBlock::new(60);
    block.push_section("Compaction");
    block.push_field("Completed", "1");
    block.push_field("Last completed", "8184 → 7394 local tokens");
    block.push_section("Last model call");
    block.push_field("First event", "120 ms");
    block.push_note("estimated");
    block.push_field("Generation tokens", "1,186");

    assert_eq!(
        text(block),
        [
            "",
            "Compaction",
            "  Completed      1",
            "  Last completed 8184 → 7394 local tokens",
            "",
            "Last model call",
            "  First event       120 ms",
            "  estimated",
            "  Generation tokens 1,186",
        ]
    );
}
