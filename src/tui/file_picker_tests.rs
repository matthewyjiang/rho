use std::fs;

use pretty_assertions::assert_eq;
use tempfile::tempdir;

use super::*;

#[test]
fn finds_mention_at_cursor() {
    assert_eq!(
        active_file_mention("review @src/tu please", 14),
        Some(FileMention {
            start: 7,
            end: 14,
            query: "src/tu".into(),
        })
    );
}

#[test]
fn mention_starts_after_newline() {
    assert_eq!(
        active_file_mention("review\n@src", 11),
        Some(FileMention {
            start: 7,
            end: 11,
            query: "src".into(),
        })
    );
}

#[test]
fn text_after_mention_is_not_part_of_query() {
    assert_eq!(active_file_mention("review @src later", 17), None);
}

#[test]
fn email_like_tokens_do_not_open_file_mentions() {
    assert_eq!(active_file_mention("email a@b", 9), None);
}

#[test]
fn picker_lists_workspace_files_and_respects_gitignore() {
    let workspace = tempdir().unwrap();
    fs::create_dir(workspace.path().join(".git")).unwrap();
    fs::create_dir_all(workspace.path().join("src/nested")).unwrap();
    fs::create_dir_all(workspace.path().join("target")).unwrap();
    fs::write(workspace.path().join("src/lib.rs"), "").unwrap();
    fs::write(workspace.path().join("src/nested/mod.rs"), "").unwrap();
    fs::write(workspace.path().join("target/generated.rs"), "").unwrap();
    fs::write(workspace.path().join(".gitignore"), "target/\n").unwrap();

    let picker = file_path_picker(workspace.path(), "slr");
    let values = picker
        .items
        .iter()
        .map(|item| item.value.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        values,
        vec![".gitignore", "src/lib.rs", "src/nested/mod.rs"]
    );
    assert_eq!(picker.selected_item().unwrap().value, "src/lib.rs");
}
