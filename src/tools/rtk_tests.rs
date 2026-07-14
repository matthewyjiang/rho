use std::path::Path;

use pretty_assertions::assert_eq;
use serde_json::Value;

use super::*;

#[test]
fn parses_rtk_versions() {
    assert_eq!(parse_version("rtk 0.23.0"), Some((0, 23, 0)));
    assert_eq!(parse_version("0.28.2"), Some((0, 28, 2)));
}

#[test]
fn old_rtk_versions_do_not_support_rewrite() {
    assert!(!supports_rewrite("rtk 0.22.9"));
    assert!(supports_rewrite("rtk 0.23.0"));
}

#[test]
fn encodes_project_paths_like_rtk_discover() {
    assert_eq!(
        encode_project_path(Path::new("/home/rho/a.project_name")),
        "-home-rho-a-project-name"
    );
    assert_eq!(
        encode_project_path(Path::new(r"C:\Users\rho\my project")),
        "C--Users-rho-my-project"
    );
}

#[tokio::test]
async fn logs_rtk_discover_compatible_tool_pairs() {
    let root = tempfile::tempdir().unwrap();
    let cwd = Path::new("/home/rho/project");

    let result = ToolResult {
        id: "call_1".into(),
        ok: true,
        content: "stdout:\nclean".into(),
    };
    log_execution_in_projects_dir(root.path(), cwd, "rtk git status", &result)
        .await
        .unwrap();

    let project_dir = root.path().join("-home-rho-project/rho-sessions");
    let path = std::fs::read_dir(project_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let lines = std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();

    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0].pointer("/message/content/0/input/command"),
        Some(&Value::String("rtk git status".into()))
    );
    let tool_use_id = lines[0]
        .pointer("/message/content/0/id")
        .and_then(Value::as_str)
        .unwrap();
    assert!(tool_use_id.starts_with("rho-"));
    assert_eq!(
        lines[1]
            .pointer("/message/content/0/tool_use_id")
            .and_then(Value::as_str),
        Some(tool_use_id)
    );
    let logged_output = lines[1]
        .pointer("/message/content/0/content")
        .and_then(Value::as_str)
        .unwrap();
    assert_eq!(logged_output.len(), "stdout:\nclean".len());
    assert!(logged_output.chars().all(|ch| ch == ' '));
    assert_eq!(
        lines[1].pointer("/message/content/0/is_error"),
        Some(&Value::Bool(false))
    );
}
