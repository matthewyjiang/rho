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
fn mention_at_mid_token_replaces_through_token_end() {
    assert_eq!(
        active_file_mention("review @src/lib.rs later", 11),
        Some(FileMention {
            start: 7,
            end: 18,
            query: "src".into(),
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
fn mention_offsets_survive_multibyte_characters_before_the_token() {
    // "héllo @w" — é is two bytes, so byte offsets and char offsets diverge.
    assert_eq!(
        active_file_mention("héllo @w", 8),
        Some(FileMention {
            start: 6,
            end: 8,
            query: "w".into(),
        })
    );
}

#[test]
fn cursor_past_the_input_still_finds_the_trailing_token() {
    assert_eq!(
        active_file_mention("review @src", 99),
        Some(FileMention {
            start: 7,
            end: 11,
            query: "src".into(),
        })
    );
}

#[test]
fn matching_paths_respect_gitignore_and_fuzzy_rank() {
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
    assert_eq!(paths.as_slice(), ["src/lib.rs", "src/nested/mod.rs"]);
}

#[test]
fn hidden_paths_are_skipped_unless_query_mentions_dot() {
    let workspace = tempdir().unwrap();
    fs::create_dir_all(workspace.path().join(".cache/nested")).unwrap();
    fs::create_dir_all(workspace.path().join("docs")).unwrap();
    fs::write(workspace.path().join(".gitignore"), "").unwrap();
    fs::write(workspace.path().join(".cache/secret.bin"), "").unwrap();
    fs::write(workspace.path().join(".cache/nested/tmp.bin"), "").unwrap();
    fs::write(workspace.path().join("docs/guide.md"), "").unwrap();
    fs::write(workspace.path().join("README.md"), "").unwrap();

    let default_matches = matching_file_paths(workspace.path(), "");
    assert_eq!(default_matches.as_slice(), ["docs/guide.md", "README.md"]);

    let hidden_matches = matching_file_paths(workspace.path(), ".giti");
    assert_eq!(hidden_matches.as_slice(), [".gitignore"]);

    let scoped_default = matching_file_paths(workspace.path(), ".cache/");
    assert_eq!(
        sorted_strings(scoped_default.as_slice()),
        sorted_strs(&[".cache/nested/tmp.bin", ".cache/secret.bin"])
    );

    let scoped_hidden = matching_file_paths(workspace.path(), ".cache/.");
    assert_eq!(
        sorted_strings(scoped_hidden.as_slice()),
        sorted_strs(&[".cache/nested/tmp.bin", ".cache/secret.bin"])
    );
}

fn sorted_strs(values: &[&str]) -> Vec<String> {
    let mut values = values
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    values.sort();
    values
}

fn sorted_strings(values: &[String]) -> Vec<String> {
    let mut values = values.to_vec();
    values.sort();
    values
}

#[test]
fn home_scope_skips_hidden_entries_by_default() {
    let home = tempdir().unwrap();
    fs::create_dir_all(home.path().join(".cache")).unwrap();
    fs::create_dir_all(home.path().join("docs")).unwrap();
    fs::write(home.path().join(".cache/huge.bin"), "").unwrap();
    fs::write(home.path().join("docs/guide.md"), "").unwrap();
    fs::write(home.path().join("notes.txt"), "").unwrap();

    let matches =
        matching_file_paths_with_home_for_test(Path::new("/tmp"), "~/", Some(home.path()));
    assert_eq!(matches.as_slice(), ["~/docs/guide.md", "~/notes.txt"]);

    let hidden =
        matching_file_paths_with_home_for_test(Path::new("/tmp"), "~/.cache/", Some(home.path()));
    assert_eq!(hidden.as_slice(), ["~/.cache/huge.bin"]);
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
fn ranked_matches_are_capped_for_weak_queries() {
    let paths = (0..(MAX_RANKED_FILE_MATCHES + 50))
        .map(|index| format!("file-{index:04}.rs"))
        .collect::<Vec<_>>();
    let matches = fuzzy_matching_paths(&paths, "file");
    assert_eq!(matches.len(), MAX_RANKED_FILE_MATCHES);
    assert!(matches[0].starts_with("file-"));
}

// Covers: mixing the two sources must filter server resources by the same query
// as the files, keep them reachable ahead of a long workspace listing, and leave
// the workspace order untouched. Appending them would bury them; re-ranking the
// files with them would move rows a person already learned the position of.
// Owner: pure unit (`@` palette ranking).
#[test]
fn palette_mixes_server_resources_ahead_of_workspace_files() {
    fn resource(uri: &str) -> crate::tools::mcp::McpResource {
        crate::tools::mcp::McpResource {
            server: "docs".into(),
            uri: uri.into(),
            name: uri.into(),
            title: None,
            description: None,
            mime_type: None,
            templated: false,
        }
    }

    let discovered = DiscoveredFilePaths::complete(vec![
        "notes/alpha.md".to_string(),
        "notes/beta.md".to_string(),
    ]);
    let resources = vec![resource("res://alpha"), resource("res://gamma")];

    let cases = [
        (
            "a query keeps only the resources it matches, resources first",
            "alpha",
            vec![
                FilePaletteEntry::McpResource(resource("res://alpha")),
                FilePaletteEntry::WorkspaceFile("notes/alpha.md".into()),
                FilePaletteEntry::WorkspaceFile("notes/beta.md".into()),
            ],
        ),
        (
            "a query no resource matches leaves the workspace listing alone",
            "beta",
            vec![
                FilePaletteEntry::WorkspaceFile("notes/alpha.md".into()),
                FilePaletteEntry::WorkspaceFile("notes/beta.md".into()),
            ],
        ),
        (
            "a bare mention offers everything in discovery order",
            "",
            vec![
                FilePaletteEntry::McpResource(resource("res://alpha")),
                FilePaletteEntry::McpResource(resource("res://gamma")),
                FilePaletteEntry::WorkspaceFile("notes/alpha.md".into()),
                FilePaletteEntry::WorkspaceFile("notes/beta.md".into()),
            ],
        ),
    ];

    for (name, query, expected) in cases {
        let matches = file_palette_matches(discovered.clone(), &resources, query);
        let rows = matches
            .rows(0, usize::MAX)
            .map(|(_, entry)| entry)
            .collect::<Vec<_>>();
        assert_eq!(rows, expected, "{name}");
    }
}

#[test]
fn scroll_counts_track_hidden_rows_above_and_below() {
    assert_eq!(file_palette_scroll_counts(12, 0, 5), (0, 0, 7));
    assert_eq!(file_palette_scroll_counts(12, 4, 5), (0, 0, 7));
    assert_eq!(file_palette_scroll_counts(12, 5, 5), (1, 1, 6));
    assert_eq!(file_palette_scroll_counts(12, 11, 5), (7, 7, 0));
}

#[test]
fn relative_directory_prefix_scopes_search_to_that_directory() {
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
fn relative_directory_prefix_stays_relative_inside_home() {
    let home = tempdir().unwrap();
    let workspace = home.path().join("project");
    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::write(workspace.join("src/lib.rs"), "").unwrap();

    let matches = matching_file_paths_with_home_for_test(&workspace, "src/", Some(home.path()));
    assert_eq!(matches.as_slice(), ["src/lib.rs"]);
}

#[test]
fn absolute_directory_prefix_stays_absolute() {
    let root = tempdir().unwrap();
    let workspace = root.path().join("project");
    let logs = root.path().join("logs");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&logs).unwrap();
    fs::write(logs.join("app.log"), "").unwrap();

    let query = format!("{}/", path_to_unix_string(&logs));
    let matches = matching_file_paths_with_home_for_test(&workspace, &query, None);
    assert_eq!(
        matches.as_slice(),
        [format!("{}/app.log", path_to_unix_string(&logs))]
    );
}

#[test]
fn parent_directory_prefix_scopes_outside_cwd() {
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
