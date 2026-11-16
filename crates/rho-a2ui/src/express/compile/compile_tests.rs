use serde_json::json;

use super::*;

/// An inline component in a list takes its 0-based index, as the
/// reference compiler and the A2UI documentation number it. The suite
/// leaves the base open: its one case collides either way.
#[test]
fn inline_list_items_count_from_zero() {
    let express = Express::new(vec![Catalog::basic().clone()]).expect("the basic catalog loads");
    let messages = express
        .compile(r#"root = Column([Text("a"), Text("b")])"#, "main")
        .expect("the block compiles");
    assert_eq!(
        messages[0]["createSurface"]["components"][0]["children"],
        json!(["root_children_0", "root_children_1"])
    );
}
