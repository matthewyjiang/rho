use std::{
    ops::ControlFlow,
    path::Path,
    time::{Duration, Instant},
};

use regex::RegexBuilder;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    path_glob::PathGlob,
    tool::*,
    workspace_walk::{visit_files, HiddenFiles, WalkLimits, WalkOptions, WalkStop, WalkedFile},
};

/// Default total match / file listing budget for a single search.
const DEFAULT_MAX_RESULTS: usize = 200;
/// Hard ceiling so callers cannot request unbounded output.
const MAX_RESULTS_CEILING: usize = 1_000;
/// Default emitted match lines per file in `content` mode.
const DEFAULT_MAX_PER_FILE: usize = 10;
/// Hard ceiling for per-file emitted match lines.
const MAX_PER_FILE_CEILING: usize = 100;
/// Walk bound: stop after inspecting this many directory entries.
const MAX_ENTRIES_SCANNED: usize = 200_000;
/// Wall-clock bound for one grep call.
const SEARCH_DEADLINE: Duration = Duration::from_secs(15);
/// Skip files larger than this to avoid reading multi-gigabyte blobs.
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// Bytes inspected at the start of a file for a NUL binary sniff.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;
/// Match-line display width before truncation.
const MAX_LINE_CHARS: usize = 200;
/// Cap regex compile heap so pathological patterns fail fast.
const REGEX_SIZE_LIMIT: usize = 10 * 1024 * 1024;
/// Cap DFA heap during regex compile.
const REGEX_DFA_SIZE_LIMIT: usize = 10 * 1024 * 1024;

pub struct Grep;

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

/// A validated search, built before any capability is requested so an
/// invalid pattern cannot cost an authorization round trip.
pub(super) struct GrepRequest {
    pub(super) path: String,
    pattern_display: String,
    regex: regex::Regex,
    glob: Option<PathGlob>,
    hidden: HiddenFiles,
    max_results: usize,
    max_per_file: usize,
    output_mode: GrepOutputMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GrepOutputMode {
    Content,
    FilesWithMatches,
    Count,
}

impl GrepRequest {
    pub(super) fn from_arguments(args: Value) -> Result<Self, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        Self::from_parsed(args)
    }

    fn from_parsed(args: Args) -> Result<Self, ToolError> {
        let pattern_display = args.pattern.clone();
        let literal = args.literal.unwrap_or(false);
        let case_sensitive = args.case_sensitive.unwrap_or(true);
        let source = if literal {
            regex::escape(&args.pattern)
        } else {
            args.pattern
        };
        let regex = RegexBuilder::new(&source)
            .case_insensitive(!case_sensitive)
            .size_limit(REGEX_SIZE_LIMIT)
            .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
            .build()
            .map_err(|error| {
                ToolError::Message(format!("invalid pattern '{pattern_display}': {error}"))
            })?;
        let glob = args.glob.as_deref().map(PathGlob::compile).transpose()?;
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
        let hidden = if args.include_hidden.unwrap_or(false) {
            HiddenFiles::Include
        } else {
            HiddenFiles::Skip
        };
        Ok(Self {
            path: args.path.unwrap_or_else(|| ".".into()),
            pattern_display,
            regex,
            glob,
            hidden,
            max_results: clamp(args.max_results, DEFAULT_MAX_RESULTS, MAX_RESULTS_CEILING),
            max_per_file: clamp(
                args.max_per_file,
                DEFAULT_MAX_PER_FILE,
                MAX_PER_FILE_CEILING,
            ),
            output_mode,
        })
    }
}

fn clamp(value: Option<usize>, default: usize, ceiling: usize) -> usize {
    value.unwrap_or(default).clamp(1, ceiling)
}

#[async_trait::async_trait]
impl Tool for Grep {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "grep".into(),
            description: "Searches file contents under a directory with a regular expression. Skips ignored, hidden, and binary files. Returns matches grouped by file with line numbers.".into(),
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

    async fn call(
        &self,
        args: Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let request = GrepRequest::from_arguments(args)?;
        let path = resolve_path(&ctx.cwd, &request.path);
        let display_root = compact_display_path(&ctx.cwd, &request.path);
        let content = tokio::task::spawn_blocking(move || {
            grep_workspace(&path, &display_root, &request, &|| false)
        })
        .await
        .map_err(|error| ToolError::Message(format!("grep task failed: {error}")))??;
        Ok(ToolResult {
            id,
            ok: true,
            content: truncate(content, ctx.max_output_bytes),
        })
    }
}

pub(super) fn grep_workspace(
    root: &Path,
    display_root: &str,
    request: &GrepRequest,
    cancelled: &dyn Fn() -> bool,
) -> Result<String, ToolError> {
    let options = WalkOptions {
        hidden: request.hidden,
        limits: WalkLimits {
            max_entries: MAX_ENTRIES_SCANNED,
            deadline: Instant::now() + SEARCH_DEADLINE,
        },
    };

    let mut files = Vec::new();
    let mut shown_matches = 0usize;
    let mut total_matches = 0usize;
    let mut files_truncated = 0usize;
    let mut result_limit_hit = false;

    let walk_stop = visit_files(root, &options, |file: WalkedFile| {
        if cancelled() {
            return ControlFlow::Break(WalkStop::Cancelled);
        }
        if let Some(glob) = &request.glob {
            if !glob.matches(&file.relative) {
                return ControlFlow::Continue(());
            }
        }

        match request.output_mode {
            GrepOutputMode::Content => {
                let Some(file_result) = search_file_content(request, &file) else {
                    return ControlFlow::Continue(());
                };
                if file_result.total == 0 {
                    return ControlFlow::Continue(());
                }
                total_matches = total_matches.saturating_add(file_result.total);
                if file_result.suppressed > 0 {
                    files_truncated = files_truncated.saturating_add(1);
                }
                let remaining = request.max_results.saturating_sub(shown_matches);
                if remaining == 0 {
                    result_limit_hit = true;
                    return ControlFlow::Break(WalkStop::ResultLimit);
                }
                let mut emitted = file_result.lines;
                if emitted.len() > remaining {
                    let extra = emitted.len() - remaining;
                    emitted.truncate(remaining);
                    files.push(ContentFile {
                        relative: file.relative,
                        lines: emitted,
                        suppressed: file_result.suppressed.saturating_add(extra),
                    });
                    shown_matches = request.max_results;
                    result_limit_hit = true;
                    return ControlFlow::Break(WalkStop::ResultLimit);
                }
                shown_matches = shown_matches.saturating_add(emitted.len());
                files.push(ContentFile {
                    relative: file.relative,
                    lines: emitted,
                    suppressed: file_result.suppressed,
                });
                if shown_matches >= request.max_results {
                    result_limit_hit = true;
                    return ControlFlow::Break(WalkStop::ResultLimit);
                }
                ControlFlow::Continue(())
            }
            GrepOutputMode::FilesWithMatches => {
                if !file_has_match(request, &file) {
                    return ControlFlow::Continue(());
                }
                files.push(ContentFile {
                    relative: file.relative,
                    lines: Vec::new(),
                    suppressed: 0,
                });
                shown_matches = shown_matches.saturating_add(1);
                total_matches = shown_matches;
                if shown_matches >= request.max_results {
                    result_limit_hit = true;
                    ControlFlow::Break(WalkStop::ResultLimit)
                } else {
                    ControlFlow::Continue(())
                }
            }
            GrepOutputMode::Count => {
                let count = count_file_matches(request, &file);
                if count == 0 {
                    return ControlFlow::Continue(());
                }
                total_matches = total_matches.saturating_add(count);
                files.push(ContentFile {
                    relative: file.relative,
                    lines: vec![(count, String::new())],
                    suppressed: 0,
                });
                shown_matches = shown_matches.saturating_add(1);
                if shown_matches >= request.max_results {
                    result_limit_hit = true;
                    ControlFlow::Break(WalkStop::ResultLimit)
                } else {
                    ControlFlow::Continue(())
                }
            }
        }
    });

    files.sort_by(|a, b| a.relative.cmp(&b.relative));

    Ok(format_results(
        request,
        display_root,
        &files,
        GrepFormatStats {
            shown_matches,
            total_matches,
            files_truncated,
            walk_stop,
            result_limit_hit,
        },
    ))
}

struct ContentFile {
    relative: String,
    /// For content: (line_no, text). For count: (count, unused).
    lines: Vec<(usize, String)>,
    suppressed: usize,
}

struct FileContentResult {
    lines: Vec<(usize, String)>,
    suppressed: usize,
    total: usize,
}

fn search_file_content(request: &GrepRequest, file: &WalkedFile) -> Option<FileContentResult> {
    let text = read_searchable_text(&file.absolute)?;
    let mut lines = Vec::new();
    let mut suppressed = 0usize;
    let mut total = 0usize;
    for (index, line) in text.lines().enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if !request.regex.is_match(line) {
            continue;
        }
        total = total.saturating_add(1);
        if lines.len() < request.max_per_file {
            lines.push((index + 1, normalize_match_text(line)));
        } else {
            suppressed = suppressed.saturating_add(1);
        }
    }
    Some(FileContentResult {
        lines,
        suppressed,
        total,
    })
}

fn file_has_match(request: &GrepRequest, file: &WalkedFile) -> bool {
    let Some(text) = read_searchable_text(&file.absolute) else {
        return false;
    };
    text.lines().any(|line| {
        let line = line.strip_suffix('\r').unwrap_or(line);
        request.regex.is_match(line)
    })
}

fn count_file_matches(request: &GrepRequest, file: &WalkedFile) -> usize {
    let Some(text) = read_searchable_text(&file.absolute) else {
        return 0;
    };
    text.lines()
        .filter(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            request.regex.is_match(line)
        })
        .count()
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

struct GrepFormatStats {
    shown_matches: usize,
    total_matches: usize,
    files_truncated: usize,
    walk_stop: WalkStop,
    result_limit_hit: bool,
}

fn format_results(
    request: &GrepRequest,
    display_root: &str,
    files: &[ContentFile],
    stats: GrepFormatStats,
) -> String {
    if files.is_empty() {
        return format!(
            "no matches for '{}' under {display_root}",
            request.pattern_display
        );
    }

    match request.output_mode {
        GrepOutputMode::Content => {
            let mut body = String::new();
            for file in files {
                body.push_str(&file.relative);
                body.push('\n');
                for (line_no, text) in &file.lines {
                    body.push_str(&format!("  {line_no}: {text}\n"));
                }
                if file.suppressed > 0 {
                    body.push_str(&format!("  ... +{} more in this file\n", file.suppressed));
                }
            }
            body.push('\n');
            body.push_str(&content_summary(
                stats.shown_matches,
                stats.total_matches,
                files.len(),
                stats.files_truncated,
                stats.walk_stop,
                stats.result_limit_hit,
            ));
            body
        }
        GrepOutputMode::FilesWithMatches => {
            let mut body = String::new();
            for file in files {
                body.push_str(&file.relative);
                body.push('\n');
            }
            body.push('\n');
            body.push_str(&files_summary(
                files.len(),
                stats.walk_stop,
                stats.result_limit_hit,
            ));
            body
        }
        GrepOutputMode::Count => {
            let mut body = String::new();
            let mut listed_total = 0usize;
            for file in files {
                let count = file.lines.first().map(|(n, _)| *n).unwrap_or(0);
                listed_total = listed_total.saturating_add(count);
                body.push_str(&format!("{}:{count}\n", file.relative));
            }
            body.push('\n');
            let summary_total =
                if stats.result_limit_hit || !matches!(stats.walk_stop, WalkStop::Completed) {
                    // When capped mid-walk, total_matches only covers visited files.
                    stats.total_matches
                } else {
                    listed_total
                };
            body.push_str(&count_summary(
                summary_total,
                files.len(),
                stats.walk_stop,
                stats.result_limit_hit,
            ));
            body
        }
    }
}

fn content_summary(
    shown: usize,
    total: usize,
    file_count: usize,
    files_truncated: usize,
    walk_stop: WalkStop,
    result_limit_hit: bool,
) -> String {
    let reasons = stop_reasons(walk_stop, result_limit_hit, files_truncated);
    if reasons.is_empty() && shown == total {
        return format!("{shown} matches in {file_count} files");
    }
    if files_truncated > 0 && shown != total {
        let mut line = format!("{shown} matches shown ({total} total) in {file_count} files");
        if !reasons.is_empty() {
            line.push_str(&format!(" ({})", reasons.join("; ")));
        }
        if !reasons.iter().any(|r| r.contains("narrow")) {
            line.push_str("; narrow the pattern, path, or glob");
        }
        return line;
    }
    let mut line = format!("{shown} matches shown in {file_count} files");
    if shown != total || !reasons.is_empty() {
        let mut detail = Vec::new();
        if shown != total {
            detail.push(format!("{total} total"));
        }
        detail.extend(reasons);
        line.push_str(&format!(" ({})", detail.join("; ")));
    }
    line
}

fn files_summary(file_count: usize, walk_stop: WalkStop, result_limit_hit: bool) -> String {
    let reasons = stop_reasons(walk_stop, result_limit_hit, 0);
    if reasons.is_empty() {
        format!("{file_count} files")
    } else {
        format!("{file_count} files ({})", reasons.join("; "))
    }
}

fn count_summary(
    total: usize,
    file_count: usize,
    walk_stop: WalkStop,
    result_limit_hit: bool,
) -> String {
    let reasons = stop_reasons(walk_stop, result_limit_hit, 0);
    if reasons.is_empty() {
        format!("{total} matches in {file_count} files")
    } else {
        format!(
            "{total} matches in {file_count} files ({})",
            reasons.join("; ")
        )
    }
}

fn stop_reasons(
    walk_stop: WalkStop,
    result_limit_hit: bool,
    files_truncated: usize,
) -> Vec<String> {
    let mut reasons = Vec::new();
    if result_limit_hit || matches!(walk_stop, WalkStop::ResultLimit) {
        reasons.push("result limit reached; narrow the pattern, path, or glob".to_string());
    }
    if files_truncated > 0 {
        reasons.push(format!(
            "{files_truncated} files truncated by max_per_file; raise max_per_file or narrow the pattern"
        ));
    }
    match walk_stop {
        WalkStop::EntryLimit => {
            reasons.push("scan limit reached; narrow the path or glob".to_string())
        }
        WalkStop::Deadline => reasons.push("time limit reached".to_string()),
        WalkStop::Cancelled => reasons.push("cancelled".to_string()),
        WalkStop::Completed | WalkStop::ResultLimit => {}
    }
    reasons
}

#[cfg(test)]
#[path = "grep_tests.rs"]
mod tests;
