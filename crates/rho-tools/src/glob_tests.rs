use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use super::Glob;
use crate::tool::{Tool, ToolContext, ToolError};

fn ctx(dir: &TempDir) -> ToolContext {
    ToolContext {
        cwd: dir.path().to_path_buf(),
        max_output_bytes: 64_000,
    }
}

async fn call_glob(dir: &TempDir, args: serde_json::Value) -> Result<String, ToolError> {
    let result = Glob.call(args, ctx(dir), "id-1".into()).await?;
    assert!(result.ok);
    Ok(result.content)
}

fn write(dir: &TempDir, relative: &str, content: &str) {
    let path = dir.path().join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

#[tokio::test]
async fn nested_pattern_returns_sorted_relative_paths() {
    let dir = TempDir::new().unwrap();
    write(&dir, "src/b.rs", "");
    write(&dir, "src/a.rs", "");
    write(&dir, "src/nested/c.rs", "");
    write(&dir, "src/a.txt", "");
    std::fs::create_dir_all(dir.path().join("src/empty")).unwrap();

    let content = call_glob(&dir, json!({"pattern": "**/*.rs"}))
        .await
        .unwrap();
    assert_eq!(
        content,
        "\
src/a.rs
src/b.rs
src/nested/c.rs

3 files"
    );
}

#[tokio::test]
async fn path_scopes_search_and_paths_are_relative_to_scope() {
    let dir = TempDir::new().unwrap();
    write(&dir, "crates/rho/src/lib.rs", "");
    write(&dir, "crates/other/src/lib.rs", "");

    let content = call_glob(&dir, json!({"pattern": "**/*.rs", "path": "crates/rho"}))
        .await
        .unwrap();
    assert_eq!(
        content,
        "\
src/lib.rs

1 files"
    );
}

#[tokio::test]
async fn directories_are_absent_from_results() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    write(&dir, "src/main.rs", "");

    let content = call_glob(&dir, json!({"pattern": "src/**"})).await.unwrap();
    assert!(content.contains("src/main.rs\n"), "{content}");
    assert!(!content.lines().any(|line| line == "src"), "{content}");
}

#[tokio::test]
async fn gitignored_and_hidden_excluded_by_default() {
    let dir = TempDir::new().unwrap();
    write(&dir, ".gitignore", "ignored.rs\n");
    write(&dir, "ignored.rs", "");
    write(&dir, ".hidden.rs", "");
    write(&dir, "kept.rs", "");

    let content = call_glob(&dir, json!({"pattern": "*.rs"})).await.unwrap();
    assert_eq!(
        content,
        "\
kept.rs

1 files"
    );

    let hidden = call_glob(&dir, json!({"pattern": "*.rs", "include_hidden": true}))
        .await
        .unwrap();
    assert!(hidden.contains(".hidden.rs\n"), "{hidden}");
    assert!(!hidden.contains("ignored.rs"), "{hidden}");
}

#[tokio::test]
async fn max_results_notes_truncation() {
    let dir = TempDir::new().unwrap();
    write(&dir, "a.rs", "");
    write(&dir, "b.rs", "");
    write(&dir, "c.rs", "");

    let content = call_glob(&dir, json!({"pattern": "*.rs", "max_results": 1}))
        .await
        .unwrap();
    assert!(
        content.contains("1 files (result limit reached"),
        "{content}"
    );
    assert_eq!(
        content.lines().filter(|line| line.ends_with(".rs")).count(),
        1
    );
}

#[tokio::test]
async fn no_matches_message() {
    let dir = TempDir::new().unwrap();
    write(&dir, "a.txt", "");
    let content = call_glob(&dir, json!({"pattern": "*.rs", "path": "."}))
        .await
        .unwrap();
    assert_eq!(content, "no files matching '*.rs' under .");
}
