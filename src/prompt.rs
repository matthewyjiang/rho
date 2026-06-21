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
To call a tool, output exactly one valid JSON object in a fenced json code block. Escape newlines inside JSON string values as `\n`; do not put raw multiline strings inside JSON.

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
    let json = extract_first_json_object(after).ok_or_else(|| {
        ModelError::InvalidResponse("invalid tool call: expected JSON object".into())
    })?;
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| {
        let snippet: String = json.chars().take(500).collect();
        ModelError::InvalidResponse(format!("invalid tool call json: {e}; snippet: {snippet}"))
    })?;
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

fn extract_first_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;
    for (offset, ch) in s[start..].char_indices() {
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = start + offset + ch.len_utf8();
                    return Some(s[start..end].trim());
                }
            }
            _ => {}
        }
    }
    None
}
