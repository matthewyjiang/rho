use tempfile::TempDir;

use super::*;
use crate::tui::tests::test_app;

#[test]
fn skill_command_uses_expanded_model_prompt_and_compact_user_prompt() {
    let root = TempDir::new().unwrap();
    let skill_dir = root.path().join(".agents/skills/inspect");
    std::fs::create_dir_all(&skill_dir).unwrap();
    let skill_path = skill_dir.join("SKILL.md");
    std::fs::write(
        &skill_path,
        "---\nname: inspect\ndescription: Inspect things\n---\nUse inspection tools.\n",
    )
    .unwrap();
    let mut app = test_app();
    app.info.runtime.cwd = root.path().to_path_buf();

    let prompt = app
        .skill_command_prompt(
            "skill:inspect",
            "Check errors.",
            "/skill:inspect Check errors.".into(),
        )
        .unwrap()
        .unwrap();

    let skill_path = crate::paths::display(&skill_path);
    let skill_dir = crate::paths::display(&skill_dir);
    assert_eq!(
        prompt,
        TurnPrompt::command(
            format!(
                "<skill name=\"inspect\" location=\"{skill_path}\">\nReferences are relative to {skill_dir}.\n\nUse inspection tools.\n</skill>\n\nCheck errors."
            ),
            "/skill:inspect Check errors.".into(),
        )
    );
}
