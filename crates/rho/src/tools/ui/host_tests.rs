use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

const CONFIRM: &str = r#"root = Column([question, Button(Text("Merge"), action=Event("merge", {branch: "fix"}))])
question = Text("Merge?")"#;

fn request(express: &str, surface: &str) -> SurfaceRequest {
    SurfaceRequest {
        express: express.to_owned(),
        surface: Some(surface.to_owned()),
    }
}

/// A2UI's `action` message for event `name` of `component` on `surface`.
fn pressed(surface: &str, component: &str, name: &str) -> Value {
    json!({
        "version": "1.0",
        "action": {
            "name": name,
            "surfaceId": surface,
            "sourceComponentId": component,
            "timestamp": "2026-01-01T00:00:00Z",
            "context": {"branch": "fix"},
        },
    })
}

/// The id of the component of `content` that is a `kind`.
fn component_id(content: &SurfaceContent, kind: &str) -> String {
    content.messages[0]["createSurface"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|component| component["component"] == kind)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

// Covers: a call refused after the processor took its createSurface must
// not leave the surface behind, or the model's corrected call is refused
// as a duplicate and clients never see the surface the daemon holds
// Owner: surface host
#[test]
fn a_refused_call_leaves_no_surface_behind() {
    let host = SurfaceHost::default();
    let unresolvable = request(r#"root = Text(formatString("${nope()}"))"#, "s");

    let refused = host.show("call-1", &unresolvable);
    let corrected = host.show("call-2", &request(CONFIRM, "s"));

    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(
        corrected.map(|content| content.surface.clone()),
        Ok("s".to_owned())
    );
}

// Covers: replay and the live tool both show a call; the second must get
// the first's surface, id included, not a refusal for creating it twice
// Owner: surface host
#[test]
fn showing_a_call_again_returns_what_it_showed() {
    let host = SurfaceHost::default();
    let unnamed = SurfaceRequest {
        express: CONFIRM.to_owned(),
        surface: None,
    };

    let first = host.show("call-1", &unnamed).unwrap();
    let again = host.show("call-1", &unnamed).unwrap();

    assert!(Arc::ptr_eq(&first, &again));
}

// Covers: an action sent as its waiting call is cancelled must not be
// lost; the next turn delivers it
// Owner: surface host
#[test]
fn an_action_whose_call_stopped_waiting_is_kept_for_a_turn() {
    let host = SurfaceHost::default();
    let content = host.show("call-1", &request(CONFIRM, "s")).unwrap();
    let button = component_id(&content, "Button");
    let _viewer = host.viewer();
    let asked_again = request(r#"question = Text("Merge now?")"#, "s");
    drop(host.show_waiting("call-2", &asked_again).unwrap());

    host.act("s", &pressed("s", &button, "merge"), None)
        .unwrap();

    assert_eq!(
        host.take_actions(),
        vec![SurfaceAction {
            surface: "s".to_owned(),
            action: pressed("s", &button, "merge")["action"].clone(),
        }]
    );
}

// Covers: a client must not hand the model an action the surface cannot
// send
// Owner: surface host
#[test]
fn an_action_the_surface_cannot_send_is_refused() {
    let host = SurfaceHost::default();
    let content = host.show("call-1", &request(CONFIRM, "s")).unwrap();
    let button = component_id(&content, "Button");
    let text = component_id(&content, "Text");
    let cases = [
        (
            "another surface",
            "other",
            pressed("other", &button, "merge"),
        ),
        (
            "a message about another surface",
            "s",
            pressed("other", &button, "merge"),
        ),
        (
            "a component the surface lacks",
            "s",
            pressed("s", "missing", "merge"),
        ),
        (
            "a component with no action",
            "s",
            pressed("s", &text, "merge"),
        ),
        (
            "an action the component lacks",
            "s",
            pressed("s", &button, "delete"),
        ),
        ("no action message", "s", json!({"name": "merge"})),
    ];

    for (case, surface, message) in cases {
        assert!(host.act(surface, &message, None).is_err(), "{case}");
    }
    assert_eq!(host.take_actions(), Vec::new());
}

// Covers: an update to a surface the client already shows can be acted on
// at once, so the call that shows it and waits must get that action; it
// must not sit in the queue for a turn while the call waits on
// Owner: surface host
#[tokio::test]
async fn a_waiting_call_gets_the_next_action_on_a_surface_already_shown() {
    let host = SurfaceHost::default();
    let _viewer = host.viewer();
    let content = host.show("call-1", &request(CONFIRM, "s")).unwrap();
    let button = component_id(&content, "Button");
    let update = request(r#"question = Text("Merge now?")"#, "s");

    let (_, mut waiting) = host.show_waiting("call-2", &update).unwrap();
    host.act("s", &pressed("s", &button, "merge"), None)
        .unwrap();

    assert_eq!(
        (waiting.action().await.ok(), host.take_actions()),
        (
            Some(SurfaceAction {
                surface: "s".to_owned(),
                action: pressed("s", &button, "merge")["action"].clone(),
            }),
            Vec::new(),
        )
    );
}

// Covers: an action handed to a waiting call as its turn is cancelled
// must not be lost with the call; the next turn delivers it
// Owner: surface host
#[test]
fn an_action_a_cancelled_call_already_had_is_kept_for_a_turn() {
    let host = SurfaceHost::default();
    let _viewer = host.viewer();
    let (content, waiting) = host.show_waiting("call-1", &request(CONFIRM, "s")).unwrap();
    let button = component_id(&content, "Button");
    host.act("s", &pressed("s", &button, "merge"), None)
        .unwrap();

    drop(waiting);

    assert_eq!(
        host.take_actions(),
        vec![SurfaceAction {
            surface: "s".to_owned(),
            action: pressed("s", &button, "merge")["action"].clone(),
        }]
    );
}

// Covers: a call must not wait for the user while no client that draws
// surfaces follows the session, and a call refused for that must show
// nothing, or its surface id is taken by a surface nobody was sent
// Owner: surface host
#[test]
fn a_call_waits_only_while_a_client_that_draws_surfaces_follows() {
    let host = SurfaceHost::default();

    let unwatched = host.show_waiting("call-1", &request(CONFIRM, "s")).err();
    let viewer = host.viewer();
    let watched = host.show_waiting("call-2", &request(CONFIRM, "s")).is_ok();
    drop(viewer);
    let left = host.show_waiting("call-3", &request(CONFIRM, "t")).err();

    assert_eq!(
        (unwatched.is_some(), watched, left.is_some()),
        (true, true, true)
    );
}
