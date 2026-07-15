use async_trait::async_trait;
use serde::Deserialize;

use crate::{
    skills,
    tool::{truncate, Tool, ToolContext, ToolError, ToolResult, ToolSpec},
};

pub struct Skill;

#[derive(Deserialize)]
struct Args {
    name: String,
}

#[async_trait]
impl Tool for Skill {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill".into(),
            description: "Load the full SKILL.md content for an available skill by name.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The skill name to load"
                    }
                },
                "required": ["name"],
                "additionalProperties": false
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        let skill = skills::discover(&ctx.cwd)
            .into_iter()
            .find(|skill| skill.name == args.name)
            .ok_or_else(|| ToolError::Message(format!("unknown skill: {}", args.name)))?;

        Ok(ToolResult {
            id,
            ok: true,
            content: truncate(skill.contents, ctx.max_output_bytes),
        })
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn loads_skill_contents() {
        let root = TempDir::new().unwrap();
        let skill_dir = root.path().join(".agents/skills/test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: test desc\n---\nbody contents\n",
        )
        .unwrap();

        let result = Skill
            .call(
                serde_json::json!({"name": "test-skill"}),
                ToolContext {
                    cwd: root.path().to_path_buf(),
                    max_output_bytes: 12000,
                },
                "call_1".into(),
            )
            .await
            .unwrap();

        assert!(result.ok);
        assert!(result.content.contains("body contents"));
    }

    #[tokio::test]
    async fn rejects_unknown_skill_name() {
        let root = TempDir::new().unwrap();

        let err = Skill
            .call(
                serde_json::json!({"name": "missing-skill"}),
                ToolContext {
                    cwd: root.path().to_path_buf(),
                    max_output_bytes: 12000,
                },
                "call_1".into(),
            )
            .await
            .unwrap_err();

        assert_eq!(err.to_string(), "unknown skill: missing-skill");
    }

    #[tokio::test]
    async fn truncates_loaded_skill_contents() {
        let root = TempDir::new().unwrap();
        let skill_dir = root.path().join(".agents/skills/short-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: short-skill\ndescription: short desc\n---\nlong body contents\n",
        )
        .unwrap();

        let result = Skill
            .call(
                serde_json::json!({"name": "short-skill"}),
                ToolContext {
                    cwd: root.path().to_path_buf(),
                    max_output_bytes: 16,
                },
                "call_1".into(),
            )
            .await
            .unwrap();

        assert!(result.content.ends_with("\n[truncated]"));
    }
}
