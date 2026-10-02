use pretty_assertions::assert_eq;
use tempfile::tempdir;

use super::*;
use crate::{
    app::config_repository::ConfigRepository, commands::parse_command, tui::tests::test_app,
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

fn invocation(command: &str) -> CommandInvocation {
    parse_command(command).unwrap().unwrap()
}

// Covers: /codemode on|only reach the runtime and persist as codemode.mode;
// bare /codemode only reports; unknown modes (old `off`, Pi-less `yolo`)
// change nothing.
// Owner: /codemode command
#[test]
fn codemode_command_applies_and_persists_requested_mode() {
    use CodemodeMode::{On, Only};
    struct Case {
        command: &'static str,
        initially: CodemodeMode,
        calls: Vec<CodemodeMode>,
        saved: CodemodeMode,
    }
    let cases = [
        Case {
            command: "/codemode only",
            initially: On,
            calls: vec![Only],
            saved: Only,
        },
        Case {
            command: "/codemode on",
            initially: Only,
            calls: vec![On],
            saved: On,
        },
        Case {
            command: "/codemode",
            initially: Only,
            calls: vec![],
            saved: On,
        },
        Case {
            command: "/codemode only",
            initially: Only,
            calls: vec![Only],
            saved: On,
        },
        Case {
            command: "/codemode off",
            initially: On,
            calls: vec![],
            saved: On,
        },
        Case {
            command: "/codemode yolo",
            initially: On,
            calls: vec![],
            saved: On,
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
        assert_eq!(
            (runtime.calls, saved),
            (case.calls, case.saved),
            "{}",
            case.command
        );
    }
}

// Covers: a failed config save must not leave the runtime in the new mode.
// Owner: /codemode command
#[test]
fn failed_config_save_rolls_back_codemode() {
    let directory = tempdir().unwrap();
    let mut app = test_app();
    app.info.services.config_repository =
        ConfigRepository::new(Some(directory.path().to_path_buf()));
    let mut runtime = FakeRuntime::default();

    app.execute_codemode_command(invocation("/codemode only"), &mut runtime)
        .unwrap();

    assert_eq!(
        (runtime.mode, runtime.calls),
        (CodemodeMode::On, vec![CodemodeMode::Only, CodemodeMode::On])
    );
}
