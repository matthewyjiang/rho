//! Renders grep results. Every output mode reads the same [`FileHit`] list, so
//! the modes differ only in layout, never in what was collected.

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
            format!("{} matches in {} files", stats.total_matches, hits.len()),
        ),
    };
    format!("{body}\n{}", with_reasons(counts, &stats.reasons, NARROW))
}

fn content_body(hits: &[FileHit]) -> String {
    let mut body = String::new();
    for hit in hits {
        let _ = writeln!(body, "{}", hit.relative);
        for (line_no, text) in &hit.lines {
            let _ = writeln!(body, "  {line_no}: {text}");
        }
        if hit.suppressed() > 0 {
            let _ = writeln!(body, "  ... +{} more in this file", hit.suppressed());
        }
    }
    body
}

fn path_body(hits: &[FileHit]) -> String {
    let mut body = String::new();
    for hit in hits {
        let _ = writeln!(body, "{}", hit.relative);
    }
    body
}

fn count_body(hits: &[FileHit]) -> String {
    let mut body = String::new();
    for hit in hits {
        let _ = writeln!(body, "{}:{}", hit.relative, hit.total);
    }
    body
}

/// `content` mode is the only mode where the number shown can fall short of
/// the number found, so it is the only one that reports both.
fn content_counts(file_count: usize, stats: &GrepStats) -> String {
    if stats.shown == stats.total_matches {
        format!("{} matches in {file_count} files", stats.shown)
    } else {
        format!(
            "{} matches shown ({} total) in {file_count} files",
            stats.shown, stats.total_matches
        )
    }
}
