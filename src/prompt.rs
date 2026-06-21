use uuid::Uuid;

use crate::model::ModelError;
use crate::tool::{ToolCall, ToolSpec};

pub const BASE_SYSTEM_PROMPT: &str = "You are an expert coding assistant operating inside rho, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.";

pub fn system_prompt(tools: &[ToolSpec]) -> String {
    let mut out = BASE_SYSTEM_PROMPT.to_string();
    out.push_str("\n\nYou have access to the following tools:\n\n");
    for tool in tools {
        out.push_str(&format!(
            "- {}: {}\n  input_schema: {}\n",
            tool.name, tool.description, tool.input_schema
        ));
    }
    out.push_str(r#"
To call a tool, output exactly one JSON object in a fenced json code block.

Example:

```json
{
  "tool": "read_file",
  "arguments": {
    "path": "src/main.rs"
  }
}
```

Call one tool at a time. After receiving a tool result, continue from that result. Do not invent tool results. When the task is complete, answer the user directly.
"#);
    out
}

pub fn parse_tool_call(content: &str) -> Result<Option<ToolCall>, ModelError> {
    let Some(start) = content.find("```json") else {
        return Ok(None);
    };
    let after = &content[start + "```json".len()..];
    let Some(end) = after.find("```") else {
        return Ok(None);
    };
    let json = after[..end].trim();
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| ModelError::InvalidResponse(format!("invalid tool call json: {e}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| ModelError::InvalidResponse("expected JSON object".into()))?;
    let name = obj
        .get("tool")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ModelError::InvalidResponse("missing tool".into()))?
        .to_string();
    let arguments = obj
        .get("arguments")
        .cloned()
        .unwrap_or(serde_json::Value::Object(Default::default()));
    Ok(Some(ToolCall {
        id: Uuid::new_v4().to_string(),
        name,
        arguments,
    }))
}
