//! Single-file exact string replacement tool (`str_replace`).
//!
//! One call edits one existing UTF-8 file. By default the old string must match
//! exactly once after newline normalization. Set `replace_all` to replace every
//! occurrence.

mod content;

use serde::Deserialize;
use serde_json::json;

use crate::tool::*;

pub(crate) use content::str_replace_content;
use content::validate_edit_args;

pub struct StrReplace;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StrReplaceArgs {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

impl StrReplaceArgs {
    pub(crate) fn validate(&self) -> Result<(), ToolError> {
        validate_edit_args(&self.old_string, &self.new_string)
    }
}

impl Tool for StrReplace {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "str_replace".into(),
            description: "Edits an existing UTF-8 text file by string replacement. Matching normalizes CRLF/LF newlines while preserving the file's newline style on write. By default old_string must match exactly once; set replace_all to replace every match. Use write to create or fully rewrite a file.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path of an existing file to edit."
                    },
                    "old_string": {
                        "type": "string",
                        "description": "Text to find. Must be non-empty and must differ from new_string. CRLF and LF newlines are treated equivalently when matching."
                    },
                    "new_string": {
                        "type": "string",
                        "description": "Replacement text. Newlines are rewritten to match the file's existing style."
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "When true, replace every occurrence of old_string. When false or omitted, require exactly one match."
                    }
                },
                "required": ["path", "old_string", "new_string"],
                "additionalProperties": false
            }),
        }
    }

    fn call<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> AppToolFuture<'a> {
        Box::pin(async move {
            let args: StrReplaceArgs = serde_json::from_value(args)?;
            let path = resolve_path(&ctx.cwd, &args.path);
            let outcome = str_replace_content(
                &path,
                &compact_display_path(&ctx.cwd, &args.path),
                &args.old_string,
                &args.new_string,
                args.replace_all,
                ctx.max_output_bytes,
            )
            .await?;
            Ok(ToolResult {
                id,
                ok: true,
                content: outcome.content,
            })
        })
    }
}

#[cfg(test)]
#[path = "../str_replace_tests.rs"]
mod tests;
