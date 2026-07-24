use std::{ffi::OsStr, path::PathBuf};

use super::*;

#[cfg(unix)]
#[test]
fn unix_editor_command_supports_arguments_and_quoted_paths() {
    let parts = editor_parts(OsStr::new("'/opt/My Editor/bin/editor' --wait")).unwrap();

    assert_eq!(
        parts,
        [
            OsString::from("/opt/My Editor/bin/editor"),
            OsString::from("--wait")
        ]
    );
}

#[test]
fn existing_editor_path_is_not_split() {
    let executable = tempfile::NamedTempFile::new().unwrap();
    let path = executable.path().as_os_str();

    assert_eq!(editor_parts(path).unwrap(), [path]);
}

#[cfg(windows)]
#[test]
fn windows_editor_command_preserves_paths_flags_and_empty_arguments() {
    let parts = editor_parts(OsStr::new(
        r#""C:\Program Files\Editor\editor.exe" --wait """#,
    ))
    .unwrap();

    assert_eq!(
        parts,
        [
            OsString::from(r"C:\Program Files\Editor\editor.exe"),
            OsString::from("--wait"),
            OsString::new(),
        ]
    );
}

#[cfg(windows)]
#[test]
fn windows_editor_command_preserves_unquoted_backslashes() {
    let parts = editor_parts(OsStr::new(r"C:\tools\vim.exe --nofork")).unwrap();

    assert_eq!(
        parts,
        [
            OsString::from(r"C:\tools\vim.exe"),
            OsString::from("--nofork"),
        ]
    );
}

#[test]
fn preserves_a_draft_in_a_durable_recovery_file() {
    let path = preserve_draft_for_recovery("recovery contents").unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "recovery contents");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn removes_only_the_editors_final_line_ending() {
    assert_eq!(remove_editor_final_line_ending("draft\n".into()), "draft");
    assert_eq!(remove_editor_final_line_ending("draft\r\n".into()), "draft");
    assert_eq!(
        remove_editor_final_line_ending("draft\n\n".into()),
        "draft\n"
    );
    assert_eq!(remove_editor_final_line_ending("draft".into()), "draft");
}

#[test]
fn editor_command_rejects_whitespace_only_values() {
    let error = editor_command(OsStr::new("  ")).unwrap_err();

    assert_eq!(error.to_string(), "editor command is empty");
}

#[test]
fn resolve_editor_prefers_visual_over_editor() {
    let editor = resolve_editor(
        Some(OsString::from("visual-editor")),
        Some(OsString::from("fallback-editor")),
    );

    assert_eq!(editor, Some(OsString::from("visual-editor")));
}

#[test]
fn resolve_editor_uses_editor_when_visual_is_unset_or_empty() {
    assert_eq!(
        resolve_editor(None, Some(OsString::from("only-editor"))),
        Some(OsString::from("only-editor"))
    );
    assert_eq!(
        resolve_editor(Some(OsString::new()), Some(OsString::from("only-editor"))),
        Some(OsString::from("only-editor"))
    );
}

#[test]
fn resolve_editor_requires_a_configured_editor() {
    assert_eq!(resolve_editor(None, None), None);
    assert_eq!(resolve_editor(Some(OsString::new()), None), None);
    assert_eq!(resolve_editor(None, Some(OsString::new())), None);
}

#[test]
fn editor_parts_preserves_nonexistent_direct_path_shape() {
    let parts = editor_parts(OsStr::new("/missing/editor --wait")).unwrap();

    assert_eq!(
        parts,
        [OsString::from("/missing/editor"), OsString::from("--wait")]
    );
    assert_eq!(PathBuf::from(&parts[0]), PathBuf::from("/missing/editor"));
}
