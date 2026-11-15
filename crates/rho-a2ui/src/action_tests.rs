use serde_json::json;

use super::*;
use crate::catalog::Catalog;
use crate::functions::Functions;
use crate::processor::Validation;

/// What a dispatch hands the host beyond the `action` message: the
/// surface's data model, when it was created with `sendDataModel`, and
/// the URL an `openUrl` call asks to open.
#[test]
fn dispatch_hands_the_host_the_data_model_and_effects() {
    let mut processor = MessageProcessor::new(Validation::Skip);
    processor
        .add_catalog(Catalog::basic().clone(), Functions::basic())
        .expect("the basic catalog compiles");
    let created = processor.process(&json!({ "version": "1.0", "createSurface": {
        "surfaceId": "s", "catalogId": Catalog::basic().id(), "sendDataModel": true,
        "dataModel": { "email": "ada@example.com" },
    }}));
    assert_eq!(created.outbound, []);
    let interaction = Interaction {
        surface_id: "s",
        source_component_id: "submit",
        scope: Scope::root(),
        activation: UserActivation::Present,
        metadata: None,
        at: UNIX_EPOCH,
    };
    let submit = Action::from_json(&json!({ "event": { "name": "submit",
        "context": { "email": { "@path": "/email" } } } }))
    .expect("a valid action");
    let dispatched = processor
        .dispatch_action(&interaction, &submit)
        .expect("surface s");
    let sent: Vec<Value> = dispatched
        .outbound
        .iter()
        .map(RendererMessage::to_json)
        .collect();
    let want = json!({ "version": "1.0", "action": { "name": "submit", "surfaceId": "s",
        "sourceComponentId": "submit", "timestamp": "1970-01-01T00:00:00.000Z",
        "context": { "email": "ada@example.com" } } });
    assert_eq!(sent, [want]);
    let model = json!({ "version": "1.0", "surfaces": { "s": { "email": "ada@example.com" } } });
    assert_eq!(dispatched.data_model, Some(model));

    let open = Action::from_json(&json!({ "functionCall": {
        "@call": "openUrl", "args": { "url": "https://example.com" } } }))
    .expect("a valid action");
    let dispatched = processor
        .dispatch_action(&interaction, &open)
        .expect("surface s");
    assert_eq!(
        (dispatched.outbound, dispatched.effects),
        (
            Vec::new(),
            vec![Effect::OpenUrl("https://example.com".to_owned())]
        )
    );
}
