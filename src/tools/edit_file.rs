use crate::tool::*;
use serde::Deserialize;
use serde_json::json;

pub struct EditFile;
#[derive(Deserialize)]
struct Args {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[async_trait::async_trait]
impl Tool for EditFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit_file".into(),
            description: "Edits an existing UTF-8 text file by exact string replacement.".into(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["path","old_string","new_string"]}),
        }
    }
    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        let path = resolve_path(&ctx.cwd, &args.path);
        let content = std::fs::read_to_string(&path)?;
        let count = content.matches(&args.old_string).count();
        if args.old_string.is_empty() {
            return Err(ToolError::Message("old_string must not be empty".into()));
        }
        if !args.replace_all && count != 1 {
            return Err(ToolError::Message(format!(
                "old_string appeared {count} times, expected exactly once"
            )));
        }
        if args.replace_all && count == 0 {
            return Err(ToolError::Message("old_string appeared 0 times".into()));
        }
        let new_content = if args.replace_all {
            content.replace(&args.old_string, &args.new_string)
        } else {
            content.replacen(&args.old_string, &args.new_string, 1)
        };
        std::fs::write(&path, new_content)?;
        Ok(ToolResult {
            id,
            ok: true,
            content: format!(
                "edited {}; replaced {} occurrence(s)",
                path.display(),
                count
            ),
        })
    }
}
