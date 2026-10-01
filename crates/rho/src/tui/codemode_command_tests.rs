use pretty_assertions::assert_eq;
use tempfile::tempdir;

use super::*;
use crate::{
    app::config_repository::ConfigRepository, commands::parse_command, tui::tests::test_app,
};

#[derive(Default)]
struct FakeRuntime {
    enabled: bool,
    /// Each `set_codemode` argument, in order.
    calls: Vec<bool>,
}

impl CodemodeRuntime for FakeRuntime {
    fn codemode_enabled(&self) -> bool {
        self.enabled
    }

    fn set_codemode(
        &mut self,
        enabled: bool,
    ) -> impl Future<Output = anyhow::Result<Option<String>>> + Send {
        self.calls.push(enabled);
        let changed = self.enabled != enabled;
        self.enabled = enabled;
        std::future::ready(Ok(changed.then(|| format!("codemode {enabled}"))))
    }

    fn tool_specs(&self) -> Vec<rho_sdk::model::ToolSpec> {
        Vec::new()
    }
}

fn invocation(command: &str) -> CommandInvocation {
    parse_command(command).unwrap().unwrap()
}

// Covers: /codemode toggles and on/off reach the runtime and persist; a bad
// argument (including the removed `yolo`) changes nothing.
// Owner: /codemode command
#[tokio::test]
async fn codemode_command_applies_and_persists_requested_state() {
    struct Case {
        command: &'static str,
        initially: bool,
        calls: Vec<bool>,
        saved: bool,
    }
    let cases = [
        Case {
            command: "/codemode off",
            initially: true,
            calls: vec![false],
            saved: false,
        },
        Case {
            command: "/codemode on",
            initially: false,
            calls: vec![true],
            saved: true,
        },
        Case {
            command: "/codemode",
            initially: true,
            calls: vec![false],
            saved: false,
        },
        Case {
            command: "/codemode on",
            initially: true,
            calls: vec![],
            saved: true,
        },
        Case {
            command: "/codemode yolo",
            initially: false,
            calls: vec![],
            saved: true,
        },
    ];
    for case in cases {
        let mut app = test_app();
        let mut runtime = FakeRuntime {
            enabled: case.initially,
            calls: Vec::new(),
        };

        app.execute_codemode_command_with_runtime(invocation(case.command), &mut runtime)
            .await
            .unwrap();

        let saved = app.info.services.config_repository.load().unwrap().codemode;
        assert_eq!(
            (runtime.calls, saved),
            (case.calls, case.saved),
            "{}",
            case.command
        );
    }
}

// Covers: a failed config save must not leave the runtime in the new state.
// Owner: /codemode command
#[tokio::test]
async fn failed_config_save_rolls_back_codemode() {
    let directory = tempdir().unwrap();
    let mut app = test_app();
    app.info.services.config_repository =
        ConfigRepository::new(Some(directory.path().to_path_buf()));
    let mut runtime = FakeRuntime {
        enabled: true,
        calls: Vec::new(),
    };

    app.execute_codemode_command_with_runtime(invocation("/codemode off"), &mut runtime)
        .await
        .unwrap();

    assert_eq!((runtime.enabled, runtime.calls), (true, vec![false, true]));
}
