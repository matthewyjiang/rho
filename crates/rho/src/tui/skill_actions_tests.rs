use tempfile::TempDir;

use super::*;
use crate::tui::tests::test_app;

#[test]
fn skill_command_prefills_the_skill_tool_call_without_expanding_the_user_prompt() {
    let root = TempDir::new().unwrap();
    let skill_dir = root.path().join(".agents/skills/inspect");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: inspect\ndescription: Inspect things\ndisable-model-invocation: true\n---\nUse inspection tools.\n",
    )
    .unwrap();
    let mut app = test_app();
    app.info.runtime.cwd = root.path().to_path_buf();

    let SkillCommandAction::Prompt(prompt) = app
        .skill_command_action(
            "skill:inspect",
            "/skill:inspect Check errors.".into(),
            "/skill:inspect Check errors.".into(),
            true,
        )
        .unwrap()
    else {
        panic!("skill command should resolve to a prompt");
    };
    let call = prompt.initial_tool_call.clone().unwrap();

    assert_eq!(
        prompt,
        TurnPrompt::command(
            "/skill:inspect Check errors.".into(),
            "/skill:inspect Check errors.".into(),
        )
        .with_initial_tool_call(rho_sdk::model::ToolCall {
            id: call.id,
            name: "skill".into(),
            arguments: serde_json::json!({"name": "inspect"}),
        })
    );
}
