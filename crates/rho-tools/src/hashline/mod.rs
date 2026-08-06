//! Line-anchored multi-hunk edit tool (`edit`) with snapshot tags.
//!
//! `read_file` / `write` mint `[path#TAG]` snapshots. `grep` content mode
//! mints headers + line numbers for anchors (match text is preview only). `edit`
//! applies a compact PUT/CUT document against those original line numbers and
//! rejects stale tags. Failures leave the file untouched and return a bounded
//! live snapshot to copy.

mod apply;
mod execute;
mod format;
mod parser;
mod proposed;

use serde::Deserialize;
use serde_json::json;

use crate::tool::*;

pub(crate) use execute::{apply_prepared_sections, claim_unique_path, PreparedSection};
pub(crate) use format::{
    compute_file_hash, format_chain_snapshot, format_hashline_view, format_header,
    split_content_lines,
};
pub(crate) use parser::parse_hashline;
pub use proposed::{
    planned_edit, proposed_edit, proposed_sections, EditPreview, EditPreviewKind, ProposedSection,
    EDIT_DOCUMENT_ONLY_NOTICE,
};

/// Operational contract only. Chaining policy and dialect tips live in the system
/// prompt / docs so this schema string does not drift as a third essay.
const TOOL_DESCRIPTION: &str = r#"Multi-hunk line-anchored edits to existing UTF-8 files.

Requires a fresh `[path#TAG]` from `read_file`, `grep` (content mode TAG + line numbers only), `write` snapshot, a prior non-structural `edit` preview, or a failed `edit` live snapshot. Never invent a TAG. Prefer `write` to create or fully rewrite a file.

Document:
[path#TAG]
PUT N:
+replacement
PUT N.=M:
+range body
PUT <N: / PUT >N: / PUT >$:
+insert
CUT N.=M

Locators: `PUT 12:` (single line; never `PUT 12.:`), `PUT 12.=15:` (inclusive range; also `12-15` / `12..15`), inserts as above, `CUT` without a colon. Body rows under `:` headers must start with `+`. PUT needs ≥1 body row; use CUT to delete. Line numbers are original snapshot lines and do not shift mid-document. One section per path; do not stack two `edit` calls on the same path in one batch. Stale TAG, overlap, out-of-range, and mid-edit changes fail closed with a live snapshot to copy."#;

pub(crate) struct Edit;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    input: String,
}

impl Tool for Edit {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".into(),
            description: TOOL_DESCRIPTION.into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "input": {
                        "type": "string",
                        "description": "Hashline document: one or more [path#TAG] sections with PUT/CUT ops. Copy each TAG from a fresh snapshot; never invent tags."
                    }
                },
                "required": ["input"],
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
            // App-tool harness for unit tests only. Production runs go through the
            // SDK EditTool (workspace resolve, capabilities, resources, revalidate).
            let args: Args = serde_json::from_value(args)?;
            let sections = parse_hashline(&args.input)
                .map_err(|error| ToolError::Message(error.to_string()))?
                .into_iter()
                .map(|section| PreparedSection {
                    path: resolve_path(&ctx.cwd, &section.path),
                    display_path: compact_display_path(&ctx.cwd, &section.path),
                    section,
                })
                .collect();
            let outcome = apply_prepared_sections(sections, ctx.max_output_bytes).await?;
            Ok(ToolResult {
                id,
                ok: true,
                content: outcome.content,
            })
        })
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
