use async_trait::async_trait;
use serde::Deserialize;

use crate::{
    skills,
    tool::{Tool, ToolContext, ToolError, ToolResult, ToolSpec},
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

    fn display_style(&self) -> crate::tool::ToolDisplayStyle {
        crate::tool::ToolDisplayStyle::skill()
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
            content: skill.contents,
        })
    }
}
