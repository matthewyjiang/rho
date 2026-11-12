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
    for change in &changes {
        draft.apply(change).expect("the draft takes the change");
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
    let refused = [
        wire::SessionChange::FastMode { enabled: true },
        wire::SessionChange::PermissionMode {
            mode: wire::PermissionMode::Auto,
        },
        wire::SessionChange::Model {
            model: wire::ModelRef {
                provider: "xai".into(),
                model: "no-such-model".into(),
            },
            auth: None,
            setup: false,
        },
        wire::SessionChange::SelectNode { node: "n1".into() },
    ];
    for change in &refused {
        assert!(draft.apply(change).is_err(), "{change:?} is refused");
    }
    assert_eq!(draft.options(), before);
}
