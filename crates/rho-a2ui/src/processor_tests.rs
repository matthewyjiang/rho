use serde_json::json;

use super::*;

fn processor() -> MessageProcessor {
    let mut processor = MessageProcessor::new(Validation::Check(ValidationConfig::STRICT));
    processor
        .add_catalog(Catalog::basic().clone(), Functions::basic())
        .expect("the basic catalog compiles");
    processor
}

fn update(components: &Value) -> Value {
    json!({ "version": "1.0", "updateComponents": { "surfaceId": "s", "components": components } })
}

fn change(make: fn(String, String) -> Change, component: &str) -> Change {
    make("s".to_owned(), component.to_owned())
}

/// The changes a view acts on: what each message added, replaced or
/// removed, the watchers a data write fired, and nothing at all for a
/// rejected message, which leaves the surface as it was.
#[test]
fn processing_reports_what_changed() {
    let mut processor = processor();
    let created = processor.process(&json!({ "version": "1.0", "createSurface": {
        "surfaceId": "s",
        "catalogId": Catalog::basic().id(),
        "dataModel": { "name": "Ada" },
        "components": [
            { "id": "root", "component": "Column", "children": ["title"] },
            { "id": "title", "component": "Text", "text": { "@path": "/name" } },
        ],
    }}));
    let added = |component| {
        change(
            |surface, component| Change::ComponentAdded { surface, component },
            component,
        )
    };
    let want = vec![
        Change::SurfaceCreated {
            surface: "s".to_owned(),
        },
        added("root"),
        added("title"),
    ];
    assert_eq!((created.changes, created.outbound), (want, Vec::new()));

    let name = Pointer::parse("/name").expect("a valid pointer");
    let watch = processor.surface_mut("s").expect("surface s").watch(&name);
    let wrote = processor.process(&json!({ "version": "1.0",
        "updateDataModel": { "surfaceId": "s", "path": "/name", "value": "Grace" } }));
    let want = vec![Change::DataUpdated {
        surface: "s".to_owned(),
        path: name,
        fired: vec![watch],
    }];
    assert_eq!(wrote.changes, want);

    let replaced = processor.process(&update(&json!([
        { "id": "root", "component": "Column", "children": ["title"] },
        { "id": "title", "component": "Image", "url": "https://example.com/a.png" },
    ])));
    let want = vec![
        change(
            |surface, component| Change::ComponentUpdated { surface, component },
            "root",
        ),
        change(
            |surface, component| Change::ComponentRemoved { surface, component },
            "title",
        ),
        added("title"),
    ];
    assert_eq!(replaced.changes, want);

    let before = processor
        .surface("s")
        .expect("surface s")
        .components()
        .to_vec();
    let cycle = processor.process(&update(&json!([
        { "id": "root", "component": "Column", "children": ["title", "loop"] },
        { "id": "loop", "component": "Card", "child": "loop" },
    ])));
    let codes: Vec<ErrorCode> = cycle.errors().map(ErrorMessage::code).collect();
    assert_eq!(
        (cycle.changes, codes),
        (Vec::new(), vec![ErrorCode::RecursionLimitExceeded])
    );
    assert_eq!(
        processor.surface("s").expect("surface s").components(),
        before
    );
}
