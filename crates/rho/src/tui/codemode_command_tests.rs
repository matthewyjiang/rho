use pretty_assertions::assert_eq;
use tempfile::tempdir;

use super::*;
use crate::{
    app::{config_repository::ConfigRepository, interactive_runtime::test_edit_tool_runtime},
    commands::parse_command,
    config::EditTool,
    session::Session as StoredSession,
    tui::tests::test_app,
};

#[derive(Default)]
struct FakeRuntime {
    mode: CodemodeMode,
    /// Each `set_codemode_mode` argument, in order.
    calls: Vec<CodemodeMode>,
}

impl CodemodeRuntime for FakeRuntime {
    fn codemode_mode(&self) -> CodemodeMode {
        self.mode
    }

    fn set_codemode_mode(&mut self, mode: CodemodeMode) -> anyhow::Result<Option<String>> {
        self.calls.push(mode);
        let changed = self.mode != mode;
        self.mode = mode;
        Ok(changed.then(|| format!("codemode {}", mode.as_str())))
    }

    fn tool_specs(&self) -> Vec<rho_sdk::model::ToolSpec> {
        Vec::new()
    }
}

/// `codemode_mode` as the `rho` diagnostics tool reports it.
fn diagnosed_mode(app: &App) -> String {
    let config: serde_json::Value =
        serde_json::from_str(&app.info.services.diagnostics.response("config").unwrap()).unwrap();
    config["codemode_mode"].as_str().unwrap().to_string()
}

fn invocation(command: &str) -> CommandInvocation {
    parse_command(command).unwrap().unwrap()
}

// Covers: /codemode on|only reach the runtime, persist as codemode.mode, and
// update the `rho` diagnostics mirror;
// bare /codemode and /codemode status only report; unknown modes (old `off`,
// Pi-less `yolo`) are rejected and change nothing.
// Owner: /codemode command
#[test]
fn codemode_command_applies_and_persists_requested_mode() {
    use CodemodeMode::{On, Only};
    struct Case {
        command: &'static str,
        initially: CodemodeMode,
        calls: Vec<CodemodeMode>,
        saved: CodemodeMode,
        rejected: bool,
    }
    let cases = [
        Case {
            command: "/codemode only",
            initially: On,
            calls: vec![Only],
            saved: Only,
            rejected: false,
        },
        Case {
            command: "/codemode on",
            initially: Only,
            calls: vec![On],
            saved: On,
            rejected: false,
        },
        Case {
            command: "/codemode",
            initially: Only,
            calls: vec![],
            saved: On,
            rejected: false,
        },
        Case {
            command: "/codemode status",
            initially: Only,
            calls: vec![],
            saved: On,
            rejected: false,
        },
        Case {
            command: "/codemode only",
            initially: Only,
            calls: vec![Only],
            saved: On,
            rejected: false,
        },
        Case {
            command: "/codemode off",
            initially: On,
            calls: vec![],
            saved: On,
            rejected: true,
        },
        Case {
            command: "/codemode yolo",
            initially: On,
            calls: vec![],
            saved: On,
            rejected: true,
        },
    ];
    for case in cases {
        let mut app = test_app();
        let mut runtime = FakeRuntime {
            mode: case.initially,
            calls: Vec::new(),
        };

        app.execute_codemode_command(invocation(case.command), &mut runtime)
            .unwrap();

        let saved = app
            .info
            .services
            .config_repository
            .load()
            .unwrap()
            .codemode
            .mode;
        let rejected = app
            .history
            .entries()
            .iter()
            .any(|entry| matches!(entry, Entry::Error(_)));
        assert_eq!(
            (runtime.calls, saved, rejected, diagnosed_mode(&app)),
            (
                case.calls,
                case.saved,
                case.rejected,
                case.saved.as_str().to_string()
            ),
            "{}",
            case.command
        );
    }
}

// Covers: a failed durable preference write restores tool exposure without
// hiding either transition from the live, model, or persisted transcript.
// Owner: /codemode compensation seam; instance-scoped save injection is not
// available through PTY, and this checks real runtime + session persistence.
#[tokio::test]
async fn failed_codemode_save_keeps_compensation_histories_aligned() {
    use rho_sdk::model::Message;

    let directory = tempdir().unwrap();
    let storage = StoredSession::create_in_root(directory.path(), directory.path()).unwrap();
    let mut app = test_app();
    let repository = ConfigRepository::temporary_for_tests().unwrap();
    repository.fail_next_save_for_tests();
    app.info.services.config_repository = repository;
    let mut runtime =
        test_edit_tool_runtime(EditTool::Pinned(rho_tools::EditFormat::Hashline)).await;
    runtime.resume(storage.clone()).await.unwrap();
    assert_eq!(runtime.session_id().as_str(), storage.id());
    let specs_before = runtime.tool_specs();
    assert!(specs_before.iter().any(|spec| spec.name == "read_file"));
    let history_before = runtime.history();

    app.execute_codemode_command(invocation("/codemode only"), &mut runtime)
        .unwrap();

    assert_eq!(runtime.codemode_mode(), CodemodeMode::On);
    assert_eq!(diagnosed_mode(&app), "on");
    assert_eq!(runtime.tool_specs(), specs_before);
    assert_eq!(
        app.info
            .services
            .config_repository
            .load()
            .unwrap()
            .codemode
            .mode,
        CodemodeMode::On
    );

    let transitions =
        [CodemodeMode::Only, CodemodeMode::On].map(crate::prompt::codemode_mode_context);
    let mut expected_model = history_before;
    expected_model.extend(
        transitions
            .iter()
            .map(|(model, _)| Message::user_text(model.clone())),
    );
    assert_eq!(runtime.history(), expected_model);
    let entries = app.history.entries();
    assert_eq!(entries.len(), transitions.len() + 1);
    let live_notices: Vec<_> = entries[..transitions.len()]
        .iter()
        .map(|entry| match entry {
            Entry::Notice(text) => Message::user_text(text.clone()),
            other => panic!("expected transition notice, got {other:?}"),
        })
        .collect();
    let expected_display: Vec<_> = transitions
        .iter()
        .map(|(_, display)| Message::user_text(display.clone()))
        .collect();
    assert_eq!(live_notices, expected_display);
    assert!(matches!(entries.last(), Some(Entry::Error(_))));
    assert_eq!(app.status(), "config save failed");

    let (_, persisted) = StoredSession::open_by_id_with_histories_in_root(
        directory.path(),
        directory.path(),
        storage.id(),
    )
    .unwrap();
    assert_eq!(persisted.model, expected_model);
    assert_eq!(persisted.display, expected_display);
    runtime.shutdown().await;
}
