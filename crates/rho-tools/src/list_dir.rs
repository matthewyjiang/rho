#[cfg(all(test, unix))]
#[path = "list_dir_tests.rs"]
mod tests;

use std::path::Path;

use crate::tool::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub struct ListDir;
#[derive(Deserialize)]
struct Args {
    path: String,
}

impl Tool for ListDir {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_dir".into(),
            description: "Lists the immediate entries of one directory, sorted by name, one per line. Directory names end with `/`. Not recursive, and it does not skip hidden or ignored entries; use `glob` to find files by pattern across a tree. Output beyond the tool-output limit is truncated.".into(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string","description":"Directory to list, absolute or relative to the working directory."}},"required":["path"]}),
        }
    }

    fn call<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> AppToolFuture<'a> {
        Box::pin(async move {
            let args: Args = serde_json::from_value(args)?;
            let path = resolve_path(&ctx.cwd, &args.path);
            let entries = list_directory(&path).await?;
            Ok(render_listing(entries, ctx.max_output_bytes).into_result(id))
        })
    }
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EntryKind {
    File,
    Dir,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct Entry {
    name: String,
    kind: EntryKind,
}

impl Entry {
    fn display(&self) -> String {
        match self.kind {
            EntryKind::File => self.name.clone(),
            EntryKind::Dir => format!("{}/", self.name),
        }
    }
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct Listing {
    entries: Vec<Entry>,
    truncated: bool,
}

fn render_entries(entries: &[Entry]) -> String {
    entries
        .iter()
        .map(Entry::display)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Keep complete entries in the script view, even when names contain newlines.
pub(crate) fn render_listing(
    entries: Vec<Entry>,
    max_output_bytes: usize,
) -> crate::Rendered<Listing> {
    let text = render_entries(&entries);
    let truncated = text.len() > max_output_bytes;
    crate::Rendered::new(
        truncate(text, max_output_bytes),
        Listing { entries, truncated },
    )
}

pub(super) async fn list_directory(path: &Path) -> Result<Vec<Entry>, ToolError> {
    let mut listing = Vec::new();
    let mut entries = tokio::fs::read_dir(path).await?;
    while let Some(entry) = entries.next_entry().await? {
        listing.push(Entry {
            name: entry.file_name().to_string_lossy().into_owned(),
            kind: if entry.file_type().await?.is_dir() {
                EntryKind::Dir
            } else {
                EntryKind::File
            },
        });
    }
    listing.sort_by_cached_key(Entry::display);
    Ok(listing)
}
