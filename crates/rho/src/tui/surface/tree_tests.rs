use pretty_assertions::assert_eq;
use rho_sdk::ToolCallId;
use serde_json::json;

use super::*;
use crate::tui::surface::store::SurfaceStore;

/// What the keyboard reaches on the surface these `components` make.
fn controls(components: serde_json::Value) -> Vec<(String, Control)> {
    let mut store = SurfaceStore::default();
    let call = ToolCallId::from_string("call").unwrap();
    let messages = [
        json!({"version": "1.0", "createSurface": {
            "surfaceId": "s",
            "catalogId": rho_a2ui::Catalog::basic().id(),
        }}),
        json!({"version": "1.0", "updateComponents": {
            "surfaceId": "s",
            "components": components,
        }}),
    ];
    assert!(store.apply(&call, "s", &messages));
    store
        .tree("s")
        .expect("the surface has a root")
        .focusables
        .into_iter()
        .map(|focusable| (focusable.target.component, focusable.control))
        .collect()
}

fn pointer(path: &str) -> Pointer {
    Pointer::parse(path).unwrap()
}

// Covers: Tab must visit controls in the order they are drawn, one stop per
// choice, and must skip an input with nowhere to keep what the user enters.
// Owner: pure model
#[test]
fn keyboard_reaches_bound_controls_in_drawing_order() {
    let reached = controls(json!([
        {"id": "root", "component": "Column", "children": ["name", "fixed", "plan", "go"]},
        {"id": "name", "component": "TextField", "label": "Name", "value": {"@path": "/name"}},
        {"id": "fixed", "component": "TextField", "label": "Fixed", "value": "literal"},
        {"id": "plan", "component": "ChoicePicker", "value": {"@path": "/plan"}, "options": [
            {"label": "Free", "value": "free"},
            {"label": "Pro", "value": "pro"},
        ]},
        {"id": "go", "component": "Button", "child": "label", "action": {"event": {"name": "go"}}},
        {"id": "label", "component": "Text", "text": "Go"},
    ]));
    let choice = |value: &str| Control::Choice {
        binding: pointer("/plan"),
        value: value.to_owned(),
        multiple: false,
    };
    let go = Action::from_json(&json!({"event": {"name": "go"}})).unwrap();
    assert_eq!(
        reached,
        vec![
            (
                "name".to_owned(),
                Control::Field {
                    binding: pointer("/name"),
                    kind: FieldKind::Short,
                },
            ),
            ("plan".to_owned(), choice("free")),
            ("plan".to_owned(), choice("pro")),
            ("go".to_owned(), Control::Button { action: Some(go) }),
        ]
    );
}
