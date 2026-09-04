use rho_providers::model::{ContentBlock, Message, ToolCall, ToolResult};
use serde_json::json;

use super::*;
use crate::tui::syntax::warm_syntax_set;

fn user_text(text: &str) -> Message {
    Message::User(vec![ContentBlock::Text(text.into())])
}

fn tool_call_path(path: &str) -> Message {
    Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: "call".into(),
        name: "read_file".into(),
        arguments: json!({"path": path}),
    })])
}

// Covers: resume warmup collects fence languages and skips markdown
// Owner: pure unit (syntax warmup token scan)
#[test]
fn warmup_plan_collects_fences_and_skips_markdown() {
    let plan = SyntaxWarmupPlan::from_messages(&[user_text(
        "```rust\nfn x() {}\n```\n```md\n# h\n```\n```ts\nconst n = 1\n```\n```mermaid\ngraph TD\n```\n```plain\nhi\n```",
    )]);
    assert_eq!(
        plan,
        SyntaxWarmupPlan {
            tokens: vec!["rust".into(), "ts".into()],
            paths: Vec::new(),
        }
    );
}

// Covers: resume warmup uses structured tool-call paths, not tool-result bodies
// Owner: pure unit (syntax warmup path scan)
#[test]
fn warmup_plan_uses_tool_call_path_not_result_body() {
    let plan = SyntaxWarmupPlan::from_messages(&[
        tool_call_path("src/lib.rs"),
        Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
            id: "edit".into(),
            name: "edit".into(),
            arguments: json!({"file_path": "foo.ts", "old_string": "bar.py"}),
        })]),
        tool_call_path("README.md"),
        Message::ToolResult(ToolResult {
            id: "call".into(),
            ok: true,
            content: r#"also ignored.ts and {"path":"leaked.go"}"#.into(),
        }),
    ]);
    assert_eq!(
        plan,
        SyntaxWarmupPlan {
            tokens: Vec::new(),
            paths: vec!["src/lib.rs".into(), "foo.ts".into()],
        }
    );
}

// Covers: rust fence and .rs path compile once; markdown is skipped; count is capped
// Owner: pure unit (syntax warmup identity budget)
#[test]
fn warmup_names_dedup_skip_markdown_and_cap() {
    warm_syntax_set();
    let rust = syntax_name_for_language("rust").expect("bundled rust syntax");
    let bash = syntax_name_for_language("bash").expect("bundled bash syntax");
    let powershell = syntax_name_for_language("powershell").expect("bundled PowerShell syntax");
    assert_eq!(syntax_name_for_path("src/lib.rs"), Some(rust));

    let candidates = [
        "rust", "ts", "py", "go", "bash", "java", "c", "cpp", "ruby", "php", "swift", "kotlin",
        "lua", "json", "yaml", "toml", "html", "css", "sql", "markdown",
    ];
    let resolvable = candidates
        .iter()
        .filter(|token| should_warmup_token(token) && syntax_name_for_language(token).is_some())
        .count();
    assert!(
        resolvable > MAX_WARMED_SYNTAXES,
        "need more bundled languages than the cap, got {resolvable}"
    );

    let fences = candidates
        .iter()
        .map(|token| format!("```{token}\n"))
        .collect::<String>();
    let names: Vec<_> = planned_warmups(&SyntaxWarmupPlan::from_messages(&[
        user_text(&fences),
        tool_call_path("src/lib.rs"),
        tool_call_path("README.md"),
    ]))
    .into_iter()
    .map(|(name, _)| name)
    .collect();

    assert_eq!(names.iter().filter(|name| **name == rust).count(), 1);
    assert!(names.contains(&bash), "{names:?}");
    assert!(names.contains(&powershell), "{names:?}");
    assert!(
        names
            .iter()
            .all(|name| !name.eq_ignore_ascii_case("markdown")),
        "{names:?}"
    );
    assert_eq!(names.len(), MAX_WARMED_SYNTAXES);
}
