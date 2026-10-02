//! Renders grep results. Every output mode reads the same [`FileHit`] list, so
//! paths are identical in text and structured output. Existence-only mode does
//! not claim counts; other modes render the counted matching lines.

use std::fmt::Write;

use crate::{
    grep::{FileHit, GrepOutputMode, GrepRequest, GrepStats},
    search::{with_reasons, NarrowHint},
};

/// How grep tells the model to shrink a search.
const NARROW: NarrowHint = NarrowHint("the pattern, path, or glob");

pub(crate) fn format_results(
    request: &GrepRequest,
    display_root: &str,
    hits: &[FileHit],
    stats: GrepStats,
) -> String {
    if hits.is_empty() {
        // Still report why, so a search cut short by a limit or a cancellation
        // is never mistaken for a search that found nothing.
        let counts = format!(
            "no matches for '{}' under {display_root}",
            request.pattern_display
        );
        return with_reasons(counts, &stats.reasons, NARROW);
    }

    let (body, counts) = match request.output_mode {
        GrepOutputMode::Content => (content_body(hits), content_counts(hits.len(), &stats)),
        GrepOutputMode::FilesWithMatches => (path_body(hits), format!("{} files", hits.len())),
        GrepOutputMode::Count => (
            count_body(hits),
            format!(
                "{} matches in {} files",
                stats.total_matches.expect_counted(),
                hits.len()
            ),
        ),
    };
    format!("{body}\n{}", with_reasons(counts, &stats.reasons, NARROW))
}

fn content_body(hits: &[FileHit]) -> String {
    let mut body = String::new();
    for hit in hits {
        let path = &hit.path;
        if let Some(tag) = &hit.file_tag {
            // Sole hashline wire emitter owns header shape. Path must be the
            // workspace-relative form edit resolves, not walk-root-relative.
            let _ = writeln!(body, "{}", crate::hashline::format_header(path, tag));
        } else {
            let _ = writeln!(body, "{path}");
        }
        for crate::grep::MatchLine {
            line: line_no,
            text,
        } in &hit.lines
        {
            // Preview shape uses `N | text`, not hashline `N:text`, so truncated
            // match bodies are not copy-pasteable into edit PUT rows.
            let _ = writeln!(body, "{line_no} | {text}");
        }
        if hit.suppressed() > 0 {
            let _ = writeln!(body, "... +{} more in this file", hit.suppressed());
        }
    }
    body
}

fn path_body(hits: &[FileHit]) -> String {
    let mut body = String::new();
    for hit in hits {
        let _ = writeln!(body, "{}", hit.path);
    }
    body
}

fn count_body(hits: &[FileHit]) -> String {
    let mut body = String::new();
    for hit in hits {
        let _ = writeln!(body, "{}:{}", hit.path, hit.count.expect_counted());
    }
    body
}

/// `content` mode is the only mode where the number shown can fall short of
/// the number found, so it is the only one that reports both.
fn content_counts(file_count: usize, stats: &GrepStats) -> String {
    let total_matches = stats.total_matches.expect_counted();
    if stats.shown == total_matches {
        format!("{} matches in {file_count} files", stats.shown)
    } else {
        format!(
            "{} matches shown ({total_matches} total) in {file_count} files",
            stats.shown
        )
    }
}
