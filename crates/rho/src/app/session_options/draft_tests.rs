use pretty_assertions::assert_eq;
use rho_protocol as wire;

use std::sync::Arc;

use super::{DraftOptions, LaunchFlag, SettingOrigin, SettingOrigins};
use crate::{config::Config, permission::PermissionMode};

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
    let file = Arc::new(Config::default());
    let changes = [
        wire::SessionChange::Model {
            model: wire::ModelRef {
                provider: "xai".into(),
                model: "grok-4.7".into(),
            },
            auth: None,
            setup: false,
        },
        wire::SessionChange::FastMode { enabled: true },
        wire::SessionChange::PermissionMode {
            mode: wire::PermissionMode::Supervised,
        },
    ];
    for change in changes {
        match &change {
            wire::SessionChange::Model { model, .. } => draft.set_model(model, None),
            wire::SessionChange::FastMode { enabled } => draft.set_fast_mode(*enabled),
            wire::SessionChange::PermissionMode { mode } => draft.set_permission_mode(*mode),
            _ => unreachable!("only settings above"),
        }
        .expect("the draft takes the setting");
        draft.chose(&change, Arc::clone(&file));
    }
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

/// `config.toml` with `model`, `reasoning`, and `permission_mode`.
fn file(model: &str, reasoning: rho_sdk::ReasoningLevel, mode: PermissionMode) -> Arc<Config> {
    Arc::new(Config {
        provider: "openai-codex".into(),
        model: model.into(),
        auth: "codex".into(),
        reasoning,
        permission_mode: mode,
        ..Config::default()
    })
}

// Covers: a choice holds in the next new session while `config.toml`
// configures that setting as when it was made, so an edit since a pick, or
// since the launch for the reasoning a model flag carries and for a bare
// `--auth`, wins over it, a pick made after an edit holds, and a launch flag
// naming the model always holds.
// Owner: session options
#[test]
fn a_choice_holds_until_config_changes_its_setting() {
    use rho_sdk::ReasoningLevel::{High, Low};
    use PermissionMode::{Bypass, Supervised};

    let launch = file("gpt-5.5", High, Bypass);
    let model = wire::SessionChange::Model {
        model: wire::ModelRef {
            provider: "openai-codex".into(),
            model: "gpt-5.4".into(),
        },
        auth: None,
        setup: false,
    };
    let reasoning = wire::SessionChange::Reasoning {
        level: wire::ReasoningLevel::High,
    };
    let permission = wire::SessionChange::PermissionMode {
        mode: wire::PermissionMode::Supervised,
    };
    let flags = || SettingOrigins::of_launch_flags([LaunchFlag::Model], Arc::clone(&launch));
    let auth_flag = || SettingOrigins::of_launch_flags([LaunchFlag::Auth], Arc::clone(&launch));
    let picked = |made_on: &Arc<Config>| {
        let mut picks = SettingOrigins::default();
        picks.choose(&permission, Arc::clone(made_on));
        picks
    };
    let edited = file("gpt-5.5", High, Supervised);
    let (config, carried, chosen) = (
        SettingOrigin::Config,
        SettingOrigin::Carried,
        SettingOrigin::Chosen,
    );
    // (origins, config.toml now, expected origins of model, reasoning,
    // and permission mode)
    let cases = [
        // A model flag holds; the reasoning it carries yields to an edit.
        (
            flags(),
            file("gpt-5.4", Low, Bypass),
            [chosen, config, config],
        ),
        (
            flags(),
            file("gpt-5.4", High, Bypass),
            [chosen, carried, config],
        ),
        // A bare `--auth` holds until an edit of the model.
        (auth_flag(), Arc::clone(&launch), [chosen, config, config]),
        (
            auth_flag(),
            file("gpt-5.4", High, Bypass),
            [config, config, config],
        ),
        // A pick holds until an edit of its setting after it.
        (
            picked(&launch),
            Arc::clone(&launch),
            [config, config, chosen],
        ),
        (
            picked(&launch),
            Arc::clone(&edited),
            [config, config, config],
        ),
        (
            picked(&edited),
            Arc::clone(&edited),
            [config, config, chosen],
        ),
    ];

    let held = cases
        .iter()
        .map(|(origins, now, _)| {
            let origins = origins.clone().holding_on(now);
            [&model, &reasoning, &permission].map(|change| origins.of(change))
        })
        .collect::<Vec<_>>();
    let expected = cases
        .iter()
        .map(|(.., expected)| *expected)
        .collect::<Vec<_>>();
    assert_eq!(held, expected);
}

// Covers: `/new` takes the config it reads then for every setting nobody
// chose, carries a chosen one, which then applies where the daemon takes
// it, while one chosen since must apply, so a carried setting the new
// session refuses never blocks every prompt.
// Owner: session options
#[test]
fn a_new_session_carries_only_chosen_settings() {
    let made_on = Arc::new(Config::default());
    let mut old = draft("openai-codex", "gpt-5.5", "codex");
    let permission = wire::SessionChange::PermissionMode {
        mode: wire::PermissionMode::Supervised,
    };
    old.set_permission_mode(wire::PermissionMode::Supervised)
        .expect("supervised is offered");
    old.chose(&permission, Arc::clone(&made_on));
    let mut new = draft("openai-codex", "gpt-5.4", "codex");
    new.carry(&old.options(), old.origins());
    let fast = wire::SessionChange::FastMode { enabled: false };
    new.set_fast_mode(false).expect("fast mode turns off");
    new.chose(&fast, made_on);
    let options = new.options();
    let origins = [
        wire::SessionChange::Model {
            model: options.model.current.clone(),
            auth: None,
            setup: true,
        },
        permission,
        fast,
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
            ],
        )
    );
}
