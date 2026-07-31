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
        crate::tools::CANONICAL_TOOL_NAMES,
    )
}

fn parse_project(contents: &str, root: &Path) -> Result<Vec<HookDefinition>, HookConfigError> {
    parse_hooks_file(
        &root.join(".rho/hooks.toml"),
        contents,
        HookSource::Project,
        Some(root),
        crate::tools::CANONICAL_TOOL_NAMES,
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
    assert_eq!(hook.event(), HookEventKind::BeforeToolUse);
    assert_eq!(hook.command(), [PROGRAM, "-c", "exit 0"]);
    assert_eq!(hook.timeout(), Duration::from_secs(2));
    assert_eq!(hook.env(), ["MY_HOOK_TOKEN"], "duplicates collapse");
    assert!(hook.tools().matches("bash"));
    assert!(!hook.tools().matches("read_file"));
}

#[test]
fn an_omitted_tool_list_means_every_tool() {
    let hooks = parse_user(&format!(
        r#"
version = 1

[[hook]]
id = "log-all"
on = "after_tool_use"
command = ["{PROGRAM}"]
timeout = "1s"
"#
    ))
    .unwrap();

    assert!(hooks[0].tools().matches("read_file"));
    assert_eq!(hooks[0].tools().describe(), "*");
}

#[test]
fn a_file_with_no_hooks_is_valid() {
    assert_eq!(parse_user("version = 1").unwrap(), Vec::new());
}

#[test]
fn an_unknown_schema_version_is_rejected() {
    let error = error("version = 2");

    assert_eq!(error.field.as_deref(), Some("version"));
    assert!(error.message.contains("unsupported hooks schema version 2"));
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

#[test]
fn an_unknown_event_names_the_hook_and_the_field() {
    let error = error(&format!(
        r#"
version = 1

[[hook]]
id = "bad-event"
on = "before_tool"
command = ["{PROGRAM}"]
timeout = "1s"
"#
    ));

    assert_eq!(error.hook_id.as_deref(), Some("bad-event"));
    assert_eq!(error.field.as_deref(), Some("on"));
    assert!(error.message.contains("unknown event 'before_tool'"));
    assert!(error.message.contains("before_tool_use"));
}

#[test]
fn an_unknown_event_name_is_rejected() {
    let error = error(&format!(
        r#"
version = 1

[[hook]]
id = "inject"
on = "user_prompt_accepted"
command = ["{PROGRAM}"]
timeout = "1s"
"#
    ));

    assert_eq!(error.field.as_deref(), Some("on"));
    assert!(error
        .message
        .contains("unknown event 'user_prompt_accepted'"));
}

#[test]
fn a_duplicate_id_within_one_file_is_an_error() {
    let error = error(&format!(
        r#"
version = 1

[[hook]]
id = "same"
on = "after_tool_use"
command = ["{PROGRAM}"]
timeout = "1s"

[[hook]]
id = " same "
on = "run_completed"
command = ["{PROGRAM}"]
timeout = "1s"
"#
    ));

    assert_eq!(error.hook_id.as_deref(), Some("same"));
    assert_eq!(error.field.as_deref(), Some("id"));
    assert!(error.message.contains("duplicate hook ID"));
}

// Covers: a valid executable under a non-UTF-8 project path must remain executable.
// Owner: hook config path resolution
#[cfg(unix)]
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
fn an_id_with_unsafe_characters_is_rejected() {
    let error = error(&format!(
        r#"
version = 1

[[hook]]
id = "bad id!"
on = "after_tool_use"
command = ["{PROGRAM}"]
timeout = "1s"
"#
    ));

    assert_eq!(error.field.as_deref(), Some("id"));
}

#[test]
fn an_empty_argv_is_rejected() {
    let error = error(
        r#"
version = 1

[[hook]]
id = "empty"
on = "after_tool_use"
command = []
timeout = "1s"
"#,
    );

    assert_eq!(error.field.as_deref(), Some("command"));
    assert!(error.message.contains("empty"));
}

#[test]
fn a_zero_timeout_is_rejected() {
    let error = error(&format!(
        r#"
version = 1

[[hook]]
id = "instant"
on = "after_tool_use"
command = ["{PROGRAM}"]
timeout = "0s"
"#
    ));

    assert_eq!(error.field.as_deref(), Some("timeout"));
    assert!(error.message.contains("greater than zero"));
}

#[test]
fn an_unbounded_timeout_is_rejected() {
    let error = error(&format!(
        r#"
version = 1

[[hook]]
id = "forever"
on = "after_tool_use"
command = ["{PROGRAM}"]
timeout = "2h"
"#
    ));

    assert_eq!(error.field.as_deref(), Some("timeout"));
    assert!(error.message.contains("maximum"));
}

#[test]
fn an_unparsable_timeout_names_the_field() {
    let error = error(&format!(
        r#"
version = 1

[[hook]]
id = "vague"
on = "after_tool_use"
command = ["{PROGRAM}"]
timeout = "soon"
"#
    ));

    assert_eq!(error.field.as_deref(), Some("timeout"));
    assert!(error.message.contains("invalid duration"));
}

#[test]
fn an_env_entry_with_a_value_is_rejected() {
    let error = error(&format!(
        r#"
version = 1

[[hook]]
id = "leaky"
on = "after_tool_use"
command = ["{PROGRAM}"]
timeout = "1s"
env = ["TOKEN=secret"]
"#
    ));

    assert_eq!(error.field.as_deref(), Some("env"));
    assert!(error.message.contains("list only variable names"));
}

#[test]
fn a_tool_list_on_an_event_with_no_tool_is_rejected() {
    let error = error(&format!(
        r#"
version = 1

[[hook]]
id = "confused"
on = "run_completed"
tools = ["bash"]
command = ["{PROGRAM}"]
timeout = "1s"
"#
    ));

    assert_eq!(error.field.as_deref(), Some("tools"));
    assert!(error.message.contains("no tool to match"));
}

#[test]
fn a_tool_outside_the_canonical_list_names_the_list() {
    let error = error(&format!(
        r#"
version = 1

[[hook]]
id = "unknown-tool"
on = "before_tool_use"
tools = ["shell"]
command = ["{PROGRAM}"]
timeout = "1s"
"#
    ));

    assert_eq!(error.field.as_deref(), Some("tools"));
    assert!(error.message.contains("canonical tool list"));
    assert!(error.message.contains("bash"));
}

#[test]
fn a_user_hook_may_name_a_program_on_path() {
    let hooks = parse_user(
        r#"
version = 1

[[hook]]
id = "path-name"
on = "after_tool_use"
command = ["my-hook"]
timeout = "1s"
"#,
    )
    .unwrap();

    assert_eq!(hooks[0].command(), ["my-hook"]);
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

#[test]
fn an_absolute_program_that_does_not_exist_fails_at_load() {
    // temp_dir is absolute on every platform, including Windows where a
    // leading `/` is not an absolute path and would skip the exists check.
    let missing: PathBuf = std::env::temp_dir().join("rho-missing-hook-program");
    let _ = std::fs::remove_file(&missing);
    assert!(
        !missing.exists(),
        "fixture path must stay absent: {}",
        missing.display()
    );

    let error = error(&format!(
        r#"
version = 1

[[hook]]
id = "missing"
on = "after_tool_use"
command = ["{}"]
timeout = "1s"
"#,
        toml_string(&missing)
    ));

    assert_eq!(error.field.as_deref(), Some("command"));
    assert!(error.message.contains("does not exist"));
}

#[test]
fn an_error_renders_the_file_the_hook_and_the_field() {
    let error = error(&format!(
        r#"
version = 1

[[hook]]
id = "vague"
on = "after_tool_use"
command = ["{PROGRAM}"]
timeout = "soon"
"#
    ));

    assert_eq!(
        error.to_string(),
        format!(
            "{USER_FILE}: hook 'vague': field 'timeout': invalid duration: expected number at 0"
        )
    );
}
