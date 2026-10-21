use std::ffi::OsStr;

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
fn editor_command_rejects_whitespace_only_values() {
    let error = editor_command(OsStr::new("  ")).unwrap_err();

    assert_eq!(error.to_string(), "editor command is empty");
}

// Covers: VISUAL wins over EDITOR, an empty VISUAL falls back to EDITOR, and
// empty or missing values mean no editor is configured.
// Owner: external editor resolution
#[test]
fn resolve_editor_prefers_visual_then_editor() {
    let os = |value: &str| Some(OsString::from(value));
    for (visual, editor, expected) in [
        (os("visual"), os("fallback"), os("visual")),
        (os(""), os("fallback"), os("fallback")),
        (None, None, None),
        (os(""), None, None),
        (None, os(""), None),
    ] {
        assert_eq!(
            resolve_editor(visual.clone(), editor.clone()),
            expected,
            "{visual:?} {editor:?}"
        );
    }
}
