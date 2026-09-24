use pretty_assertions::assert_eq;

use super::super::{tests::test_app, App, ComposerMode};

fn app_with_pins(pins: &[&str]) -> App {
    let mut app = test_app();
    // OpenAI models come from a cache these unit tests do not populate.
    // xAI is a static-catalog provider, so a stored key makes pins usable.
    rho_providers::credentials::save_provider_api_key(
        app.credential_store.as_ref(),
        "xai",
        "xai-test",
    )
    .unwrap();
    let pins = pins.iter().map(|pin| pin.to_string()).collect::<Vec<_>>();
    app.info
        .services
        .config_repository
        .update(|config| config.favorite_models = pins.clone())
        .unwrap();
    app.info.runtime.favorite_models = pins;
    app.refresh_available_auths();
    app
}

fn open_model_picker(app: &mut App) {
    let picker = app.conversation_model_picker();
    app.input_ui.set_composer(ComposerMode::Picker(picker));
}

fn picker_values(app: &App) -> Vec<String> {
    let ComposerMode::Picker(picker) = app.input_ui.composer() else {
        panic!("model picker should be open");
    };
    picker.items.iter().map(|item| item.value.clone()).collect()
}

// Covers: the scope toggle must flip the open picker between pinned and all,
// keep the choice for the rest of the session, and refuse to leave the session
// on a pinned view that has nothing to show.
// Owner: model picker scope
#[test]
fn scope_toggle_flips_the_open_picker_and_sticks() {
    let mut app = app_with_pins(&["xai/grok-4.6"]);
    let pinned = vec!["xai/grok-4.6".to_string()];
    open_model_picker(&mut app);
    assert_eq!(picker_values(&app), pinned);

    app.toggle_model_picker_scope().unwrap();
    assert!(picker_values(&app).len() > 1);

    // The session remembers the choice, so reopening stays on all.
    open_model_picker(&mut app);
    assert!(picker_values(&app).len() > 1);

    app.toggle_model_picker_scope().unwrap();
    assert_eq!(picker_values(&app), pinned);
}

// Covers: toggling to pinned with no usable pin must report and leave both the
// picker and the remembered scope untouched, never flip silently.
// Owner: model picker scope
#[test]
fn scope_toggle_refuses_an_empty_pinned_view() {
    let mut app = app_with_pins(&[]);
    open_model_picker(&mut app);
    let before = picker_values(&app);

    app.toggle_model_picker_scope().unwrap();

    assert_eq!(picker_values(&app), before);
    assert_eq!(app.model_picker_scope_override, None);
}

// Covers: a rebuild must keep the parent picker so users opened from /config
// still have a way back after pinning or switching scope.
// Owner: model picker scope
#[test]
fn rebuilding_keeps_the_parent_picker() {
    let mut app = app_with_pins(&["xai/grok-4.6"]);
    let parent = crate::tui::provider_picker::login_group_picker();
    let picker = app.conversation_model_picker().with_parent(parent);
    app.input_ui.set_composer(ComposerMode::Picker(picker));

    app.toggle_model_picker_scope().unwrap();

    let ComposerMode::Picker(picker) = app.input_ui.composer() else {
        panic!("model picker should still be open");
    };
    assert!(picker.has_parent(), "scope toggle must keep the parent");
}

// Covers: a session that first opened /model with no usable pin must still
// open on the pinned list once a pin has auth, without the user pressing
// the scope toggle.
// Owner: model picker scope
#[test]
fn first_open_without_pins_promotes_after_a_pin_is_added() {
    let mut app = app_with_pins(&[]);
    open_model_picker(&mut app);
    assert!(picker_values(&app).len() > 1);
    assert_eq!(app.model_picker_scope_override, None);

    app.info
        .services
        .config_repository
        .update(|config| config.favorite_models = vec!["xai/grok-4.6".into()])
        .unwrap();
    app.info.runtime.favorite_models = vec!["xai/grok-4.6".into()];

    open_model_picker(&mut app);
    assert_eq!(picker_values(&app), vec!["xai/grok-4.6".to_string()]);
    assert_eq!(app.model_picker_scope_override, None);
}
