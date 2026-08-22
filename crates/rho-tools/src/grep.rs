use std::{
    io::{BufRead, BufReader},
    ops::ControlFlow,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};

use regex::RegexBuilder;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    file_view::{FileViewPolicy, FileViewStyle},
    grep_format::format_results,
    hashline::{compute_file_hash, FileHash},
    path_glob::PathGlob,
    search::{
        clamp_limit, stop_reasons, StopReason, WorkspaceSearch, DEFAULT_MAX_RESULTS,
        MAX_RESULTS_CEILING, SEARCH_DEADLINE,
    },
    text_view::LineFingerprint,
    tool::{ToolError, ToolSpec},
    workspace_walk::{
        visit_files_parallel, HiddenFiles, WalkLimits, WalkOptions, WalkStop, WalkedFile,
    },
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

#[derive(Clone)]
pub(crate) struct GrepSearch {
    file_view: FileViewPolicy,
}

impl GrepSearch {
    pub(crate) fn new(file_view: FileViewPolicy) -> Self {
        Self { file_view }
    }
}

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
            description: "Searches file contents under a directory with a regular expression. Skips ignored, hidden, and binary files. Returns matches grouped by file with line numbers. Content mode shows matches as `N | text`. When the selected edit tool is hashline, each file is prefixed with a [path#TAG] snapshot header. Match text is a preview and may be truncated; use read_file when you need exact line text.".into(),
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
        &self,
        root: &Path,
        display_root: &str,
        request: &GrepRequest,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<String, ToolError> {
        grep_workspace(
            root,
            display_root,
            request,
            cancelled,
            self.file_view.style(),
        )
    }
}

/// One file that matched, in the shape every output mode renders from.
pub(crate) struct FileHit {
    pub(crate) relative: String,
    /// Full-file snapshot tag when content mode computed one for edit anchors.
    pub(crate) file_tag: Option<String>,
    /// Matching lines in the file, including any not retained below.
    pub(crate) total: usize,
    /// Retained match lines as `(line number, display text)`. Empty unless the
    /// output mode renders line text. Preview only - not hashline body text.
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
    style: FileViewStyle,
) -> Result<String, ToolError> {
    let options = WalkOptions {
        hidden: request.hidden,
        limits: WalkLimits::within(SEARCH_DEADLINE),
    };
    let retained_per_file = request.output_mode.retained_lines(request.max_per_file);

    let hits = Mutex::new(Vec::new());
    let spent = AtomicUsize::new(0);
    let cancelled = SharedCancel::new(cancelled);

    let walk_stop = visit_files_parallel(root, &options, |file: WalkedFile| {
        if cancelled.check() {
            return ControlFlow::Break(WalkStop::Cancelled);
        }
        if spent.load(Ordering::Relaxed) >= request.max_results {
            return ControlFlow::Break(WalkStop::ResultLimit);
        }
        if let Some(glob) = &request.glob {
            if !glob.matches(&file.relative) {
                return ControlFlow::Continue(());
            }
        }
        let Some(hit) = scan_file(request, file, retained_per_file, style) else {
            return ControlFlow::Continue(());
        };

        let cost = request.output_mode.budget_cost(&hit);
        let mut collected = hits.lock().unwrap_or_else(|poison| poison.into_inner());
        collected.push(hit);
        let now = spent
            .fetch_add(cost, Ordering::Relaxed)
            .saturating_add(cost);
        if now >= request.max_results {
            ControlFlow::Break(WalkStop::ResultLimit)
        } else {
            ControlFlow::Continue(())
        }
    });

    let mut hits = hits
        .into_inner()
        .unwrap_or_else(|poison| poison.into_inner());
    hits.sort_by(|left, right| left.relative.cmp(&right.relative));

    // Parallel discovery can overshoot the cap while in-flight scans finish.
    // Sort first, then keep the path-ordered prefix so output is deterministic.
    if hits.len() > request.max_results {
        hits.truncate(request.max_results);
    }

    let mut shown = 0usize;
    let mut keep = 0usize;
    let mut per_file_truncated = 0usize;
    for hit in &mut hits {
        if shown >= request.max_results {
            break;
        }
        // Count max_per_file cuts before the result-budget trim, so a file
        // split only by max_results does not look like a per-file truncation.
        if retained_per_file > 0 && hit.suppressed() > 0 {
            per_file_truncated = per_file_truncated.saturating_add(1);
        }
        let remaining = request.max_results - shown;
        hit.lines.truncate(remaining);
        shown = shown.saturating_add(request.output_mode.budget_cost(hit));
        keep = keep.saturating_add(1);
    }
    hits.truncate(keep);

    // Totals describe the deterministic kept prefix, not extra files a racy
    // worker may still have scanned after quit.
    let total_matches: usize = hits
        .iter()
        .fold(0, |acc, hit| acc.saturating_add(hit.total));

    // Serial grep reports ResultLimit whenever the shown budget is full, even
    // if the walk also finished the tree (one file can exhaust max_results).
    let walk_stop = if shown >= request.max_results {
        WalkStop::ResultLimit
    } else {
        walk_stop
    };

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
/// Returns `None` for unreadable, oversized, binary, or non-matching files.
/// Reads through a buffered stream so `files_with_matches` can stop at the
/// first hit and size is enforced by bytes read rather than a metadata call.
fn scan_file(
    request: &GrepRequest,
    file: WalkedFile,
    retain: usize,
    style: FileViewStyle,
) -> Option<FileHit> {
    let reader = BufReader::new(std::fs::File::open(&file.absolute).ok()?);
    let mint_tag = request.output_mode == GrepOutputMode::Content && style.mints_snapshot_tags();
    let scanned = scan_searchable_text(reader, request, retain, mint_tag)?;
    if scanned.total == 0 {
        return None;
    }
    Some(FileHit {
        relative: file.relative,
        file_tag: scanned.file_tag,
        total: scanned.total,
        lines: scanned.lines,
    })
}

struct ScannedFile {
    file_tag: Option<String>,
    total: usize,
    lines: Vec<(usize, String)>,
}

/// Line-oriented scan matching [`crate::text_view::iter_content_lines`].
///
/// `read_until('\n')` keeps the delimiter. Hashing follows
/// [`crate::hashline::compute_file_hash_bytes`]: every `\n` segment, including
/// the empty segment after a trailing newline. Match line numbers strip a
/// trailing `\n` first (so a final newline does not invent a blank line) and
/// then a trailing `\r`.
fn scan_searchable_text<R: BufRead>(
    mut reader: R,
    request: &GrepRequest,
    retain: usize,
    mint_tag: bool,
) -> Option<ScannedFile> {
    let stop_early = request.output_mode.stops_at_first_match() && !mint_tag;
    let mut hasher = mint_tag.then(FileHash::new);
    let mut hit = ScannedFile {
        file_tag: None,
        total: 0,
        lines: Vec::new(),
    };
    let mut buf = Vec::new();
    let mut bytes_read = 0u64;
    let mut sniff_remaining = BINARY_SNIFF_BYTES;
    let mut line_no = 0usize;
    // Holds the previous `\n` segment until the next chunk proves it is not
    // a trailing-newline phantom. Empty files never enter this path.
    let mut pending: Option<Vec<u8>> = None;
    let mut ended_with_lf = false;

    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf).ok()?;
        if n == 0 {
            break;
        }
        bytes_read = bytes_read.saturating_add(n as u64);
        if bytes_read > MAX_FILE_BYTES {
            return None;
        }
        if sniff_remaining > 0 {
            let sniff = &buf[..sniff_remaining.min(buf.len())];
            if sniff.contains(&0) {
                return None;
            }
            sniff_remaining = sniff_remaining.saturating_sub(sniff.len());
        }

        ended_with_lf = buf.last() == Some(&b'\n');
        if ended_with_lf {
            buf.pop();
        }
        if let Some(hasher) = hasher.as_mut() {
            hasher.push_line(&buf);
        }

        if let Some(previous) = pending.take() {
            match_pending(&mut hit, request, retain, &mut line_no, &previous)?;
            if stop_early && hit.total > 0 {
                break;
            }
        }

        if ended_with_lf {
            pending = Some(std::mem::take(&mut buf));
        } else {
            match_pending(&mut hit, request, retain, &mut line_no, &buf)?;
            break;
        }
    }

    if let Some(previous) = pending.take() {
        match_pending(&mut hit, request, retain, &mut line_no, &previous)?;
    }

    if mint_tag {
        if bytes_read == 0 {
            hit.file_tag = Some(compute_file_hash(""));
        } else {
            if ended_with_lf {
                if let Some(hasher) = hasher.as_mut() {
                    // `split('\n')` yields an empty last segment after a trailing newline.
                    hasher.push_line(b"");
                }
            }
            hit.file_tag = hasher.map(FileHash::finish);
        }
    }
    Some(hit)
}

/// `WorkspaceSearch` exposes cancel as `&dyn Fn() -> bool`, which is not
/// `Sync`. The adapter and tests pass thread-safe closures (`CancellationToken`
/// polls or literals); workers may call `check` concurrently.
struct SharedCancel<'a> {
    check: &'a dyn Fn() -> bool,
}

impl<'a> SharedCancel<'a> {
    fn new(check: &'a dyn Fn() -> bool) -> Self {
        Self { check }
    }

    fn check(&self) -> bool {
        (self.check)()
    }
}

// Safety: the captured callback is invoked as a pure poll and does not rely on
// thread-local state. Concurrent calls are required by the parallel walk.
unsafe impl Sync for SharedCancel<'_> {}
unsafe impl Send for SharedCancel<'_> {}

fn match_pending(
    hit: &mut ScannedFile,
    request: &GrepRequest,
    retain: usize,
    line_no: &mut usize,
    raw: &[u8],
) -> Option<()> {
    let line_bytes = raw.strip_suffix(b"\r").unwrap_or(raw);
    let line = std::str::from_utf8(line_bytes).ok()?;
    *line_no = line_no.saturating_add(1);
    if !request.regex.is_match(line) {
        return Some(());
    }
    hit.total = hit.total.saturating_add(1);
    if hit.lines.len() < retain {
        // Search preview only - may truncate. Not hashline `N:text` body text.
        hit.lines
            .push((*line_no, truncate_chars(line, MAX_LINE_CHARS)));
    }
    Some(())
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if let Some((idx, _)) = text.char_indices().nth(max_chars) {
        let mut out = String::with_capacity(idx + '…'.len_utf8());
        out.push_str(&text[..idx]);
        out.push('…');
        out
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
#[path = "grep_tests.rs"]
mod tests;
