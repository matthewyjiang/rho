use std::{ops::ControlFlow, path::Path};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    path_glob::PathGlob,
    search::{
        clamp_limit, stop_reasons, with_reasons, NarrowHint, WorkspaceSearch, DEFAULT_MAX_RESULTS,
        MAX_RESULTS_CEILING, SEARCH_DEADLINE,
    },
    tool::{ToolError, ToolSpec},
    workspace_walk::{visit_files, HiddenFiles, WalkLimits, WalkOptions, WalkStop, WalkedFile},
};

/// How glob tells the model to shrink a search.
const NARROW: NarrowHint = NarrowHint("the pattern or path");

pub(crate) struct GlobSearch;

#[derive(Deserialize)]
struct Args {
    pattern: String,
    path: Option<String>,
    include_hidden: Option<bool>,
    max_results: Option<usize>,
}

pub(crate) struct GlobRequest {
    pub(crate) path: String,
    pattern_display: String,
    glob: PathGlob,
    hidden: HiddenFiles,
    max_results: usize,
}

impl GlobRequest {
    pub(crate) fn from_arguments(args: Value) -> Result<Self, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        Ok(Self {
            pattern_display: args.pattern.clone(),
            glob: PathGlob::compile(&args.pattern)?,
            path: args.path.unwrap_or_else(|| ".".into()),
            hidden: if args.include_hidden.unwrap_or(false) {
                HiddenFiles::Include
            } else {
                HiddenFiles::Skip
            },
            max_results: clamp_limit(args.max_results, DEFAULT_MAX_RESULTS, MAX_RESULTS_CEILING),
        })
    }
}

impl WorkspaceSearch for GlobSearch {
    type Request = GlobRequest;

    const NAME: &'static str = "glob";

    fn spec() -> ToolSpec {
        ToolSpec {
            name: Self::NAME.into(),
            description: "Finds files matching a glob pattern under a directory. Skips ignored and hidden files. Returns relative paths in directory order.".into(),
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

    fn parse(arguments: Value) -> Result<GlobRequest, ToolError> {
        GlobRequest::from_arguments(arguments)
    }

    fn root(request: &GlobRequest) -> &str {
        &request.path
    }

    fn run(
        root: &Path,
        display_root: &str,
        request: &GlobRequest,
        cancelled: &dyn Fn() -> bool,
        _mint_tag: bool,
    ) -> Result<String, ToolError> {
        glob_workspace(root, display_root, request, cancelled)
    }
}

pub(crate) fn glob_workspace(
    root: &Path,
    display_root: &str,
    request: &GlobRequest,
    cancelled: &dyn Fn() -> bool,
) -> Result<String, ToolError> {
    let options = WalkOptions {
        hidden: request.hidden,
        limits: WalkLimits::within(SEARCH_DEADLINE),
    };

    let mut matches = Vec::new();
    let walk_stop = visit_files(root, &options, |file: WalkedFile| {
        if cancelled() {
            return ControlFlow::Break(WalkStop::Cancelled);
        }
        if !request.glob.matches(&file.relative) {
            return ControlFlow::Continue(());
        }
        matches.push(file.relative);
        if matches.len() >= request.max_results {
            ControlFlow::Break(WalkStop::ResultLimit)
        } else {
            ControlFlow::Continue(())
        }
    });

    let reasons = stop_reasons(walk_stop, /*per_file_truncated*/ 0);
    if matches.is_empty() {
        // Still report why, so a walk cut short by a limit or a cancellation is
        // never mistaken for a directory with no matching files.
        let counts = format!(
            "no files matching '{}' under {display_root}",
            request.pattern_display
        );
        return Ok(with_reasons(counts, &reasons, NARROW));
    }

    let counts = format!("{} files", matches.len());
    Ok(format!(
        "{}\n\n{}",
        matches.join("\n"),
        with_reasons(counts, &reasons, NARROW)
    ))
}

#[cfg(test)]
#[path = "glob_tests.rs"]
mod tests;
