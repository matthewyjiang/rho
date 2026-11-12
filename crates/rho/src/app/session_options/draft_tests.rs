use pretty_assertions::assert_eq;
use rho_protocol as wire;

use super::DraftOptions;
use crate::config::Config;

fn draft(provider: &str, model: &str, auth: &str) -> DraftOptions {
    let config = Config {
        provider: provider.into(),
        model: model.into(),
        auth: auth.into(),
        ..Config::default()
    };
    DraftOptions::new(
        config,
        vec!["xai-oauth".into(), "codex".into()],
        Vec::new(),
        Vec::new(),
    )
}

// Covers: a setting chosen before the session opens moves the options the
// TUI shows the way the daemon's would, and the settings carried to a
// session that replaces this one are the ones chosen.
// Owner: session options
#[test]
fn applied_settings_show_in_the_options_and_carry() {
    let mut draft = draft("openai-codex", "gpt-5.5", "codex");
    let grok = wire::ModelRef {
        provider: "xai".into(),
        model: "grok-4.7".into(),
    };
    draft
        .set_model(&grok, None)
        .expect("the draft takes the model");
    draft.set_fast_mode(true).expect("grok has fast mode");
    draft
        .set_permission_mode(wire::PermissionMode::Supervised)
        .expect("supervised is offered");
    let options = draft.options();

    assert_eq!(
        (
            &options.model.current,
            options.model.auth.as_str(),
            options.fast_mode.clone(),
            options.permission_mode.current,
        ),
        (
            &wire::ModelRef {
                provider: "xai".into(),
                model: "grok-4.7".into(),
            },
            "xai-oauth",
            Some(wire::FastModeOptions { enabled: true }),
            wire::PermissionMode::Supervised,
        )
    );
    let carried = DraftOptions::carried(
        &options,
        Config::default(),
        vec!["xai-oauth".into(), "codex".into()],
        Vec::new(),
    );
    assert_eq!(carried.options(), options);
}

// Covers: a setting the daemon would refuse is refused before the session
// opens and leaves the draft as it was, so what the TUI shows is never a
// choice the session will not take.
// Owner: session options
#[test]
fn refused_settings_leave_the_draft_unchanged() {
    let mut draft = draft("openai-codex", "gpt-5.4", "codex");
    let before = draft.options();
    let unknown = wire::ModelRef {
        provider: "xai".into(),
        model: "no-such-model".into(),
    };
    let refused = [
        draft.set_fast_mode(true).is_err(),
        draft
            .set_permission_mode(wire::PermissionMode::Auto)
            .is_err(),
        draft.set_model(&unknown, None).is_err(),
    ];
    assert_eq!(refused, [true; 3]);
    assert_eq!(draft.options(), before);
}

// Covers: after `/new`, settings carried from the old session apply where
// the daemon takes them, while one the person changed since must apply, so
// a carried setting the new session refuses never blocks every prompt.
// Owner: session options
#[test]
fn only_settings_chosen_since_a_carry_are_required() {
    let old = draft("openai-codex", "gpt-5.5", "codex").options();
    let mut carried = DraftOptions::carried(
        &old,
        Config::default(),
        vec!["xai-oauth".into(), "codex".into()],
        Vec::new(),
    );
    carried
        .set_permission_mode(wire::PermissionMode::Supervised)
        .expect("supervised is offered");
    let changes = [
        wire::SessionChange::Model {
            model: old.model.current.clone(),
            auth: None,
            setup: true,
        },
        wire::SessionChange::FastMode { enabled: false },
        wire::SessionChange::PermissionMode {
            mode: wire::PermissionMode::Supervised,
        },
    ];

    assert_eq!(
        changes.map(|change| carried.inherits(&change)),
        [true, true, false]
    );
}
