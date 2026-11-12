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
// ("Last completed8184 → 7394").
// Owner: command block field layout.
#[test]
fn a_label_wider_than_the_column_keeps_a_space_before_its_value() {
    let mut block = CommandBlock::new(60);
    block.push_field("Completed", "1");
    block.push_field("Last completed", "8184 → 7394 local tokens");
    block.push_field("Generation tokens", "1,186");

    assert_eq!(
        text(block),
        [
            "  Completed     1",
            "  Last completed 8184 → 7394 local tokens",
            "  Generation tokens 1,186",
        ]
    );
}
