use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use super::*;

const USER_FILE: &str = "/home/rho/.rho/hooks.toml";

/// Bare PATH program for user-hook fixtures that only exercise validation.
///
/// Existence is checked only for absolute paths, so bare names keep these tests
/// independent of which binaries a platform ships under `/bin`.
const PROGRAM: &str = "rho-hook-fixture";

fn parse_user(contents: &str) -> Result<Vec<HookDefinition>, HookConfigError> {
    parse_hooks_file(
        Path::new(USER_FILE),
        contents,
        HookSource::User,
        None,
        crate::tools::canonical_tool_names(),
    )
}

fn parse_project(contents: &str, root: &Path) -> Result<Vec<HookDefinition>, HookConfigError> {
    parse_hooks_file(
        &root.join(".rho/hooks.toml"),
        contents,
        HookSource::Project,
        Some(root),
        crate::tools::canonical_tool_names(),
    )
}

fn error(contents: &str) -> HookConfigError {
    parse_user(contents).expect_err("configuration should have been rejected")
}

fn toml_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\")
}

#[test]
fn a_complete_hook_parses_into_a_resolved_definition() {
    let hooks = parse_user(&format!(
        r#"
version = 1

[[hook]]
id = "deny-force-push"
on = "before_tool_use"
tools = ["bash", "powershell"]
command = ["{PROGRAM}", "-c", "exit 0"]
timeout = "2s"
env = ["MY_HOOK_TOKEN", "MY_HOOK_TOKEN"]
"#
    ))
    .unwrap();

    let hook = &hooks[0];
    assert_eq!(hook.qualified_id(), "user:deny-force-push");
    assert_eq!(
        hook.event(),
        ConfiguredHookEvent::Sdk(HookEventKind::BeforeToolUse)
    );
    assert_eq!(hook.command(), [PROGRAM, "-c", "exit 0"]);
    assert_eq!(hook.timeout(), Duration::from_secs(2));
    assert_eq!(hook.env(), ["MY_HOOK_TOKEN"], "duplicates collapse");
    assert!(hook.tools().matches("bash"));
    assert!(!hook.tools().matches("read_file"));
}

#[test]
fn an_unknown_key_is_rejected_so_a_typo_cannot_disable_a_hook() {
    let error = error(&format!(
        r#"
version = 1

[[hook]]
id = "typo"
on = "after_tool_use"
command = ["{PROGRAM}"]
timeout = "1s"
timout = "1s"
"#
    ));

    assert!(
        error.message.contains("unknown field"),
        "expected an unknown-field error, got: {}",
        error.message
    );
}

// Covers: workflow lifecycle hook names must load without adding workflow policy to the SDK.
// Owner: app hook configuration.
#[test]
fn workflow_lifecycle_events_are_app_owned_and_observational() {
    for event in WorkflowHookEventKind::ALL {
        let hooks = parse_user(&format!(
            r#"
version = 1

[[hook]]
id = "workflow"
on = "{}"
command = ["{PROGRAM}"]
timeout = "1s"
"#,
            event.wire_name()
        ))
        .unwrap();

        assert_eq!(hooks[0].event(), ConfiguredHookEvent::Workflow(*event));
        assert!(!hooks[0].event().is_blocking());
    }
}

// Covers: a valid executable under a non-UTF-8 project path must remain executable.
// Owner: hook config path resolution
#[cfg(target_os = "linux")]
#[test]
fn a_project_executable_keeps_its_native_path() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let parent = tempfile::TempDir::new().unwrap();
    let root = parent
        .path()
        .join(OsString::from_vec(b"project-\xff".to_vec()));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("handler"), "").unwrap();
    let hooks = parse_project(
        r#"
version = 1

[[hook]]
id = "native-path"
on = "run_completed"
command = ["./handler"]
timeout = "1s"
"#,
        &root,
    )
    .unwrap();

    assert_eq!(hooks[0].executable(), root.join("handler"));
}

#[test]
fn a_project_hook_may_not_name_a_bare_path_program() {
    let root = tempfile::TempDir::new().unwrap();
    let error = parse_project(
        r#"
version = 1

[[hook]]
id = "path-name"
on = "after_tool_use"
command = ["my-hook"]
timeout = "1s"
"#,
        root.path(),
    )
    .expect_err("bare PATH names must be rejected for project hooks");

    assert_eq!(error.field.as_deref(), Some("command"));
    assert!(error.message.contains("by path"));
}

#[test]
fn a_relative_project_command_resolves_against_the_project_root() {
    let root = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(root.path().join(".rho/hooks")).unwrap();
    let program = root.path().join(".rho/hooks/fmt");
    std::fs::write(&program, "#!/bin/sh\n").unwrap();

    let hooks = parse_project(
        r#"
version = 1

[[hook]]
id = "fmt"
on = "after_tool_use"
command = ["./.rho/hooks/fmt", "--check"]
timeout = "5s"
"#,
        root.path(),
    )
    .unwrap();

    assert_eq!(
        hooks[0].command(),
        [crate::paths::display(&program), "--check".into()]
    );
    assert_eq!(hooks[0].working_directory(), root.path());
    assert_eq!(hooks[0].qualified_id(), "project:fmt");
}

// Covers: each invalid hook field is rejected at load and attributed to the
// hook and field that caused it, so a bad entry cannot silently load.
// Owner: hook config validation.
#[test]
fn an_invalid_hook_is_rejected_at_its_field() {
    let hook = |id: &str, on: &str, extra: &str| {
        format!(
            "version = 1\n\n[[hook]]\nid = \"{id}\"\non = \"{on}\"\ncommand = [\"{PROGRAM}\"]\ntimeout = \"1s\"\n{extra}"
        )
    };
    let with = |id: &str, on: &str, command: &str, timeout: &str, extra: &str| {
        format!(
            "version = 1\n\n[[hook]]\nid = \"{id}\"\non = \"{on}\"\ncommand = {command}\ntimeout = \"{timeout}\"\n{extra}"
        )
    };
    let program = format!("[\"{PROGRAM}\"]");
    // temp_dir is absolute on every platform, including Windows where a
    // leading `/` is not an absolute path and would skip the exists check.
    let missing: PathBuf = std::env::temp_dir().join("rho-missing-hook-program");
    let _ = std::fs::remove_file(&missing);
    let missing_command = format!("[\"{}\"]", toml_string(&missing));
    let cases = [
        ("unknown schema version", "version = 2".to_owned(), None, "version"),
        (
            "unknown event",
            hook("bad-event", "before_tool", ""),
            Some("bad-event"),
            "on",
        ),
        (
            "non-configurable event",
            hook("inject", "user_prompt_accepted", ""),
            Some("inject"),
            "on",
        ),
        (
            "duplicate id after trimming",
            format!(
                "{}\n[[hook]]\nid = \" same \"\non = \"run_completed\"\ncommand = {program}\ntimeout = \"1s\"\n",
                hook("same", "after_tool_use", "")
            ),
            Some("same"),
            "id",
        ),
        ("unsafe id", hook("bad id!", "after_tool_use", ""), None, "id"),
        (
            "empty argv",
            with("empty", "after_tool_use", "[]", "1s", ""),
            Some("empty"),
            "command",
        ),
        (
            "missing absolute program",
            with("missing", "after_tool_use", &missing_command, "1s", ""),
            Some("missing"),
            "command",
        ),
        (
            "zero timeout",
            with("instant", "after_tool_use", &program, "0s", ""),
            Some("instant"),
            "timeout",
        ),
        (
            "unbounded timeout",
            with("forever", "after_tool_use", &program, "2h", ""),
            Some("forever"),
            "timeout",
        ),
        (
            "unparsable timeout",
            with("vague", "after_tool_use", &program, "soon", ""),
            Some("vague"),
            "timeout",
        ),
        (
            "env entry with a value",
            hook("leaky", "after_tool_use", "env = [\"TOKEN=secret\"]\n"),
            Some("leaky"),
            "env",
        ),
        (
            "tool list on an event with no tool",
            hook("confused", "run_completed", "tools = [\"bash\"]\n"),
            Some("confused"),
            "tools",
        ),
        (
            "tool outside the canonical list",
            hook("unknown-tool", "before_tool_use", "tools = [\"shell\"]\n"),
            Some("unknown-tool"),
            "tools",
        ),
    ];
    for (case, contents, hook_id, field) in cases {
        let error = error(&contents);

        assert_eq!(error.field.as_deref(), Some(field), "{case}: {error}");
        if let Some(hook_id) = hook_id {
            assert_eq!(error.hook_id.as_deref(), Some(hook_id), "{case}");
        }
    }
}
