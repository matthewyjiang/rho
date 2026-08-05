use std::{ops::ControlFlow, path::Path};

use regex::RegexBuilder;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    grep_format::format_results,
    hashline::SnapshotStore,
    path_glob::PathGlob,
    search::{
        clamp_limit, stop_reasons, StopReason, WorkspaceSearch, DEFAULT_MAX_RESULTS,
        MAX_RESULTS_CEILING, SEARCH_DEADLINE,
    },
    tool::{ToolError, ToolSpec},
    workspace_walk::{visit_files, HiddenFiles, WalkLimits, WalkOptions, WalkStop, WalkedFile},
};

/// Default emitted match lines per file in `content` mode.
const DEFAULT_MAX_PER_FILE: usize = 10;
/// Hard ceiling for per-file emitted match lines.
const MAX_PER_FILE_CEILING: usize = 100;
/// Skip files larger than this to avoid reading multi-gigabyte blobs.
pub(crate) const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// Bytes inspected at the start of a file for a NUL binary sniff.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;
/// Match-line display width before truncation.
const MAX_LINE_CHARS: usize = 200;
/// Cap regex compile heap so pathological patterns fail fast.
const REGEX_SIZE_LIMIT: usize = 10 * 1024 * 1024;
/// Cap DFA heap during regex compile.
const REGEX_DFA_SIZE_LIMIT: usize = 10 * 1024 * 1024;

pub(crate) struct GrepSearch;

#[derive(Deserialize)]
struct Args {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
    literal: Option<bool>,
    case_sensitive: Option<bool>,
    include_hidden: Option<bool>,
    max_results: Option<usize>,
    max_per_file: Option<usize>,
    output_mode: Option<String>,
}

pub(crate) struct GrepRequest {
    pub(crate) path: String,
    pub(crate) pattern_display: String,
    regex: regex::Regex,
    glob: Option<PathGlob>,
    hidden: HiddenFiles,
    max_results: usize,
    max_per_file: usize,
    pub(crate) output_mode: GrepOutputMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GrepOutputMode {
    Content,
    FilesWithMatches,
    Count,
}

impl GrepOutputMode {
    /// How many match lines a per-file scan keeps for display. Zero means the
    /// mode renders no line text, so a scan only needs counts.
    fn retained_lines(self, max_per_file: usize) -> usize {
        match self {
            Self::Content => max_per_file,
            Self::FilesWithMatches | Self::Count => 0,
        }
    }

    /// Whether a scan can quit at the first match in a file.
    fn stops_at_first_match(self) -> bool {
        match self {
            Self::FilesWithMatches => true,
            Self::Content | Self::Count => false,
        }
    }

    /// What one file's hit spends from the `max_results` budget: match lines
    /// in `content`, otherwise the file itself.
    fn budget_cost(self, hit: &FileHit) -> usize {
        match self {
            Self::Content => hit.lines.len(),
            Self::FilesWithMatches | Self::Count => 1,
        }
    }
}

impl GrepRequest {
    pub(crate) fn from_arguments(args: Value) -> Result<Self, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        let pattern_display = args.pattern.clone();
        let source = if args.literal.unwrap_or(false) {
            regex::escape(&args.pattern)
        } else {
            args.pattern
        };
        let regex = RegexBuilder::new(&source)
            .case_insensitive(!args.case_sensitive.unwrap_or(true))
            .size_limit(REGEX_SIZE_LIMIT)
            .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
            .build()
            .map_err(|error| {
                ToolError::Message(format!("invalid pattern '{pattern_display}': {error}"))
            })?;
        let output_mode = match args.output_mode.as_deref().unwrap_or("content") {
            "content" => GrepOutputMode::Content,
            "files_with_matches" => GrepOutputMode::FilesWithMatches,
            "count" => GrepOutputMode::Count,
            other => {
                return Err(ToolError::Message(format!(
                    "invalid output_mode '{other}': expected content, files_with_matches, or count"
                )));
            }
        };
        Ok(Self {
            path: args.path.unwrap_or_else(|| ".".into()),
            pattern_display,
            regex,
            glob: args.glob.as_deref().map(PathGlob::compile).transpose()?,
            hidden: if args.include_hidden.unwrap_or(false) {
                HiddenFiles::Include
            } else {
                HiddenFiles::Skip
            },
            max_results: clamp_limit(args.max_results, DEFAULT_MAX_RESULTS, MAX_RESULTS_CEILING),
            max_per_file: clamp_limit(
                args.max_per_file,
                DEFAULT_MAX_PER_FILE,
                MAX_PER_FILE_CEILING,
            ),
            output_mode,
        })
    }
}

impl WorkspaceSearch for GrepSearch {
    type Request = GrepRequest;

    const NAME: &'static str = "grep";

    fn spec() -> ToolSpec {
        ToolSpec {
            name: Self::NAME.into(),
            description: "Searches file contents under a directory with a regular expression. Skips ignored, hidden, and binary files. Returns matches grouped by file with line numbers. Content mode prefixes each file with a chainable [path#TAG] header and N:text rows so edit can validate anchors without a separate read_file.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string"},
                    "glob": {"type": "string"},
                    "literal": {"type": "boolean"},
                    "case_sensitive": {"type": "boolean"},
                    "include_hidden": {"type": "boolean"},
                    "max_results": {"type": "integer", "minimum": 1},
                    "max_per_file": {"type": "integer", "minimum": 1},
                    "output_mode": {
                        "type": "string",
                        "enum": ["content", "files_with_matches", "count"]
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    fn parse(arguments: Value) -> Result<GrepRequest, ToolError> {
        GrepRequest::from_arguments(arguments)
    }

    fn root(request: &GrepRequest) -> &str {
        &request.path
    }

    fn run(
        root: &Path,
        display_root: &str,
        request: &GrepRequest,
        cancelled: &dyn Fn() -> bool,
        snapshots: Option<&SnapshotStore>,
    ) -> Result<String, ToolError> {
        grep_workspace(root, display_root, request, cancelled, snapshots)
    }
}

/// One file that matched, in the shape every output mode renders from.
pub(crate) struct FileHit {
    pub(crate) relative: String,
    /// Full-file snapshot tag when content mode computed one for chaining.
    pub(crate) file_tag: Option<String>,
    /// Matching lines in the file, including any not retained below.
    pub(crate) total: usize,
    /// Retained match lines as `(line number, display text)`. Empty unless the
    /// output mode renders line text.
    pub(crate) lines: Vec<(usize, String)>,
}

impl FileHit {
    /// Match lines found but not shown, for the `... +N more` note.
    pub(crate) fn suppressed(&self) -> usize {
        self.total.saturating_sub(self.lines.len())
    }
}

pub(crate) struct GrepStats {
    /// Results counted against `max_results`: match lines in `content` mode,
    /// files otherwise.
    pub(crate) shown: usize,
    /// Matching lines across every file the walk visited. Exceeds `shown` when
    /// a limit cut the output short.
    pub(crate) total_matches: usize,
    pub(crate) reasons: Vec<StopReason>,
}

pub(crate) fn grep_workspace(
    root: &Path,
    display_root: &str,
    request: &GrepRequest,
    cancelled: &dyn Fn() -> bool,
    snapshots: Option<&SnapshotStore>,
) -> Result<String, ToolError> {
    let options = WalkOptions {
        hidden: request.hidden,
        limits: WalkLimits::within(SEARCH_DEADLINE),
    };
    let retained_per_file = request.output_mode.retained_lines(request.max_per_file);

    let mut hits: Vec<FileHit> = Vec::new();
    let mut shown = 0usize;
    let mut total_matches = 0usize;
    let mut per_file_truncated = 0usize;

    let walk_stop = visit_files(root, &options, |file: WalkedFile| {
        if cancelled() {
            return ControlFlow::Break(WalkStop::Cancelled);
        }
        if let Some(glob) = &request.glob {
            if !glob.matches(&file.relative) {
                return ControlFlow::Continue(());
            }
        }
        let Some(mut hit) = scan_file(request, file, retained_per_file, snapshots) else {
            return ControlFlow::Continue(());
        };

        total_matches = total_matches.saturating_add(hit.total);
        // Only meaningful where the mode keeps line text at all.
        if retained_per_file > 0 && hit.suppressed() > 0 {
            per_file_truncated = per_file_truncated.saturating_add(1);
        }

        // `shown < max_results` holds here: the walk breaks as soon as it is
        // reached, so `remaining` is always at least one. A file may carry
        // more match lines than the budget has left; the extra lines fall into
        // `suppressed()`. Modes that keep no line text are unaffected.
        let remaining = request.max_results - shown;
        hit.lines.truncate(remaining);
        shown = shown.saturating_add(request.output_mode.budget_cost(&hit));
        hits.push(hit);

        if shown >= request.max_results {
            ControlFlow::Break(WalkStop::ResultLimit)
        } else {
            ControlFlow::Continue(())
        }
    });

    Ok(format_results(
        request,
        display_root,
        &hits,
        GrepStats {
            shown,
            total_matches,
            reasons: stop_reasons(walk_stop, per_file_truncated),
        },
    ))
}

/// Scans one file, keeping at most `retain` match lines for display.
///
/// Returns `None` for unreadable, oversized, binary, or non-matching files, so
/// every output mode shares one read and one pass over the lines.
fn scan_file(
    request: &GrepRequest,
    file: WalkedFile,
    retain: usize,
    snapshots: Option<&SnapshotStore>,
) -> Option<FileHit> {
    let text = read_searchable_text(&file.absolute)?;
    let stop_early = request.output_mode.stops_at_first_match();
    let mut hit = FileHit {
        relative: file.relative,
        file_tag: None,
        total: 0,
        lines: Vec::new(),
    };
    for (index, line) in text.lines().enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if !request.regex.is_match(line) {
            continue;
        }
        hit.total = hit.total.saturating_add(1);
        if hit.lines.len() < retain {
            hit.lines.push((index + 1, normalize_match_text(line)));
        }
        if stop_early {
            break;
        }
    }
    if hit.total == 0 {
        return None;
    }
    if request.output_mode == GrepOutputMode::Content {
        let tag = crate::hashline::compute_file_hash(&text);
        if let Some(store) = snapshots {
            let seen = hit.lines.iter().map(|(n, _)| *n);
            store.record(&file.absolute, text, Some(seen));
        }
        hit.file_tag = Some(tag);
    }
    Some(hit)
}

fn read_searchable_text(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let sniff_len = BINARY_SNIFF_BYTES.min(bytes.len());
    if bytes[..sniff_len].contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn normalize_match_text(line: &str) -> String {
    let trimmed = line.trim();
    let mut out = String::with_capacity(trimmed.len());
    let mut previous_space = false;
    for ch in trimmed.chars() {
        if ch == ' ' || ch == '\t' {
            if !previous_space {
                out.push(' ');
                previous_space = true;
            }
        } else {
            previous_space = false;
            out.push(ch);
        }
    }
    truncate_chars(&out, MAX_LINE_CHARS)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}

#[cfg(test)]
#[path = "grep_tests.rs"]
mod tests;
