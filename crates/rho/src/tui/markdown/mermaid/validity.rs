//! Source checks for malformed input that mermaid-rs-renderer accepts.
//!
//! The upstream parser skips lines it cannot read, implicitly closes open
//! blocks, and invents branches on checkout, so some invalid sources still
//! yield a plausible graph. These checks reject those sources so they take
//! the malformed fallback path instead of painting misleading art.

use std::collections::HashSet;

use mermaid_rs_renderer::{DiagramKind, Graph};

/// Why a parsed source is not a well-formed diagram.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Defect {
    /// A `{` block is still open at the end of the source. Later lines can
    /// close it, so a streaming prefix may still paint.
    UnclosedBlock,
    /// No continuation can repair the source.
    Invalid,
}

/// Checks `source`, already parsed into `ir`, for defects the parser let
/// through. Line checks see complete lines only: the open-fence path never
/// hands a partial last line to the renderer.
pub(super) fn check(source: &str, ir: &Graph) -> Result<(), Defect> {
    let lines = code_lines(source);
    match ir.kind {
        DiagramKind::Flowchart => {
            if lines.into_iter().all(node_shapes_close) {
                Ok(())
            } else {
                Err(Defect::Invalid)
            }
        }
        DiagramKind::State => blocks_close(&lines, |line| line.starts_with("state ")),
        DiagramKind::Class => blocks_close(&lines, |line| {
            line.starts_with("class ") || line.starts_with("namespace ")
        }),
        // Relationship lines always carry a `: label`; entity blocks never do.
        DiagramKind::Er => blocks_close(&lines, |line| !line.contains(':')),
        DiagramKind::GitGraph => branch_refs_resolve(&lines, &ir.gitgraph.main_branch),
        // The upstream strict validator already balances sequence frames;
        // gantt and mindmap have no paired syntax the parser closes silently.
        DiagramKind::Sequence | DiagramKind::Gantt | DiagramKind::Mindmap => Ok(()),
        // Never painted: the paint policy declines these before any check.
        DiagramKind::Pie
        | DiagramKind::Journey
        | DiagramKind::Timeline
        | DiagramKind::Requirement
        | DiagramKind::C4
        | DiagramKind::Sankey
        | DiagramKind::Quadrant
        | DiagramKind::ZenUML
        | DiagramKind::Block
        | DiagramKind::Packet
        | DiagramKind::Kanban
        | DiagramKind::Architecture
        | DiagramKind::Radar
        | DiagramKind::Treemap
        | DiagramKind::XYChart => Ok(()),
    }
}

/// Trimmed non-empty lines with `%%` comments removed.
fn code_lines(source: &str) -> Vec<&str> {
    source
        .lines()
        .map(|line| line.split_once("%%").map_or(line, |(code, _)| code).trim())
        .filter(|line| !line.is_empty())
        .collect()
}

/// True when every node shape opened on a flowchart line closes on it.
///
/// A shape opens where `[`, `(`, `{`, or the asymmetric `>` follows a node
/// id; inside a shape every opener nests, so `A[(db)]` and `A[f (x)]` close.
/// Quoted strings and `|edge labels|` are opaque. Unmatched closers inside a
/// label stay text, which keeps lenient labels such as `A[x)]` painting.
fn node_shapes_close(line: &str) -> bool {
    let mut expected: Vec<char> = Vec::new();
    let mut previous = None;
    let mut chars = line.chars();
    while let Some(character) = chars.next() {
        let after_id =
            previous.is_some_and(|c: char| c.is_alphanumeric() || matches!(c, '_' | '@'));
        match character {
            // Skip to the closing quote or pipe; none means it never closes.
            '"' => {
                if !chars.any(|c| c == '"') {
                    return false;
                }
            }
            '|' if expected.is_empty() => {
                if !chars.any(|c| c == '|') {
                    return false;
                }
            }
            '[' | '(' | '{' if after_id || !expected.is_empty() => {
                expected.push(match character {
                    '[' => ']',
                    '(' => ')',
                    _ => '}',
                });
            }
            '>' if after_id && expected.is_empty() => expected.push(']'),
            ']' | ')' | '}' if expected.last() == Some(&character) => {
                expected.pop();
            }
            _ => {}
        }
        previous = Some(character);
    }
    expected.is_empty()
}

/// Balances `{` blocks: a line ending in `{` that `opens` accepts opens one,
/// and a lone `}` closes one. Single-line `X { ... }` bodies are neither.
fn blocks_close(lines: &[&str], opens: impl Fn(&str) -> bool) -> Result<(), Defect> {
    let mut depth = 0usize;
    for &line in lines {
        if line == "}" {
            depth = depth.checked_sub(1).ok_or(Defect::Invalid)?;
        } else if line.ends_with('{') && opens(line) {
            depth += 1;
        }
    }
    if depth == 0 {
        Ok(())
    } else {
        Err(Defect::UnclosedBlock)
    }
}

/// Requires `checkout`, `switch`, and `merge` to name a branch created
/// earlier. Mermaid rejects such references; the upstream parser instead
/// invents an empty branch lane. Names compare by their first token with
/// quotes trimmed, so `branch feat order: 1` declares `feat`.
fn branch_refs_resolve(lines: &[&str], main_branch: &str) -> Result<(), Defect> {
    let mut branches = HashSet::from([main_branch]);
    for line in lines {
        let mut words = line.split_whitespace();
        let (Some(keyword), Some(name)) = (words.next(), words.next()) else {
            continue;
        };
        let name = name.trim_matches('"');
        if keyword.eq_ignore_ascii_case("branch") {
            branches.insert(name);
        } else if ["checkout", "switch", "merge"]
            .iter()
            .any(|reference| keyword.eq_ignore_ascii_case(reference))
            && !branches.contains(name)
        {
            return Err(Defect::Invalid);
        }
    }
    Ok(())
}
