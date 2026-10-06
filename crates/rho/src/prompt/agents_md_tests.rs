use pretty_assertions::assert_eq;

use super::{append_instruction, appended_contents, instruction_lock_path};
use crate::config_writer::{self, edit_lock::acquire_lock_file};

// Covers: competing sessions cannot replace instructions, including first-file creation.
// Owner: instruction-file transaction; independent handles exercise the OS advisory lock.
#[test]
fn append_rejects_a_contender_and_rereads_after_unlock() {
    let cases = [
        (
            None,
            "- instruction from other session\n- contending instruction\n",
        ),
        (
            Some("- original instruction\n"),
            "- original instruction\n- instruction from other session\n- contending instruction\n",
        ),
    ];
    for (initial, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/AGENTS.md");
        if let Some(initial) = initial {
            config_writer::write_atomically(&path, initial).unwrap();
        }
        let locks = dir.path().join("locks");
        // Key the lock the way `append_instruction` does: after the parent
        // exists, so canonicalization (macOS /private/var, Windows \\?\)
        // yields the same key.
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let held = acquire_lock_file(&instruction_lock_path(&path, &locks)).unwrap();
        append_instruction(&path, &locks, "contending instruction").unwrap_err();
        assert_eq!(std::fs::read_to_string(&path).ok().as_deref(), initial);

        // The holder saves before the contender retries. A first-file creator
        // and an existing-file writer must both retain this committed content.
        let saved = appended_contents(initial, "instruction from other session");
        config_writer::write_atomically(&path, &saved).unwrap();
        drop(held);
        append_instruction(&path, &locks, "contending instruction").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
        // No lock debris lands next to the instructions (it would show in git status).
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1,
            "only AGENTS.md beside the destination"
        );
    }
}

// Covers: atomic rename must not bypass a read-only target or replace a symlink.
// Owner: Unix instruction-file destination protection.
#[cfg(unix)]
#[test]
fn append_preserves_protected_destinations() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    for symlink_destination in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.md");
        let path = dir.path().join("AGENTS.md");
        let original = "- protected instruction\n";
        std::fs::write(&target, original).unwrap();
        if symlink_destination {
            symlink(&target, &path).unwrap();
        } else {
            std::fs::write(&path, original).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        }
        let before = std::fs::symlink_metadata(&path).unwrap();
        assert!(append_instruction(&path, &dir.path().join("locks"), "must not be saved").is_err());
        let after = std::fs::symlink_metadata(&path).unwrap();
        assert_eq!(
            (after.file_type(), after.permissions()),
            (before.file_type(), before.permissions())
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), original);
    }
}

// Covers: appending must not merge bullets, erase instructions, or accumulate blank lines.
// Owner: AGENTS.md Markdown assembly.
#[test]
fn appends_one_trimmed_bullet() {
    let cases = [
        (None, "use max jobs 8", "- use max jobs 8\n"),
        (Some(""), "use max jobs 8", "- use max jobs 8\n"),
        (
            Some("# Instructions\n"),
            "be concise",
            "# Instructions\n- be concise\n",
        ),
        (
            Some("- preserve tests"),
            "be concise",
            "- preserve tests\n- be concise\n",
        ),
        (
            Some("- preserve tests\n\n\n"),
            "be concise",
            "- preserve tests\n- be concise\n",
        ),
        (
            Some("- preserve tests\r\n"),
            "  be concise \t",
            "- preserve tests\r\n- be concise\r\n",
        ),
        (
            Some("# Instructions\r\n- preserve tests\r\n\r\n"),
            "be concise",
            "# Instructions\r\n- preserve tests\r\n- be concise\r\n",
        ),
        (Some("\r\n"), "be concise", "- be concise\r\n"),
        (None, "  be concise \t", "- be concise\n"),
    ];
    for (existing, text, expected) in cases {
        assert_eq!(
            appended_contents(existing, text),
            expected,
            "existing: {existing:?}, text: {text:?}"
        );
    }
}

// Covers: a first `/remember` must not create an owner-only AGENTS.md; it is
// shared project text, so it gets the same mode as any file the user creates.
// Owner: Unix instruction-file creation.
#[cfg(unix)]
#[test]
fn new_instruction_file_uses_process_umask() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("AGENTS.md");
    let reference = dir.path().join("reference.md");
    std::fs::write(&reference, "").unwrap();
    append_instruction(&path, &dir.path().join("locks"), "shared rule").unwrap();
    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), mode(&reference));
}
