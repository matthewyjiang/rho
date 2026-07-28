use std::{cell::Cell, rc::Rc};

use pretty_assertions::assert_eq;
use tempfile::tempdir;

use super::*;
use crate::{
    app::config_repository::ConfigRepository, commands::parse_command, tui::tests::test_app,
};

#[derive(Clone, Default)]
struct FakeRuntime {
    enabled: Rc<Cell<bool>>,
    set_calls: Rc<Cell<usize>>,
}

impl FastModeRuntime for FakeRuntime {
    fn fast_mode(&self) -> bool {
        self.enabled.get()
    }

    fn set_fast_mode(&self, enabled: bool) -> anyhow::Result<()> {
        self.enabled.set(enabled);
        self.set_calls.set(self.set_calls.get() + 1);
        Ok(())
    }
}

fn invocation(command: &str) -> CommandInvocation {
    parse_command(command).unwrap().unwrap()
}

fn supported_app() -> App {
    let mut app = test_app();
    app.info.runtime.provider = "openai-codex".into();
    app.info.runtime.model = "gpt-5.5".into();
    app
}

#[test]
fn enabling_fast_mode_updates_the_runtime_and_config() {
    let mut app = supported_app();
    let runtime = FakeRuntime::default();

    app.execute_fast_command_with_runtime(invocation("/fast on"), &runtime)
        .unwrap();

    assert!(runtime.fast_mode());
    assert_eq!(runtime.set_calls.get(), 1);
    assert!(
        app.info
            .services
            .config_repository
            .load()
            .unwrap()
            .fast_mode
    );
    assert_eq!(app.status, "fast mode on");
}

#[test]
fn unsupported_models_reject_fast_mode_without_mutation() {
    let mut app = supported_app();
    app.info.runtime.model = "gpt-5.3-codex-spark".into();
    let runtime = FakeRuntime::default();

    app.execute_fast_command_with_runtime(invocation("/fast on"), &runtime)
        .unwrap();

    assert!(!runtime.fast_mode());
    assert_eq!(runtime.set_calls.get(), 0);
    assert!(
        !app.info
            .services
            .config_repository
            .load()
            .unwrap()
            .fast_mode
    );
    assert_eq!(app.status, "fast mode unavailable");
}

#[test]
fn failed_config_save_rolls_back_the_runtime() {
    let directory = tempdir().unwrap();
    let mut app = supported_app();
    app.info.services.config_repository =
        ConfigRepository::new(Some(directory.path().to_path_buf()));
    let runtime = FakeRuntime::default();

    app.execute_fast_command_with_runtime(invocation("/fast on"), &runtime)
        .unwrap();

    assert!(!runtime.fast_mode());
    assert_eq!(runtime.set_calls.get(), 2);
    assert_eq!(app.status, "config save failed");
}

#[test]
fn status_does_not_mutate_runtime_or_config() {
    let mut app = supported_app();
    let runtime = FakeRuntime::default();
    runtime.enabled.set(true);

    app.execute_fast_command_with_runtime(invocation("/fast status"), &runtime)
        .unwrap();

    assert!(runtime.fast_mode());
    assert_eq!(runtime.set_calls.get(), 0);
    assert!(
        !app.info
            .services
            .config_repository
            .load()
            .unwrap()
            .fast_mode
    );
    assert_eq!(app.status, "fast mode on");
}
