//! Codex-compatible `apply_patch` workspace tool.
//!
//! Patch grammar and chunk matching are adapted from the Apache-2.0
//! codex-rs apply-patch crate so models trained on that format can reuse it.

mod apply;
mod parser;
mod seek_sequence;

use serde::Deserialize;
use serde_json::json;

use crate::tool::*;

pub(crate) use apply::apply_hunks;
pub(crate) use parser::{parse_patch, Hunk};

pub struct ApplyPatch;

const TOOL_DESCRIPTION: &str = r#"Use the `apply_patch` tool to edit files with a Codex-style patch.
Your patch language is a stripped-down, file-oriented diff format designed to be easy to parse and safe to apply:

*** Begin Patch
[ one or more file sections ]
*** End Patch

Each operation starts with one of three headers:
*** Add File: <path> - create a file. Every following content line is a + line.
*** Delete File: <path> - remove an existing file.
*** Update File: <path> - patch an existing file. Optional next line: *** Move to: <new path>
Then one or more hunks, each introduced by @@ (optionally followed by a header).
Within a hunk each line starts with ' ' (context), '-' (remove), or '+' (add).
Optional final marker inside a hunk: *** End of File

Example:
*** Begin Patch
*** Add File: hello.txt
+Hello world
*** Update File: src/app.py
@@ def greet():
-print("Hi")
+print("Hello, world!")
*** Delete File: obsolete.txt
*** End Patch

Rules:
- Include Begin/End markers and an action header for every file
- Prefix new file lines with + even when creating a file
- Prefer relative paths
- Prefer about 3 lines of context around each change
- Use @@ headers when context alone is not unique"#;

#[derive(Deserialize)]
struct Args {
    input: String,
}

#[async_trait::async_trait]
impl Tool for ApplyPatch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "apply_patch".into(),
            description: TOOL_DESCRIPTION.into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "input": {
                        "type": "string",
                        "description": "The full apply_patch document, including *** Begin Patch and *** End Patch markers."
                    }
                },
                "required": ["input"],
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
        let hunks =
            parse_patch(&args.input).map_err(|error| ToolError::Message(error.to_string()))?;
        let cwd = ctx.cwd.clone();
        let outcome = apply_hunks(
            hunks,
            |path| Ok(resolve_path(&cwd, path)),
            |path| compact_display_path(&cwd, path),
            ctx.max_output_bytes,
        )
        .await?;
        Ok(ToolResult {
            id,
            ok: true,
            content: outcome.content,
        })
    }
}

/// Extract path strings referenced by a patch input for presentation/prep.
pub fn patch_paths(input: &str) -> Vec<String> {
    match parse_patch(input) {
        Ok(hunks) => hunks
            .iter()
            .flat_map(Hunk::affected_paths)
            .filter_map(|path| path.to_str().map(str::to_owned))
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
#[path = "../apply_patch_tests.rs"]
mod tests;
