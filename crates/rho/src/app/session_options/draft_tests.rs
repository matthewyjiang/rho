use pretty_assertions::assert_eq;
use rho_protocol as wire;

use super::{DraftOptions, Origin};
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
// TUI shows the way the daemon's would, and a session that replaces this
// one carries the ones chosen.
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
        draft.origins(),
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

// Covers: `/new` takes the config it reads then for every setting nobody
// chose, so a config edit applies to the next session, and carries a chosen
// one, which then applies where the daemon takes it, while one chosen since
// must apply, so a carried setting the new session refuses never blocks
// every prompt.
// Owner: session options
#[test]
fn a_new_session_takes_the_config_but_for_chosen_settings() {
    let mut old = draft("openai-codex", "gpt-5.5", "codex");
    old.set_permission_mode(wire::PermissionMode::Supervised)
        .expect("supervised is offered");
    let edited = Config {
        provider: "xai".into(),
        model: "grok-4.7".into(),
        auth: "xai-oauth".into(),
        ..Config::default()
    };
    let mut carried = DraftOptions::carried(
        &old.options(),
        old.origins(),
        edited,
        vec!["xai-oauth".into(), "codex".into()],
        Vec::new(),
    );
    carried.set_fast_mode(true).expect("grok has fast mode");
    let options = carried.options();
    let changes = [
        wire::SessionChange::Model {
            model: options.model.current.clone(),
            auth: None,
            setup: true,
        },
        wire::SessionChange::PermissionMode {
            mode: wire::PermissionMode::Supervised,
        },
        wire::SessionChange::FastMode { enabled: true },
    ];

    assert_eq!(
        (
            options.model.current,
            options.permission_mode.current,
            changes.map(|change| carried.origin(&change)),
        ),
        (
            wire::ModelRef {
                provider: "xai".into(),
                model: "grok-4.7".into(),
            },
            wire::PermissionMode::Supervised,
            [Origin::Config, Origin::Carried, Origin::Chosen],
        )
    );
}
