use std::{
    ops::ControlFlow,
    path::Path,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    path_glob::PathGlob,
    tool::*,
    workspace_walk::{visit_files, HiddenFiles, WalkLimits, WalkOptions, WalkStop, WalkedFile},
};

/// Default number of paths returned by one glob call.
const DEFAULT_MAX_RESULTS: usize = 200;
/// Hard ceiling so callers cannot request unbounded listings.
const MAX_RESULTS_CEILING: usize = 1_000;
/// Walk bound: stop after inspecting this many directory entries.
const MAX_ENTRIES_SCANNED: usize = 200_000;
/// Wall-clock bound for one glob call.
const SEARCH_DEADLINE: Duration = Duration::from_secs(10);

pub struct Glob;

#[derive(Deserialize)]
struct Args {
    pattern: String,
    path: Option<String>,
    include_hidden: Option<bool>,
    max_results: Option<usize>,
}

/// A validated path search, built before any capability is requested.
pub(super) struct GlobRequest {
    pub(super) path: String,
    pattern_display: String,
    glob: PathGlob,
    hidden: HiddenFiles,
    max_results: usize,
}

impl GlobRequest {
    pub(super) fn from_arguments(args: Value) -> Result<Self, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        Self::from_parsed(args)
    }

    fn from_parsed(args: Args) -> Result<Self, ToolError> {
        let pattern_display = args.pattern.clone();
        let glob = PathGlob::compile(&args.pattern)?;
        let hidden = if args.include_hidden.unwrap_or(false) {
            HiddenFiles::Include
        } else {
            HiddenFiles::Skip
        };
        Ok(Self {
            path: args.path.unwrap_or_else(|| ".".into()),
            pattern_display,
            glob,
            hidden,
            max_results: args
                .max_results
                .unwrap_or(DEFAULT_MAX_RESULTS)
                .clamp(1, MAX_RESULTS_CEILING),
        })
    }
}

#[async_trait::async_trait]
impl Tool for Glob {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "glob".into(),
            description: "Finds files matching a glob pattern under a directory. Skips ignored and hidden files. Returns sorted relative paths.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string"},
                    "include_hidden": {"type": "boolean"},
                    "max_results": {"type": "integer", "minimum": 1}
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let request = GlobRequest::from_arguments(args)?;
        let path = resolve_path(&ctx.cwd, &request.path);
        let display_root = compact_display_path(&ctx.cwd, &request.path);
        let content = tokio::task::spawn_blocking(move || {
            glob_workspace(&path, &display_root, &request, &|| false)
        })
        .await
        .map_err(|error| ToolError::Message(format!("glob task failed: {error}")))??;
        Ok(ToolResult {
            id,
            ok: true,
            content: truncate(content, ctx.max_output_bytes),
        })
    }
}

pub(super) fn glob_workspace(
    root: &Path,
    display_root: &str,
    request: &GlobRequest,
    cancelled: &dyn Fn() -> bool,
) -> Result<String, ToolError> {
    let options = WalkOptions {
        hidden: request.hidden,
        limits: WalkLimits {
            max_entries: MAX_ENTRIES_SCANNED,
            deadline: Instant::now() + SEARCH_DEADLINE,
        },
    };

    let mut matches = Vec::new();
    let mut result_limit_hit = false;
    let walk_stop = visit_files(root, &options, |file: WalkedFile| {
        if cancelled() {
            return ControlFlow::Break(WalkStop::Cancelled);
        }
        if !request.glob.matches(&file.relative) {
            return ControlFlow::Continue(());
        }
        matches.push(file.relative);
        if matches.len() >= request.max_results {
            result_limit_hit = true;
            ControlFlow::Break(WalkStop::ResultLimit)
        } else {
            ControlFlow::Continue(())
        }
    });

    matches.sort();

    if matches.is_empty() {
        return Ok(format!(
            "no files matching '{}' under {display_root}",
            request.pattern_display
        ));
    }

    let mut body = matches.join("\n");
    body.push_str("\n\n");
    body.push_str(&summary(matches.len(), walk_stop, result_limit_hit));
    Ok(body)
}

fn summary(count: usize, walk_stop: WalkStop, result_limit_hit: bool) -> String {
    let mut reasons = Vec::new();
    if result_limit_hit || matches!(walk_stop, WalkStop::ResultLimit) {
        reasons.push("result limit reached; narrow the pattern or path".to_string());
    }
    match walk_stop {
        WalkStop::EntryLimit => {
            reasons.push("scan limit reached; narrow the path or pattern".to_string())
        }
        WalkStop::Deadline => reasons.push("time limit reached".to_string()),
        WalkStop::Cancelled => reasons.push("cancelled".to_string()),
        WalkStop::Completed | WalkStop::ResultLimit => {}
    }
    if reasons.is_empty() {
        format!("{count} files")
    } else {
        format!("{count} files ({})", reasons.join("; "))
    }
}

#[cfg(test)]
#[path = "glob_tests.rs"]
mod tests;
