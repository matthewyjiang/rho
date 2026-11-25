use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

// Covers: actions that describe an argument with the same text share one
// clause naming all of them, so the model reads the text once, while actions
// whose text differs keep their own clause.
// Owner: sessions tool schema, generated from the session-control core.
#[test]
fn shared_argument_descriptions_appear_once() {
    let mut schema = json!({"properties": {"action": {"enum": []}}});
    extend_schema(&mut schema);

    let mut shared = 0;
    let mut problems = Vec::new();
    for (key, merged) in schema["properties"].as_object().unwrap() {
        let merged = merged["description"].as_str().unwrap_or_default();
        let mut texts: Vec<(String, Vec<&str>)> = Vec::new();
        for action in Action::ALL {
            let mut own = Value::Object(action.input_schema());
            let defs = own.get("$defs").cloned().unwrap_or(Value::Null);
            inline_refs(&mut own, &defs);
            let Some(text) = own["properties"][key]["description"].as_str() else {
                continue;
            };
            match texts.iter_mut().find(|(said, _)| said == text) {
                Some((_, actions)) => actions.push(action.name()),
                None => texts.push((text.to_owned(), vec![action.name()])),
            }
        }
        for (text, actions) in texts {
            shared += usize::from(actions.len() > 1);
            let clause = format!("For {}: {text}", actions.join(", "));
            if merged.matches(text.as_str()).count() != 1 || !merged.contains(&clause) {
                problems.push(format!("{key}: expected `{clause}` once in `{merged}`"));
            }
        }
    }

    assert_eq!(problems, Vec::<String>::new());
    assert!(shared > 0, "no argument is described alike by two actions");
}
