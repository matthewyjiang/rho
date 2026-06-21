use crate::tool::ToolSpec;

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
    out.push_str(
        r#"
Use tools only when needed. For questions answerable from context, reply directly.

Use structured tool calls when available. Do not write tool calls in prose.

Do not invent tool results. When done, answer directly.
"#,
    );
    out
}
