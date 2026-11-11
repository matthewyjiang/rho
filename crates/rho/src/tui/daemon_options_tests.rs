use pretty_assertions::assert_eq;
use rho_protocol::{ModelChoice, ModelOptions, ModelRef};

use super::{model_choice, next_choice};

// Covers: a bare id shared by two providers must not silently select the
// wrong provider; fully qualified choices work without any local catalog.
// Owner: pure daemon option selection
#[test]
fn resolve_only_unambiguous_advertised_models() {
    let first = ModelRef {
        provider: "private-host".into(),
        model: "shared".into(),
    };
    let second = ModelRef {
        provider: "other-host".into(),
        model: "shared".into(),
    };
    let options = ModelOptions {
        auth: "api-key".into(),
        current: first.clone(),
        available: vec![
            ModelChoice {
                model: first.clone(),
                label: "Private model".into(),
            },
            ModelChoice {
                model: second.clone(),
                label: "Other model".into(),
            },
        ],
    };
    for (reference, expected) in [
        ("private-host/shared", Some(first.clone())),
        ("Private model", Some(first)),
        ("other-host/shared", Some(second)),
        ("shared", None),
        ("openai/not-advertised", None),
    ] {
        assert_eq!(
            model_choice(&options, reference).ok(),
            expected,
            "{reference}"
        );
    }
}

// Covers: cycling uses the server's offered order, wraps, and does not invent
// a choice when the current value is unknown to this client.
// Owner: pure daemon option selection
#[test]
fn cycle_advertised_choices_without_catalog_fallbacks() {
    use rho_protocol::ReasoningLevel::{High, Low, Max, Unknown};
    for (current, available, expected) in [
        (Low, vec![Low, Max, High], Some(Max)),
        (High, vec![Low, Max, High], Some(Low)),
        (High, vec![High], Some(High)),
        (Unknown, vec![Low, High], None),
        (Low, vec![], None),
    ] {
        assert_eq!(
            next_choice(current, &available),
            expected,
            "{current:?}, {available:?}"
        );
    }
}

fn options_on(provider: &str, auth: &str) -> rho_protocol::SessionOptions {
    rho_protocol::SessionOptions {
        model: ModelOptions {
            auth: auth.into(),
            current: ModelRef {
                provider: provider.into(),
                model: "model".into(),
            },
            available: Vec::new(),
        },
        reasoning: None,
        fast_mode: None,
        permission_mode: rho_protocol::PermissionModeOptions {
            current: rho_protocol::PermissionMode::Bypass,
            available: Vec::new(),
        },
        commands: Vec::new(),
        added_dirs: Vec::new(),
    }
}

// Covers: the launch found no credentials for the config's provider, but
// the daemon session, a resumed one for instance, runs on another: it is
// not held signed out, from its first report on. One on that same provider
// and auth stays signed out.
// Owner: tui daemon session options
#[test]
fn a_session_on_another_provider_than_the_signed_out_one_is_not_signed_out() {
    for (provider, auth, signed_in) in [
        ("anthropic", "anthropic-api-key", true),
        ("openai", "chatgpt", true),
        ("openai", "api-key", false),
    ] {
        let mut app = crate::tui::tests::test_app();
        app.info.runtime.provider = "openai".into();
        app.info.runtime.auth = "api-key".into();
        app.info.services.auth_unavailable = Some("no credentials for openai".into());

        app.show_daemon_options(options_on(provider, auth));

        assert_eq!(app.setup_state().signed_in, signed_in, "{provider} {auth}");
    }
}

// Covers: a daemon session's first launch opens at sign-in when it is
// signed out and no other auth offers a model. The daemon always lists the
// session's own model, so a list of one is nothing to choose from.
// Owner: tui daemon session options
#[test]
fn a_signed_out_first_launch_has_a_model_to_choose_only_from_another_auth() {
    let choice = |provider: &str| ModelChoice {
        model: ModelRef {
            provider: provider.into(),
            model: "model".into(),
        },
        label: provider.into(),
    };
    for (signed_out, available, can_choose) in [
        (true, vec![choice("openai")], false),
        (true, vec![choice("openai"), choice("anthropic")], true),
        (false, vec![choice("openai")], true),
    ] {
        let mut app = crate::tui::tests::test_app();
        let mut options = options_on("openai", "api-key");
        options.model.available = available;
        app.info.runtime.provider = "openai".into();
        app.info.runtime.auth = "api-key".into();
        app.show_daemon_options(options);
        app.info.services.auth_unavailable = signed_out.then(|| "no credentials".into());

        assert_eq!(app.daemon_setup_can_choose_model(), can_choose);
    }
}

// Covers: a daemon session's model picker lists the models the daemon
// offers under the TUI's own pins and scope, as the in-process picker does,
// and a picker opened mid-turn marks the model already asked for.
// Owner: tui daemon session options
#[test]
fn the_model_picker_lays_offered_models_out_by_pin_and_pending_switch() {
    let choice = |provider: &str, model: &str| ModelChoice {
        model: ModelRef {
            provider: provider.into(),
            model: model.into(),
        },
        label: model.to_uppercase(),
    };
    let rows = |app: &crate::tui::App| {
        let picker = app.daemon_model_picker().expect("daemon options are shown");
        let rows: Vec<_> = picker
            .items
            .iter()
            .map(|item| {
                let badge = item.badge.as_ref().map(|badge| badge.text.clone());
                (item.value.clone(), badge)
            })
            .collect();
        (picker.title.clone(), rows)
    };
    let row = |value: &str, badge: Option<&str>| (value.to_owned(), badge.map(str::to_owned));
    let mut app = crate::tui::tests::test_app();
    let mut options = options_on("openai", "api-key");
    options.model.available = vec![
        choice("openai", "model"),
        choice("xai", "grok"),
        choice("poolside", "laguna"),
    ];
    app.show_daemon_options(options);

    assert_eq!(
        rows(&app),
        (
            "select model · all".to_owned(),
            vec![
                row("openai/model", Some("selected")),
                row("xai/grok", None),
                row("poolside/laguna", None),
            ]
        ),
        "no pins"
    );

    app.info.runtime.favorite_models = vec!["xai/grok".into(), "not/offered".into()];
    assert_eq!(
        rows(&app),
        (
            "select model · pinned".to_owned(),
            vec![row("xai/grok", Some("pinned"))]
        ),
        "a pin the daemon offers"
    );

    app.begin_provider_turn_ui();
    app.daemon_option_requests
        .push_back(rho_protocol::SessionChange::Model {
            model: ModelRef {
                provider: "xai".into(),
                model: "grok".into(),
            },
            auth: None,
            setup: false,
        });
    assert_eq!(
        rows(&app),
        (
            "select model for next turn · pinned".to_owned(),
            vec![row("xai/grok", Some("pinned, pending"))]
        ),
        "a switch queued behind the turn"
    );
}

/// Who asked for the model switch whose options arrive.
#[derive(Clone, Copy)]
enum Switch {
    /// `/login`, after storing credentials.
    Login,
    /// The person, with `/model`.
    Model,
    /// The person, after the daemon refused the same switch from `/login`.
    ModelAfterRefusedLogin,
}

// Covers: a model switch this TUI asked for says where it landed, except
// the one `/login` sent after storing credentials: its status line already
// names the model and the credentials, and stays. A login switch the
// daemon refused quiets nothing later (#1818).
// Owner: tui daemon session options
#[test]
fn a_switch_login_asked_for_keeps_logins_status_line() {
    const LOGIN: &str = "stored credentials for anthropic and selected anthropic/model";
    let kept: Vec<_> = [Switch::Login, Switch::Model, Switch::ModelAfterRefusedLogin]
        .into_iter()
        .map(|switch| {
            let mut app = crate::tui::tests::test_app();
            app.show_daemon_options(options_on("openai", "api-key"));
            let switched = options_on("anthropic", "anthropic-api-key");
            let model = switched.model.current.clone();
            let change = rho_protocol::SessionChange::Model {
                model: model.clone(),
                auth: None,
                setup: false,
            };
            app.daemon_option_requests.push_back(change.clone());
            match switch {
                Switch::Login => app.daemon_login_selected = Some(model),
                Switch::Model => {}
                Switch::ModelAfterRefusedLogin => {
                    app.daemon_login_selected = Some(model);
                    app.finish_daemon_option_request();
                    app.daemon_option_requests.push_back(change);
                }
            }
            app.set_status(LOGIN);

            app.show_daemon_options(switched);

            app.last_status == LOGIN
        })
        .collect();

    assert_eq!(kept, vec![true, false, false]);
}

/// When a model switch's request finished, relative to the options report
/// that follows it.
#[derive(Clone, Copy, Debug)]
enum Finished {
    /// After the report showed the switch, as the event stream orders it.
    AfterOptions,
    /// Before any report showed it, as when a reattach's response carries
    /// the options (#1880).
    BeforeOptions,
    /// Refused: the next report is on another model.
    Refused,
}

// Covers: a pinned-model switch says where it landed whether its request
// finished before or after the options showing it arrived, and a refused
// one says nothing when a later report lands elsewhere (#1880).
// Owner: tui daemon session options
#[test]
fn a_switch_says_where_it_landed_whenever_its_request_finished() {
    const BEFORE: &str = "status before the switch";
    let announced: Vec<_> = [
        Finished::AfterOptions,
        Finished::BeforeOptions,
        Finished::Refused,
    ]
    .into_iter()
    .map(|finished| {
        let mut app = crate::tui::tests::test_app();
        app.show_daemon_options(options_on("openai", "api-key"));
        let asked = options_on("anthropic", "anthropic-api-key");
        app.daemon_option_requests
            .push_back(rho_protocol::SessionChange::Model {
                model: asked.model.current.clone(),
                auth: None,
                setup: false,
            });
        app.set_status(BEFORE);
        match finished {
            Finished::AfterOptions => {
                app.show_daemon_options(asked);
                app.finish_daemon_option_request();
            }
            Finished::BeforeOptions => {
                app.finish_daemon_option_request();
                app.show_daemon_options(asked);
            }
            Finished::Refused => {
                app.finish_daemon_option_request();
                app.show_daemon_options(options_on("xai", "api-key"));
            }
        }
        app.last_status != BEFORE
    })
    .collect();

    assert_eq!(announced, vec![true, true, false]);
}
