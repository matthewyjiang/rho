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

fn picker_title(app: &App) -> String {
    let ComposerMode::Picker(picker) = app.input_ui.composer() else {
        panic!("model picker should be open");
    };
    picker.title.clone()
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
    open_model_picker(&mut app);
    assert!(picker_title(&app).contains("pinned"));
    assert_eq!(picker_values(&app), vec!["xai/grok-4.6".to_string()]);

    app.toggle_model_picker_scope().unwrap();
    assert!(picker_title(&app).contains("all"));
    assert!(picker_values(&app).len() > 1);
    assert_eq!(app.status(), "showing all models");

    // The session remembers the choice, so reopening stays on all.
    open_model_picker(&mut app);
    assert!(picker_title(&app).contains("all"));

    app.toggle_model_picker_scope().unwrap();
    assert!(picker_title(&app).contains("pinned"));
    assert_eq!(app.status(), "showing pinned models");
}

// Covers: toggling to pinned with no usable pin must report and leave both the
// picker and the remembered scope untouched, never flip silently.
// Owner: model picker scope
#[test]
fn scope_toggle_refuses_an_empty_pinned_view() {
    let mut app = app_with_pins(&[]);
    open_model_picker(&mut app);
    let before = picker_values(&app);
    assert!(picker_title(&app).contains("all"));

    app.toggle_model_picker_scope().unwrap();

    assert_eq!(app.status(), "no pinned models");
    assert!(picker_title(&app).contains("all"));
    assert_eq!(picker_values(&app), before);
    assert_eq!(app.model_picker_scope_override, None);
}

// Covers: unpinning the last usable pin while the pinned view is open must
// fall back to the catalogue instead of leaving an empty list.
// Owner: model picker scope
#[test]
fn unpinning_the_last_pin_falls_back_to_all() {
    let mut app = app_with_pins(&["xai/grok-4.6"]);
    open_model_picker(&mut app);
    assert!(picker_title(&app).contains("pinned"));

    app.toggle_selected_model_favorite().unwrap();

    assert!(picker_title(&app).contains("all"));
    assert!(picker_values(&app).len() > 1);
    assert!(app.info.runtime.favorite_models.is_empty());
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
    assert!(picker_title(&app).contains("all"));
    assert_eq!(app.model_picker_scope_override, None);

    app.info
        .services
        .config_repository
        .update(|config| config.favorite_models = vec!["xai/grok-4.6".into()])
        .unwrap();
    app.info.runtime.favorite_models = vec!["xai/grok-4.6".into()];

    open_model_picker(&mut app);
    assert!(picker_title(&app).contains("pinned"));
    assert_eq!(picker_values(&app), vec!["xai/grok-4.6".to_string()]);
    assert_eq!(app.model_picker_scope_override, None);
}
