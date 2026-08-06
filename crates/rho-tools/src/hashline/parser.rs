//! Line-oriented parser for Rho's hashline edit subset.
//!
//! Supported operations (oh-my-pi compatible subset):
//! - section header `[path#TAG]`
//! - `PUT N.=M:` replace inclusive original lines with `+` body rows
//! - `PUT <N:` / `PUT >N:` / `PUT >$:` insert body rows at a gap
//! - `CUT N.=M` delete inclusive original lines
//!
//! Intentionally omitted in this first cut: block ops (`N*`), registers,
//! `REM`, and `MV`. Use `write` to create or fully rewrite files.

use super::format::{FILE_HASH_LENGTH, FILE_HASH_SEP};

/// One file section in a hashline edit document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Section {
    pub path: String,
    pub tag: String,
    pub ops: Vec<Op>,
}

/// A single edit against original line numbers in one section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    /// Replace inclusive `[start, end]` with `body` lines.
    Replace {
        start: usize,
        end: usize,
        body: Vec<String>,
    },
    /// Insert `body` before `line` (1 = head).
    InsertBefore { line: usize, body: Vec<String> },
    /// Insert `body` after `line`, or after the last line when `line` is `None` (EOF).
    InsertAfter {
        line: Option<usize>,
        body: Vec<String>,
    },
    /// Delete inclusive `[start, end]`.
    Delete { start: usize, end: usize },
}

/// How the parser treats input it cannot understand.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Strictness {
    /// Any malformed line fails the whole document. Used before writing.
    Reject,
    /// Malformed and half-streamed lines are skipped. Used for preview cards.
    Skip,
}

/// Parse a complete hashline edit document into file sections.
pub(crate) fn parse_hashline(input: &str) -> Result<Vec<Section>, String> {
    parse(input, Strictness::Reject)
}

/// Best-effort parse of a possibly incomplete document for presentation cards.
pub(super) fn parse_lenient(input: &str) -> Vec<Section> {
    parse(input, Strictness::Skip).unwrap_or_default()
}

fn parse(input: &str, strictness: Strictness) -> Result<Vec<Section>, String> {
    let reject = strictness == Strictness::Reject;
    let mut sections = Vec::new();
    let mut current: Option<SectionBuilder> = None;
    let mut lines = input.lines().peekable();

    while let Some(raw) = lines.next() {
        let line = strip_bom(raw).trim_end();
        if line.is_empty() {
            continue;
        }
        match parse_header(line) {
            Ok(Some((path, tag))) => {
                if let Some(section) = current.replace(SectionBuilder {
                    path,
                    tag,
                    ops: Vec::new(),
                }) {
                    sections.push(section.finish(strictness)?);
                }
                continue;
            }
            Ok(None) => {}
            Err(error) if reject => return Err(error),
            Err(_) => continue,
        }
        let Some(builder) = current.as_mut() else {
            if reject {
                return Err(format!("hashline op outside a [path#TAG] section: {line}"));
            }
            continue;
        };

        match parse_cut(line) {
            Ok(Some(op)) => {
                builder.ops.push(op);
                continue;
            }
            Ok(None) => {}
            Err(error) if reject => return Err(error),
            Err(_) => continue,
        }
        match parse_put_header(line) {
            Ok(Some(op_head)) => {
                let body = take_body(&mut lines);
                match op_head.with_body(body) {
                    Ok(op) => builder.ops.push(op),
                    Err(error) if reject => return Err(error),
                    // A trailing `PUT …:` whose body has not streamed in yet.
                    Err(_) => {}
                }
                continue;
            }
            Ok(None) => {}
            Err(error) if reject => return Err(error),
            Err(_) => continue,
        }
        if reject {
            return Err(format!(
                "unrecognized hashline line (expected PUT/CUT or [path#TAG]): {line}"
            ));
        }
    }

    if let Some(section) = current.take() {
        sections.push(section.finish(strictness)?);
    }
    if reject && sections.is_empty() {
        return Err(
            "hashline input must contain at least one [path#TAG] section with operations".into(),
        );
    }
    Ok(sections)
}

struct SectionBuilder {
    path: String,
    tag: String,
    ops: Vec<Op>,
}

impl SectionBuilder {
    fn finish(self, strictness: Strictness) -> Result<Section, String> {
        if self.ops.is_empty() && strictness == Strictness::Reject {
            return Err(format!(
                "hashline section [{}#{}] has no operations",
                self.path, self.tag
            ));
        }
        Ok(Section {
            path: self.path,
            tag: self.tag,
            ops: self.ops,
        })
    }
}

enum PutHead {
    Replace { start: usize, end: usize },
    InsertBefore { line: usize },
    InsertAfter { line: Option<usize> },
}

impl PutHead {
    fn with_body(self, body: Vec<String>) -> Result<Op, String> {
        match self {
            Self::Replace { start, end } => {
                if body.is_empty() {
                    return Err(format!(
                        "PUT {start}.={end}: requires at least one + body row (use CUT to delete)"
                    ));
                }
                Ok(Op::Replace { start, end, body })
            }
            Self::InsertBefore { line } => {
                if body.is_empty() {
                    return Err(format!(
                        "PUT <{line}: requires at least one + body row (use CUT to delete)"
                    ));
                }
                Ok(Op::InsertBefore { line, body })
            }
            Self::InsertAfter { line } => {
                if body.is_empty() {
                    let label = line
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "$".into());
                    return Err(format!(
                        "PUT >{label}: requires at least one + body row (use CUT to delete)"
                    ));
                }
                Ok(Op::InsertAfter { line, body })
            }
        }
    }
}

fn parse_header(line: &str) -> Result<Option<(String, String)>, String> {
    let trimmed = line.trim();
    if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
        return Ok(None);
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    let Some((path, tag)) = inner.rsplit_once(FILE_HASH_SEP) else {
        return Err(format!(
            "hashline header must look like [path#TAG], got: {trimmed}"
        ));
    };
    if path.is_empty() {
        return Err(format!("hashline header path is empty: {trimmed}"));
    }
    let tag = tag.trim();
    if tag.len() != FILE_HASH_LENGTH || !tag.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(format!(
            "hashline tag must be {FILE_HASH_LENGTH} hex digits, got: {trimmed}"
        ));
    }
    Ok(Some((path.trim().to_string(), tag.to_ascii_uppercase())))
}

fn parse_cut(line: &str) -> Result<Option<Op>, String> {
    let trimmed = line.trim();
    let Some(rest) = strip_ascii_keyword(trimmed, "CUT") else {
        return Ok(None);
    };
    let rest = rest.trim();
    if rest.is_empty() {
        return Err("CUT requires a line range N.=M".into());
    }
    if rest.contains('*') {
        return Err(
            "CUT block ops (N*) are not supported yet; use CUT N.=M with concrete lines".into(),
        );
    }
    if rest.contains('@') {
        return Err("CUT registers (@name) are not supported yet; use plain CUT N.=M".into());
    }
    let (start, end) = parse_range(rest)?;
    Ok(Some(Op::Delete { start, end }))
}

fn parse_put_header(line: &str) -> Result<Option<PutHead>, String> {
    let trimmed = line.trim();
    let Some(rest) = strip_ascii_keyword(trimmed, "PUT") else {
        return Ok(None);
    };
    let rest = rest.trim();
    if rest.is_empty() {
        return Err("PUT requires a locator".into());
    }
    if rest.contains('*') {
        return Err(
            "PUT block ops (N*) are not supported yet; use PUT N.=M: with concrete lines".into(),
        );
    }
    if rest.contains('@') {
        return Err("PUT registers (@name) are not supported yet".into());
    }

    let Some(locator) = rest.strip_suffix(':') else {
        // Bodyless PUT is paste-from-register in full hashline; reject clearly.
        return Err(format!(
            "PUT must end with ':' and take + body rows (got: {trimmed})"
        ));
    };
    let locator = locator.trim();
    if locator.is_empty() {
        return Err(format!("PUT locator is empty: {trimmed}"));
    }

    if let Some(target) = locator.strip_prefix('<') {
        let line = parse_line_number(target.trim(), "PUT <N")?;
        return Ok(Some(PutHead::InsertBefore { line }));
    }
    if let Some(target) = locator.strip_prefix('>') {
        let target = target.trim();
        if target == "$" {
            return Ok(Some(PutHead::InsertAfter { line: None }));
        }
        let line = parse_line_number(target, "PUT >N")?;
        return Ok(Some(PutHead::InsertAfter { line: Some(line) }));
    }
    let (start, end) = parse_range(locator)?;
    Ok(Some(PutHead::Replace { start, end }))
}

fn take_body<'a, I>(lines: &mut std::iter::Peekable<I>) -> Vec<String>
where
    I: Iterator<Item = &'a str>,
{
    let mut body = Vec::new();
    while let Some(candidate) = lines.peek().copied() {
        // Drop only a terminal wire CR so CRLF split rows still match; preserve
        // trailing spaces/tabs in body content (do not trim_end).
        let candidate = candidate.strip_suffix('\r').unwrap_or(candidate);
        if let Some(body_line) = candidate.strip_prefix('+') {
            lines.next();
            body.push(body_line.to_string());
            continue;
        }
        // Stop at the next header/op. Blank lines between ops are skipped by the
        // outer loop; blank lines inside a body must be written as a lone `+`.
        break;
    }
    body
}

fn parse_range(raw: &str) -> Result<(usize, usize), String> {
    let raw = raw.trim();
    if let Some(message) = truncated_range_error(raw) {
        return Err(message);
    }
    // Canonical `N.=M`, plus lenient `N-M` / `N..M` the model sometimes emits.
    let endpoints = raw
        .split_once(".=")
        .or_else(|| raw.split_once(".."))
        .or_else(|| {
            // Single hyphen range: avoid treating negative numbers (unsupported).
            let idx = raw.find('-')?;
            if idx == 0 {
                return None;
            }
            Some((&raw[..idx], &raw[idx + 1..]))
        });
    if let Some((start, end)) = endpoints {
        let start = parse_line_number(start.trim(), "range start")?;
        let end = parse_line_number(end.trim(), "range end")?;
        if end < start {
            return Err(format!("range start {start} is after end {end}"));
        }
        return Ok((start, end));
    }
    // Allow bare `N` as `N.=N` for single-line convenience.
    let line = parse_line_number(raw, "line")?;
    Ok((line, line))
}

/// Detect botched range spellings that fall out of learning `N.=M`.
///
/// Models often emit `PUT N.:` (locator `N.`) or `PUT N.=:` (locator `N.=`).
/// Those must not be accepted as aliases and must not degrade into a generic
/// "not a positive integer" message that hides the stray punctuation.
fn truncated_range_error(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    // `3.=` — range operator present, end missing.
    if raw.ends_with(".=") {
        return Some(format!(
            "hashline range {raw:?} is incomplete; use N.=M (example: \"3.=5\") or a single line N (example: \"3\")"
        ));
    }
    // `.=5` — start missing.
    if raw.starts_with(".=") {
        return Some(format!(
            "hashline range {raw:?} is incomplete; use N.=M (example: \"3.=5\")"
        ));
    }
    // `3.` — almost always a truncated `3.=M` / mistaken `3.:` single-line form.
    // Do not treat `3..5` here; that still has a second `.` and is handled above.
    if raw.ends_with('.') && !raw.contains("..") {
        return Some(format!(
            "hashline range {raw:?} looks like a truncated N.=M; for one line use N (example: PUT 3:) not N. (invalid: PUT 3.:); for a range use N.=M (example: PUT 3.=5:)"
        ));
    }
    None
}

/// Strip a leading ASCII keyword case-insensitively when it is a whole token.
fn strip_ascii_keyword<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let bytes = line.as_bytes();
    let key = keyword.as_bytes();
    if bytes.len() < key.len() {
        return None;
    }
    if !bytes[..key.len()].eq_ignore_ascii_case(key) {
        return None;
    }
    let rest = &line[key.len()..];
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest)
    } else {
        None
    }
}

fn parse_line_number(raw: &str, label: &str) -> Result<usize, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(format!("{label} is empty"));
    }
    // Quote the token so trailing dots/spaces stay visible in tool output.
    let value: usize = raw
        .parse()
        .map_err(|_| format!("{label} must be a positive integer, got {raw:?}"))?;
    if value == 0 {
        return Err(format!("{label} must be >= 1, got {raw:?}"));
    }
    Ok(value)
}

fn strip_bom(line: &str) -> &str {
    line.strip_prefix('\u{feff}').unwrap_or(line)
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
