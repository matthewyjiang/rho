use pretty_assertions::assert_eq;
use rho_protocol::{ModelChoice, ModelRef};
use rho_providers::model::{
    display_name::ModelDisplayNameCacheGuard,
    models_dev::{
        with_models_dev_cache_dir_for_tests, write_cached_model_metadata_for_tests, ModelMetadata,
    },
    provider_models::with_provider_models_cache_dir_for_tests,
};

use super::ModelNames;

/// Runs `f` against empty catalog caches holding only the given
/// `(provider, model, name)` rows, so names never come from the developer's
/// real models.dev snapshot.
pub(in crate::tui) fn with_catalog_names<T>(
    names: &[(&str, &str, &str)],
    f: impl FnOnce() -> T,
) -> T {
    let catalog = tempfile::tempdir().unwrap();
    let provider = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir_for_tests(catalog.path().to_path_buf(), || {
        with_provider_models_cache_dir_for_tests(provider.path().to_path_buf(), || {
            let _names = ModelDisplayNameCacheGuard::new();
            for (provider_name, model, display_name) in names {
                let mut metadata = ModelMetadata::default();
                metadata.display_name = Some((*display_name).into());
                write_cached_model_metadata_for_tests(provider_name, model, &metadata);
            }
            f()
        })
    })
}

/// A daemon choice of `provider/model` labeled `label`.
pub(in crate::tui) fn offered(provider: &str, model: &str, label: &str) -> ModelChoice {
    ModelChoice {
        model: ModelRef {
            provider: provider.into(),
            model: model.into(),
        },
        label: label.into(),
    }
}

// Covers: chrome names a model by its display name from the daemon's labels
// or the local catalog, and falls back to the id when neither names it. A
// daemon label that is the id, alone or with its provider, keeps the id even
// when the local catalog has a name, since the daemon uses the id to tell
// apart models that share one.
// Owner: TUI model naming
#[test]
fn models_read_by_name_when_known_and_by_id_otherwise() {
    let daemon = [
        offered(
            "cliproxyapi",
            "anthropic/claude-opus-5-5",
            "Claude Opus 5.5",
        ),
        offered("openai", "gpt-5.5", "gpt-5.5"),
        offered("openai-codex", "gpt-5.5", "gpt-5.5 (openai-codex)"),
    ];
    with_catalog_names(
        &[
            ("anthropic", "claude-fable-5-2", "Claude Fable 5.2"),
            ("openai", "gpt-5.5", "GPT-5.5"),
        ],
        || {
            for (case, names, provider, model, name, reference) in [
                (
                    "daemon label",
                    ModelNames::new(&daemon),
                    "cliproxyapi",
                    "anthropic/claude-opus-5-5",
                    Some("Claude Opus 5.5"),
                    "Claude Opus 5.5",
                ),
                (
                    "local catalog",
                    ModelNames::default(),
                    "anthropic",
                    "claude-fable-5-2",
                    Some("Claude Fable 5.2"),
                    "Claude Fable 5.2",
                ),
                (
                    "daemon id label",
                    ModelNames::new(&daemon),
                    "openai",
                    "gpt-5.5",
                    None,
                    "openai/gpt-5.5",
                ),
                (
                    "daemon id label with its provider",
                    ModelNames::new(&daemon),
                    "openai-codex",
                    "gpt-5.5",
                    None,
                    "openai-codex/gpt-5.5",
                ),
                (
                    "unnamed",
                    ModelNames::default(),
                    "cliproxyapi",
                    "anthropic/claude-fable-5-2",
                    None,
                    "cliproxyapi/anthropic/claude-fable-5-2",
                ),
            ] {
                assert_eq!(
                    (
                        names.name(provider, model),
                        names.reference(provider, model)
                    ),
                    (name.map(String::from), reference.to_string()),
                    "{case}"
                );
            }
        },
    );
}

/// The footer as drawn at a width that keeps the model field.
fn footer(app: &mut crate::tui::App) -> String {
    app.statusline_lines(/*width*/ 120)
        .iter()
        .flat_map(|line| line.spans.iter().map(|span| span.content.to_string()))
        .collect()
}

/// Session options on `provider/model`, offered with `label`.
fn daemon_options(provider: &str, model: &str, label: &str) -> rho_protocol::SessionOptions {
    rho_protocol::SessionOptions {
        model: rho_protocol::ModelOptions {
            auth: "api-key".into(),
            current: ModelRef {
                provider: provider.into(),
                model: model.into(),
            },
            available: vec![offered(provider, model, label)],
        },
        reasoning: None,
        fast_mode: None,
        permission_mode: rho_protocol::PermissionModeOptions {
            current: rho_protocol::PermissionMode::Bypass,
            available: Vec::new(),
        },
        commands: Vec::new(),
        goal: None,
        added_dirs: Vec::new(),
    }
}

// Covers: the footer names the session's model by its display name, from the
// local catalog in process and from the daemon's label on a daemon session,
// and shows the id when neither names it.
// Owner: TUI model naming
#[test]
fn the_footer_names_the_model_in_process_and_on_a_daemon() {
    let model = "anthropic/claude-opus-5-5";
    for (case, catalog, label, shown, hidden) in [
        ("in process, named", true, None, "GPT-5.5", "gpt-5.5"),
        ("in process, unnamed", false, None, "gpt-5.5", "GPT-5.5"),
        (
            "daemon, named",
            false,
            Some("Claude Opus 5.5"),
            "Claude Opus 5.5",
            model,
        ),
        (
            "daemon, unnamed",
            false,
            Some(model),
            model,
            "Claude Opus 5.5",
        ),
    ] {
        let rows: &[(&str, &str, &str)] = if catalog {
            &[("openai", "gpt-5.5", "GPT-5.5")]
        } else {
            &[]
        };
        let footer = with_catalog_names(rows, || {
            let mut app = crate::tui::tests::test_app();
            if let Some(label) = label {
                app.show_daemon_options(daemon_options("cliproxyapi", model, label));
            }
            footer(&mut app)
        });
        assert_eq!(
            (footer.contains(shown), footer.contains(hidden)),
            (true, false),
            "{case}: {footer}"
        );
    }
}

// Covers: a daemon model switch this TUI asked for names the model it landed
// on by the daemon's label.
// Owner: TUI model naming
#[test]
fn a_daemon_switch_names_the_model_it_landed_on() {
    with_catalog_names(&[], || {
        let mut app = crate::tui::tests::test_app();
        app.show_daemon_options(daemon_options("openai", "gpt-5.5", "gpt-5.5"));
        let switched = daemon_options(
            "cliproxyapi",
            "anthropic/claude-opus-5-5",
            "Claude Opus 5.5",
        );
        app.daemon_option_requests
            .push_back(rho_protocol::SessionChange::Model {
                model: switched.model.current.clone(),
                auth: None,
                setup: false,
            });

        app.show_daemon_options(switched);

        assert_eq!(
            app.last_status,
            "model switched to Claude Opus 5.5 with reasoning off"
        );
    });
}
