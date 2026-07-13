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
fn matching_paths_respect_gitignore_and_fuzzy_rank() {
    clear_workspace_file_path_cache();
    let workspace = tempdir().unwrap();
    fs::create_dir(workspace.path().join(".git")).unwrap();
    fs::create_dir_all(workspace.path().join("src/nested")).unwrap();
    fs::create_dir_all(workspace.path().join("target")).unwrap();
    fs::write(workspace.path().join("src/lib.rs"), "").unwrap();
    fs::write(workspace.path().join("src/nested/mod.rs"), "").unwrap();
    fs::write(workspace.path().join("target/generated.rs"), "").unwrap();
    fs::write(workspace.path().join(".gitignore"), "target/\n").unwrap();

    let matches = matching_file_paths(workspace.path(), "slr");
    assert_eq!(matches.as_slice(), ["src/lib.rs"]);

    let paths = workspace_file_paths(workspace.path());
    assert_eq!(
        paths.as_slice(),
        [".gitignore", "src/lib.rs", "src/nested/mod.rs"]
    );
}

#[test]
fn fuzzy_matching_prefers_path_component_boundaries() {
    let paths = vec![
        "src/tui/model_picker.rs".to_string(),
        "AGENTS.md".to_string(),
    ];
    let matches = fuzzy_matching_paths(&paths, "tmd");
    assert_eq!(
        matches,
        vec![
            "src/tui/model_picker.rs".to_string(),
            "AGENTS.md".to_string()
        ]
    );
}

#[test]
fn empty_query_returns_workspace_paths() {
    clear_workspace_file_path_cache();
    let workspace = tempdir().unwrap();
    fs::write(workspace.path().join("a.rs"), "").unwrap();
    fs::write(workspace.path().join("b.rs"), "").unwrap();

    let paths = workspace_file_paths(workspace.path());
    let matches = matching_file_paths(workspace.path(), "");
    assert_eq!(matches.as_slice(), paths.as_slice());
    assert_eq!(matches.as_slice(), ["a.rs", "b.rs"]);
}

#[test]
fn ranked_matches_are_capped_for_weak_queries() {
    let paths = (0..(MAX_RANKED_FILE_MATCHES + 50))
        .map(|index| format!("file-{index:04}.rs"))
        .collect::<Vec<_>>();
    let matches = fuzzy_matching_paths(&paths, "file");
    assert_eq!(matches.len(), MAX_RANKED_FILE_MATCHES);
    assert!(matches[0].starts_with("file-"));
}

#[test]
fn scroll_counts_track_hidden_rows_above_and_below() {
    assert_eq!(file_palette_scroll_counts(12, 0, 5), (0, 0, 7));
    assert_eq!(file_palette_scroll_counts(12, 4, 5), (0, 0, 7));
    assert_eq!(file_palette_scroll_counts(12, 5, 5), (1, 1, 6));
    assert_eq!(file_palette_scroll_counts(12, 11, 5), (7, 7, 0));
}

#[test]
fn scroll_footer_only_when_overflow_exists() {
    assert_eq!(file_palette_scroll_footer(0, 0, 5), None);
    assert_eq!(
        file_palette_scroll_footer(2, 0, 7),
        Some("↑ 2 more · 7 total".into())
    );
    assert_eq!(
        file_palette_scroll_footer(0, 4, 9),
        Some("↓ 4 more · 9 total".into())
    );
    assert_eq!(
        file_palette_scroll_footer(3, 8, 16),
        Some("↑ 3 more · ↓ 8 more · 16 total".into())
    );
}

#[test]
fn relative_directory_prefix_scopes_search_to_that_directory() {
    clear_workspace_file_path_cache();
    let workspace = tempdir().unwrap();
    fs::create_dir_all(workspace.path().join("src/nested")).unwrap();
    fs::write(workspace.path().join("README.md"), "").unwrap();
    fs::write(workspace.path().join("src/lib.rs"), "").unwrap();
    fs::write(workspace.path().join("src/main.rs"), "").unwrap();
    fs::write(workspace.path().join("src/nested/mod.rs"), "").unwrap();

    let scoped = matching_file_paths(workspace.path(), "src/");
    assert_eq!(
        scoped.as_slice(),
        ["src/lib.rs", "src/main.rs", "src/nested/mod.rs"]
    );

    let residual = matching_file_paths(workspace.path(), "src/lib");
    assert_eq!(residual.as_slice(), ["src/lib.rs"]);
}

#[test]
fn parent_directory_prefix_scopes_outside_cwd() {
    clear_workspace_file_path_cache();
    let root = tempdir().unwrap();
    let workspace = root.path().join("project");
    let sibling = root.path().join("sibling");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&sibling).unwrap();
    fs::write(workspace.join("local.rs"), "").unwrap();
    fs::write(sibling.join("outside.rs"), "").unwrap();
    fs::write(sibling.join("notes.md"), "").unwrap();

    let matches = matching_file_paths(&workspace, "../sibling/");
    assert_eq!(
        matches.as_slice(),
        ["../sibling/notes.md", "../sibling/outside.rs"]
    );

    let filtered = matching_file_paths(&workspace, "../sibling/out");
    assert_eq!(filtered.as_slice(), ["../sibling/outside.rs"]);
}

#[test]
fn home_directory_prefix_scopes_to_home_relative_path() {
    clear_workspace_file_path_cache();
    let home = tempdir().unwrap();
    let nested = home.path().join("docs");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("guide.md"), "").unwrap();
    fs::write(nested.join("todo.txt"), "").unwrap();

    let matches =
        matching_file_paths_with_home_for_test(Path::new("/tmp"), "~/docs/", Some(home.path()));
    assert_eq!(matches.as_slice(), ["~/docs/guide.md", "~/docs/todo.txt"]);
}

#[test]
fn non_existing_directory_prefix_falls_back_to_workspace_fuzzy() {
    clear_workspace_file_path_cache();
    let workspace = tempdir().unwrap();
    fs::create_dir_all(workspace.path().join("src")).unwrap();
    fs::write(workspace.path().join("src/lib.rs"), "").unwrap();
    fs::write(workspace.path().join("README.md"), "").unwrap();

    let paths = workspace_file_paths(workspace.path());
    let query = "no_such_dir/lib";
    let matches = matching_file_paths(workspace.path(), query);
    assert_eq!(
        matches.as_slice(),
        fuzzy_matching_paths(paths.as_slice(), query).as_slice()
    );
}
