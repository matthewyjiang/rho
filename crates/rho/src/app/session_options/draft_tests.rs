use pretty_assertions::assert_eq;
use rho_protocol as wire;

use super::{DraftOptions, SettingOrigin, SettingOrigins};
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
    let mut carried = DraftOptions::new(
        Config::default(),
        vec!["xai-oauth".into(), "codex".into()],
        options.commands.clone(),
        Vec::new(),
    );
    carried.carry(&options, draft.origins());
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
// chose and for one `config.toml` changed since it was chosen, so a config
// edit applies to the next session, and carries the other chosen ones,
// which then apply where the daemon takes them, while one chosen since must
// apply, so a carried setting the new session refuses never blocks every
// prompt.
// Owner: session options
#[test]
fn a_new_session_takes_the_config_but_for_settings_chosen_since() {
    let mut old = draft("openai-codex", "gpt-5.5", "codex");
    let grok = wire::ModelRef {
        provider: "xai".into(),
        model: "grok-4.7".into(),
    };
    old.set_model(&grok, None)
        .expect("the draft takes the model");
    old.set_permission_mode(wire::PermissionMode::Supervised)
        .expect("supervised is offered");
    let chosen_on = Config {
        provider: "openai-codex".into(),
        model: "gpt-5.5".into(),
        auth: "codex".into(),
        ..Config::default()
    };
    let edited = Config {
        model: "gpt-5.4".into(),
        ..chosen_on.clone()
    };
    let chosen = old.origins().unchanged_between(&chosen_on, &edited);
    let mut new = draft("openai-codex", "gpt-5.4", "codex");
    new.carry(&old.options(), chosen);
    new.set_fast_mode(false).expect("fast mode turns off");
    let options = new.options();
    let origins = [
        wire::SessionChange::Model {
            model: options.model.current.clone(),
            auth: None,
            setup: true,
        },
        wire::SessionChange::PermissionMode {
            mode: wire::PermissionMode::Supervised,
        },
        wire::SessionChange::FastMode { enabled: false },
        wire::SessionChange::Reasoning {
            level: wire::ReasoningLevel::High,
        },
    ]
    .map(|change| new.origins().of(&change));

    assert_eq!(
        (
            options.model.current,
            options.permission_mode.current,
            origins,
        ),
        (
            wire::ModelRef {
                provider: "openai-codex".into(),
                model: "gpt-5.4".into(),
            },
            wire::PermissionMode::Supervised,
            [
                SettingOrigin::Config,
                SettingOrigin::Carried,
                SettingOrigin::Chosen,
                SettingOrigin::Config,
            ],
        )
    );
}

// Covers: a launch flag that picks the model carries the reasoning level
// the launch settled for it, leniently, and chooses only what it names.
// Owner: session options
#[test]
fn a_model_flag_carries_its_reasoning() {
    let flags = SettingOrigins::of_launch_flags([super::Setting::Model]);
    let reasoning = wire::SessionChange::Reasoning {
        level: wire::ReasoningLevel::High,
    };
    let permission = wire::SessionChange::PermissionMode {
        mode: wire::PermissionMode::Supervised,
    };
    assert_eq!(
        [flags.of(&reasoning), flags.of(&permission)],
        [SettingOrigin::Carried, SettingOrigin::Config]
    );
}
