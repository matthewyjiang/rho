//! Inline markdown span scanning: emphasis, code spans, links, raw URLs, and
//! single-row `$...$` math, plus the stable-prefix rules that keep streamed
//! lines visible while markers are still open.

use ratatui::style::Style;

use super::super::{markdown_image, theme::Theme};
use super::{math, StyledSegment};

pub(super) fn markdown_inline_segments(line: &str) -> Vec<StyledSegment> {
    let mut segments = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        match next_markdown_span(rest) {
            Some(MarkdownSpan::Styled {
                start,
                marker_len,
                end,
                style,
            }) => {
                if start > 0 {
                    segments.push(StyledSegment::new(rest[..start].to_string(), Theme::text()));
                }
                let content_start = start + marker_len;
                let marked_end = end + marker_len;
                segments.push(StyledSegment::new(
                    rest[content_start..end].to_string(),
                    style,
                ));
                rest = &rest[marked_end..];
            }
            Some(MarkdownSpan::Image { start, end, alt }) => {
                if start > 0 {
                    segments.push(StyledSegment::new(rest[..start].to_string(), Theme::text()));
                }
                // Inline images cannot reserve rows inside wrapped prose, so
                // they fall back to their alt text.
                if !alt.is_empty() {
                    segments.push(StyledSegment::new(alt, Theme::text()));
                }
                rest = &rest[end..];
            }
            Some(MarkdownSpan::Link {
                start,
                end,
                label,
                target,
            }) => {
                if start > 0 {
                    segments.push(StyledSegment::new(rest[..start].to_string(), Theme::text()));
                }
                segments.push(StyledSegment::new(label, Theme::text()));
                segments.push(StyledSegment::new(": ".to_string(), Theme::text()));
                segments.push(StyledSegment::new(target, Theme::markdown_link()));
                rest = &rest[end..];
            }
            Some(MarkdownSpan::RawUrl { start, end }) => {
                if start > 0 {
                    segments.push(StyledSegment::new(rest[..start].to_string(), Theme::text()));
                }
                segments.push(StyledSegment::new(
                    rest[start..end].to_string(),
                    Theme::markdown_link(),
                ));
                rest = &rest[end..];
            }
            Some(MarkdownSpan::InlineMath { start, end }) => {
                if start > 0 {
                    segments.push(StyledSegment::new(rest[..start].to_string(), Theme::text()));
                }
                let source = &rest[start + "$".len()..end - "$".len()];
                match math::render_inline_math(source) {
                    Some(text) => {
                        segments.push(StyledSegment::new(text, Theme::code_text()));
                    }
                    // Formulas taller than one row keep their literal source so
                    // no math is lost inside wrapped prose.
                    None => {
                        segments.push(StyledSegment::new(
                            rest[start..end].to_string(),
                            Theme::text(),
                        ));
                    }
                }
                rest = &rest[end..];
            }
            None => {
                segments.push(StyledSegment::new(rest.to_string(), Theme::text()));
                break;
            }
        }
    }
    segments
}

#[derive(Debug)]
enum MarkdownSpan {
    Styled {
        start: usize,
        marker_len: usize,
        end: usize,
        style: Style,
    },
    Image {
        start: usize,
        end: usize,
        alt: String,
    },
    Link {
        start: usize,
        end: usize,
        label: String,
        target: String,
    },
    RawUrl {
        start: usize,
        end: usize,
    },
    /// Closed `$...$` span including both delimiters.
    InlineMath {
        start: usize,
        end: usize,
    },
}

fn next_markdown_span(line: &str) -> Option<MarkdownSpan> {
    let candidates = [
        next_markdown_image_span(line),
        next_markdown_link(line),
        next_raw_url(line),
        next_delimited(line, "`", Theme::markdown_inline_code()),
        next_inline_math(line),
        next_delimited(line, "**", Theme::markdown_bold()),
        next_delimited(line, "*", Theme::markdown_italic()),
        next_delimited(line, "_", Theme::markdown_italic()),
    ];
    candidates
        .into_iter()
        .flatten()
        .min_by_key(|span| match span {
            MarkdownSpan::Styled {
                start, marker_len, ..
            } => (*start, std::cmp::Reverse(*marker_len)),
            MarkdownSpan::Image { start, .. } => (*start, std::cmp::Reverse(1)),
            MarkdownSpan::Link { start, .. } => (*start, std::cmp::Reverse(1)),
            MarkdownSpan::RawUrl { start, .. } => (*start, std::cmp::Reverse(1)),
            MarkdownSpan::InlineMath { start, .. } => (*start, std::cmp::Reverse(1)),
        })
}

fn next_markdown_image_span(line: &str) -> Option<MarkdownSpan> {
    let (image, range) = markdown_image::next_markdown_image(line)?;
    Some(MarkdownSpan::Image {
        start: range.start,
        end: range.end,
        alt: image.alt,
    })
}

fn next_markdown_link(line: &str) -> Option<MarkdownSpan> {
    let start = line.find('[')?;
    let after_label = start + 1;
    let close_label = line[after_label..].find(']')? + after_label;
    let target_start = close_label + 2;
    if !line[close_label + 1..].starts_with('(') || target_start >= line.len() {
        return None;
    }
    let target_end = line[target_start..].find(')')? + target_start;
    let label = &line[after_label..close_label];
    let target = &line[target_start..target_end];
    (!label.is_empty() && !target.is_empty()).then(|| MarkdownSpan::Link {
        start,
        end: target_end + 1,
        label: label.to_string(),
        target: target.to_string(),
    })
}

fn next_raw_url(line: &str) -> Option<MarkdownSpan> {
    let start = match (line.find("https://"), line.find("http://")) {
        (Some(https), Some(http)) => https.min(http),
        (Some(https), None) => https,
        (None, Some(http)) => http,
        (None, None) => return None,
    };
    let mut end = line[start..]
        .find(|ch: char| ch.is_whitespace())
        .map_or(line.len(), |offset| start + offset);
    while end > start
        && matches!(
            line[..end].chars().last(),
            Some('.' | ',' | ';' | ':' | '!' | '?' | ')' | ']')
        )
    {
        end -= line[..end]
            .chars()
            .last()
            .expect("end is after start")
            .len_utf8();
    }
    (end > start).then_some(MarkdownSpan::RawUrl { start, end })
}

fn next_inline_math(line: &str) -> Option<MarkdownSpan> {
    let mut search_from = 0;
    while let Some(relative_start) = line[search_from..].find('$') {
        let start = search_from + relative_start;
        // `$$` belongs to display math and never opens an inline span.
        if line[start..].starts_with("$$") {
            search_from = start + "$$".len();
            continue;
        }
        if !is_valid_inline_math_opener(line, start) {
            search_from = start + '$'.len_utf8();
            continue;
        }

        let content_start = start + '$'.len_utf8();
        let mut end_search_from = content_start;
        while let Some(relative_end) = line[end_search_from..].find('$') {
            let end = end_search_from + relative_end;
            if !is_valid_inline_math_closer(line, end) {
                end_search_from = end + '$'.len_utf8();
                continue;
            }
            return Some(MarkdownSpan::InlineMath {
                start,
                end: end + '$'.len_utf8(),
            });
        }
        search_from = content_start;
    }
    None
}

/// Pandoc-style dollar rules keep currency out of math: an opener needs a
/// following non-space, non-digit character and a closer needs a preceding
/// non-space character and no digit right after.
fn is_valid_inline_math_opener(line: &str, marker_start: usize) -> bool {
    let after = line[marker_start + '$'.len_utf8()..].chars().next();
    after.is_some_and(|ch| !ch.is_whitespace() && !ch.is_ascii_digit() && ch != '$')
}

fn is_valid_inline_math_closer(line: &str, marker_start: usize) -> bool {
    let before = line[..marker_start].chars().next_back();
    let after = line[marker_start + '$'.len_utf8()..].chars().next();
    before.is_some_and(|ch| !ch.is_whitespace() && ch != '$')
        && !after.is_some_and(|ch| ch.is_ascii_digit())
}

/// Byte length of the leading inline-markdown prefix that is safe to draw.
///
/// Open emphasis, links, code spans, and raw URLs hold only from their earliest
/// opener onward. Text before that opener stays visible so the live line does
/// not blank while markers complete.
pub(super) fn inline_markdown_stable_prefix_len(line: &str) -> usize {
    first_unresolved_inline_markdown_start(line).unwrap_or(line.len())
}

/// Completed spans of one delimiter kind, plus the first still-open opener.
///
/// Ranges and `open_at` coexist so callers can ignore finished spans while still
/// taking the minimum unresolved cut across marker kinds.
#[derive(Debug, Default)]
struct InlineDelimScan {
    ranges: Vec<std::ops::Range<usize>>,
    open_at: Option<usize>,
}

fn first_unresolved_inline_markdown_start(line: &str) -> Option<usize> {
    let mut earliest: Option<usize> = None;
    let mut consider = |open_at: Option<usize>| {
        if let Some(pos) = open_at {
            earliest = Some(earliest.map_or(pos, |existing| existing.min(pos)));
        }
    };

    let code = complete_delimiter_ranges(line, "`", &[]);
    consider(code.open_at);

    let links = complete_link_ranges(line, &code.ranges);
    consider(links.open_at);

    let mut ignored_ranges = [code.ranges, links.ranges].concat();
    consider(first_unclosed_raw_url_start(line, &ignored_ranges));

    // Math before emphasis, so `*`/`_` inside a closed `$...$` span cannot hold
    // the live preview open.
    let math = complete_inline_math_ranges(line, &ignored_ranges);
    consider(math.open_at);
    ignored_ranges.extend(math.ranges);

    // Bold must win over italic for completed spans. Feed closed `**` ranges into
    // the ignored set before scanning `*`/`_`, or a closed bold run with trailing
    // text is treated as an open italic delimiter and the live preview vanishes.
    let bold = complete_delimiter_ranges(line, "**", &ignored_ranges);
    consider(bold.open_at);
    ignored_ranges.extend(bold.ranges);

    consider(complete_delimiter_ranges(line, "*", &ignored_ranges).open_at);
    consider(complete_delimiter_ranges(line, "_", &ignored_ranges).open_at);
    earliest
}

fn complete_link_ranges(line: &str, ignored_ranges: &[std::ops::Range<usize>]) -> InlineDelimScan {
    let mut ranges = Vec::new();
    let mut search_from = 0;
    while let Some(start) = find_char_outside_ranges(line, '[', search_from, ignored_ranges) {
        let Some(close_label) =
            find_char_outside_ranges(line, ']', start + '['.len_utf8(), ignored_ranges)
        else {
            return InlineDelimScan {
                ranges,
                open_at: Some(start),
            };
        };
        let after_label = close_label + ']'.len_utf8();
        // A trailing `]` may still grow into `](url)`. Hold from `[` until more
        // input arrives; a following non-'(' character means plain brackets.
        if after_label >= line.len() {
            return InlineDelimScan {
                ranges,
                open_at: Some(start),
            };
        }
        if !line[after_label..].starts_with('(') {
            search_from = after_label;
            continue;
        }
        let target_start = close_label + "](".len();
        if target_start >= line.len() {
            return InlineDelimScan {
                ranges,
                open_at: Some(start),
            };
        }
        let Some(target_end) = line[target_start..]
            .find(')')
            .map(|index| index + target_start)
        else {
            return InlineDelimScan {
                ranges,
                open_at: Some(start),
            };
        };
        if close_label == start + '['.len_utf8() || target_end == target_start {
            return InlineDelimScan {
                ranges,
                open_at: Some(start),
            };
        }
        ranges.push(start..target_end + ')'.len_utf8());
        search_from = target_end + ')'.len_utf8();
    }
    InlineDelimScan {
        ranges,
        open_at: None,
    }
}

fn complete_delimiter_ranges(
    line: &str,
    marker: &str,
    ignored_ranges: &[std::ops::Range<usize>],
) -> InlineDelimScan {
    let mut ranges = Vec::new();
    let mut search_from = 0;
    while let Some(start) = find_marker_outside_ranges(line, marker, search_from, ignored_ranges) {
        if marker == "*" && line[start..].starts_with("**") {
            // Skip the whole bold marker. Advancing by one left the second `*`
            // eligible and false-triggered unresolved italic after closed bold.
            search_from = start + "**".len();
            continue;
        }
        if !is_valid_stream_opener(line, marker, start) {
            search_from = start + marker.len();
            continue;
        }

        let content_start = start + marker.len();
        let mut end_search_from = content_start;
        let mut matched_end = None;
        while let Some(end) =
            find_marker_outside_ranges(line, marker, end_search_from, ignored_ranges)
        {
            if marker == "*" && line[end..].starts_with("**") {
                end_search_from = end + "**".len();
                continue;
            }
            if !is_valid_stream_closer(line, marker, end) {
                end_search_from = end + marker.len();
                continue;
            }
            if end > content_start {
                matched_end = Some(end);
            }
            break;
        }
        let Some(end) = matched_end else {
            return InlineDelimScan {
                ranges,
                open_at: Some(start),
            };
        };
        ranges.push(start..end + marker.len());
        search_from = end + marker.len();
    }
    InlineDelimScan {
        ranges,
        open_at: None,
    }
}

fn complete_inline_math_ranges(
    line: &str,
    ignored_ranges: &[std::ops::Range<usize>],
) -> InlineDelimScan {
    let mut ranges = Vec::new();
    let mut search_from = 0;
    while let Some(start) = find_marker_outside_ranges(line, "$", search_from, ignored_ranges) {
        if line[start..].starts_with("$$") {
            search_from = start + "$$".len();
            continue;
        }
        let content_start = start + '$'.len_utf8();
        // A `$` at EOL may still grow into an opener once more input arrives,
        // so it holds the preview like a trailing emphasis marker does.
        let after = line[content_start..].chars().next();
        if after.is_some_and(|ch| ch.is_whitespace() || ch.is_ascii_digit()) {
            search_from = content_start;
            continue;
        }

        let mut end_search_from = content_start;
        let mut matched_end = None;
        while let Some(end) = find_marker_outside_ranges(line, "$", end_search_from, ignored_ranges)
        {
            if !is_valid_inline_math_closer(line, end) {
                end_search_from = end + '$'.len_utf8();
                continue;
            }
            matched_end = Some(end);
            break;
        }
        let Some(end) = matched_end else {
            return InlineDelimScan {
                ranges,
                open_at: Some(start),
            };
        };
        ranges.push(start..end + '$'.len_utf8());
        search_from = end + '$'.len_utf8();
    }
    InlineDelimScan {
        ranges,
        open_at: None,
    }
}

fn is_valid_stream_opener(line: &str, marker: &str, marker_start: usize) -> bool {
    let before = line[..marker_start].chars().next_back();
    let after = line[marker_start + marker.len()..].chars().next();
    // Opening emphasis cannot run into whitespace. A marker at EOL is still a
    // potential opener so the missing closer keeps the line unresolved.
    if after.is_some_and(char::is_whitespace) {
        return false;
    }
    marker != "_"
        || !matches!((before, after), (Some(before), Some(after)) if is_word_char(before) && is_word_char(after))
}

fn is_valid_stream_closer(line: &str, marker: &str, marker_start: usize) -> bool {
    let before = line[..marker_start].chars().next_back();
    let after = line[marker_start + marker.len()..].chars().next();
    // Closing emphasis cannot follow whitespace, but may be followed by spaces
    // or more prose (`**bold** tail`). The old shared rule rejected that case
    // and hid the live markdown preview after every closed span.
    if before.is_none_or(char::is_whitespace) {
        return false;
    }
    marker != "_"
        || !matches!((before, after), (Some(before), Some(after)) if is_word_char(before) && is_word_char(after))
}

fn first_unclosed_raw_url_start(
    line: &str,
    ignored_ranges: &[std::ops::Range<usize>],
) -> Option<usize> {
    let mut search_from = 0;
    while let Some(start) = next_raw_url_start(line, search_from) {
        if !is_inside_ranges(start, ignored_ranges)
            && !line[start..].chars().any(char::is_whitespace)
        {
            return Some(start);
        }
        search_from = start + "http://".len();
    }
    None
}

fn next_raw_url_start(line: &str, search_from: usize) -> Option<usize> {
    ["https://", "http://"]
        .into_iter()
        .filter_map(|scheme| {
            line[search_from..]
                .find(scheme)
                .map(|index| search_from + index)
        })
        .min()
}

fn find_char_outside_ranges(
    line: &str,
    needle: char,
    search_from: usize,
    ignored_ranges: &[std::ops::Range<usize>],
) -> Option<usize> {
    line[search_from..]
        .char_indices()
        .map(|(index, ch)| (search_from + index, ch))
        .find(|(index, ch)| *ch == needle && !is_inside_ranges(*index, ignored_ranges))
        .map(|(index, _)| index)
}

fn find_marker_outside_ranges(
    line: &str,
    marker: &str,
    search_from: usize,
    ignored_ranges: &[std::ops::Range<usize>],
) -> Option<usize> {
    let mut current = search_from;
    while let Some(relative_index) = line[current..].find(marker) {
        let index = current + relative_index;
        if !is_inside_ranges(index, ignored_ranges) {
            return Some(index);
        }
        current = index + marker.len();
    }
    None
}

fn is_inside_ranges(index: usize, ranges: &[std::ops::Range<usize>]) -> bool {
    ranges
        .iter()
        .any(|range| range.start <= index && index < range.end)
}

fn next_delimited(line: &str, marker: &str, style: Style) -> Option<MarkdownSpan> {
    let mut search_from = 0;
    while let Some(relative_start) = line[search_from..].find(marker) {
        let start = search_from + relative_start;
        if marker == "*" && line[start..].starts_with("**") {
            search_from = start + "**".len();
            continue;
        }
        if marker == "_" && !is_valid_underscore_delimiter(line, start) {
            search_from = start + marker.len();
            continue;
        }

        let content_start = start + marker.len();
        let mut end_search_from = content_start;
        while let Some(relative_end) = line[end_search_from..].find(marker) {
            let end = end_search_from + relative_end;
            if marker == "*" && line[end..].starts_with("**") {
                end_search_from = end + "**".len();
                continue;
            }
            if marker == "_" && !is_valid_underscore_delimiter(line, end) {
                end_search_from = end + marker.len();
                continue;
            }
            if end > content_start {
                return Some(MarkdownSpan::Styled {
                    start,
                    marker_len: marker.len(),
                    end,
                    style,
                });
            }
            break;
        }
        search_from = content_start;
    }
    None
}

fn is_valid_underscore_delimiter(line: &str, marker_start: usize) -> bool {
    let before = line[..marker_start].chars().next_back();
    let after = line[marker_start + 1..].chars().next();
    !matches!((before, after), (Some(before), Some(after)) if is_word_char(before) && is_word_char(after))
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

pub(super) fn markdown_inline_text(line: &str) -> String {
    markdown_inline_segments(line)
        .iter()
        .map(|segment| segment.text.as_str())
        .collect()
}
