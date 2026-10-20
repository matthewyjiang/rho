//! Source checks for malformed input that mermaid-rs-renderer accepts.
//!
//! The upstream parser skips lines it cannot read, implicitly closes open
//! blocks, and invents branches on checkout, so some invalid sources still
//! yield a plausible graph. These checks reject those sources so they take
//! the malformed fallback path instead of painting misleading art.
//!
//! Each check reads lines the way the upstream parser does (front matter,
//! comments, and block closers included), so it only rejects what that parser
//! papered over and never valid input the parser read correctly.

use std::collections::HashSet;

use mermaid_rs_renderer::Graph;

use super::policy::PaintedKind;

/// Why a parsed source is not a well-formed diagram.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Defect {
    /// A `{` block is still open at the end of the source. Later lines can
    /// close it, so a streaming prefix may still paint.
    UnclosedBlock,
    /// No continuation can repair the source.
    Invalid,
    /// Valid Mermaid the upstream parser reads wrongly, so its graph would
    /// paint misleading art. Declined like an unsupported construct.
    Misread,
}

/// Checks `source`, already parsed into `ir`, for defects the parser let
/// through. Line checks see complete lines only: the open-fence path never
/// hands a partial last line to the renderer.
pub(super) fn check(source: &str, kind: PaintedKind, ir: &Graph) -> Result<(), Defect> {
    let lines = code_lines(source);
    match kind {
        PaintedKind::Flowchart => flowchart_shapes_close(&lines),
        PaintedKind::State => blocks_close(&lines, state_block_opener),
        PaintedKind::Class => blocks_close(&lines, class_block_opener),
        PaintedKind::Er => blocks_close(&lines, er_block_opener),
        PaintedKind::GitGraph => branch_refs_resolve(source, &lines, &ir.gitgraph.main_branch),
        // The upstream strict validator already balances sequence frames;
        // gantt and mindmap have no paired syntax the parser closes silently.
        PaintedKind::Sequence | PaintedKind::Gantt | PaintedKind::Mindmap => Ok(()),
    }
}

/// The lines the upstream parser reads (its `preprocess_input`): trimmed and
/// non-empty, outside `---` front matter, without `%%` comment lines, and with
/// trailing `%%` comments cut outside quotes.
fn code_lines(source: &str) -> Vec<&str> {
    let mut in_front_matter = false;
    let mut lines = Vec::new();
    for line in source.lines().map(str::trim) {
        if line == "---" {
            in_front_matter = !in_front_matter;
            continue;
        }
        if in_front_matter || line.starts_with("%%") {
            continue;
        }
        let code = strip_trailing_comment(line);
        if !code.is_empty() {
            lines.push(code);
        }
    }
    lines
}

/// Cuts a `%%` comment that starts outside `"` or `'` quotes.
fn strip_trailing_comment(line: &str) -> &str {
    let mut quote = None;
    let mut chars = line.char_indices().peekable();
    while let Some((index, character)) = chars.next() {
        match quote {
            Some(open) if character == open => quote = None,
            Some(_) => {}
            None if matches!(character, '"' | '\'') => quote = Some(character),
            None if character == '%' && chars.peek().is_some_and(|&(_, next)| next == '%') => {
                return line[..index].trim_end();
            }
            None => {}
        }
    }
    line
}

/// True when every node shape opened on a flowchart line closes on it.
///
/// A shape opens where `[`, `(`, `{`, or the asymmetric `>` follows a node
/// id; inside a shape every opener nests, so `A[(db)]` and `A[f (x)]` close.
/// Quoted strings and `|edge labels|` are opaque. Unmatched closers inside a
/// label stay text, which keeps lenient labels such as `A[x)]` painting.
/// Requires every flowchart node shape to close on its line. Like upstream,
/// skips free-text `accTitle`, `accDescr`, and `title` lines. Upstream reads
/// the body of an `accDescr {` block that continues past its line as nodes,
/// so that form is misread; `accDescr: text {` and `accDescr { text }` are
/// single-line descriptions.
fn flowchart_shapes_close(lines: &[&str]) -> Result<(), Defect> {
    for line in lines {
        let opens_block = line
            .strip_prefix("accDescr")
            .is_some_and(|rest| rest.trim_start().starts_with('{') && !rest.contains('}'));
        if opens_block {
            return Err(Defect::Misread);
        }
        let free_text = ["accTitle", "accDescr", "title "]
            .iter()
            .any(|prefix| line.starts_with(prefix));
        if !free_text && !node_shapes_close(line) {
            return Err(Defect::Invalid);
        }
    }
    Ok(())
}

fn node_shapes_close(line: &str) -> bool {
    let mut expected: Vec<char> = Vec::new();
    let mut previous = None;
    let mut chars = line.chars();
    while let Some(character) = chars.next() {
        let after_id =
            previous.is_some_and(|c: char| c.is_alphanumeric() || matches!(c, '_' | '@'));
        match character {
            // An HTML tag such as `<br>` in `-- text -->` edge text, or an
            // arrowhead's `<`: skip to its `>`, which is not a shape opener.
            '<' if expected.is_empty() => {
                chars.any(|c| c == '>');
            }
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

/// How an open `{` block ends, matching the upstream parser for its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlockClose {
    /// State composites and class namespaces: a line that is exactly `}`.
    /// These blocks nest.
    LoneBrace,
    /// Class and entity bodies: the first `}` on any later line, so
    /// `+name }` closes too. Bodies hold members, never nested blocks.
    AnyBrace,
}

/// `state X {` opens a composite, as upstream `parse_state_container_header`.
fn state_block_opener(line: &str) -> Option<BlockClose> {
    (line.starts_with("state ") && line.contains('{')).then_some(BlockClose::LoneBrace)
}

/// `class X {` opens a member body. Upstream ignores `namespace` lines, but
/// Mermaid requires their `}`, so a namespace opens a nesting block.
fn class_block_opener(line: &str) -> Option<BlockClose> {
    if !line.contains('{') {
        None
    } else if line.starts_with("class ") {
        Some(BlockClose::AnyBrace)
    } else if line.starts_with("namespace ") {
        Some(BlockClose::LoneBrace)
    } else {
        None
    }
}

/// Any non-relationship line with `{` opens an entity body. Like upstream
/// `parse_er_relation_line`, a relationship has a `--` or `..` connector
/// before its first `:`, so `CUSTOMER ||--o{ ORDER : places` opens nothing.
fn er_block_opener(line: &str) -> Option<BlockClose> {
    let relation = line.split_once(':').map_or(line, |(before, _)| before);
    let is_relationship = relation.contains("--") || relation.contains("..");
    (line.contains('{') && !is_relationship).then_some(BlockClose::AnyBrace)
}

/// Balances `{` blocks. An opener whose own tail already holds `}`, such as
/// `class A { +x }`, opens nothing; a `}` with no open block is invalid.
fn blocks_close(lines: &[&str], opener: fn(&str) -> Option<BlockClose>) -> Result<(), Defect> {
    let mut open: Vec<BlockClose> = Vec::new();
    for &line in lines {
        match open.last() {
            Some(BlockClose::AnyBrace) => {
                if line.contains('}') {
                    open.pop();
                }
                continue;
            }
            Some(BlockClose::LoneBrace) | None if line == "}" => {
                open.pop().ok_or(Defect::Invalid)?;
                continue;
            }
            Some(BlockClose::LoneBrace) | None => {}
        }
        if let Some(close) = opener(line) {
            let tail = line.split_once('{').map_or("", |(_, tail)| tail);
            if !tail.contains('}') {
                open.push(close);
            }
        }
    }
    if open.is_empty() {
        Ok(())
    } else {
        Err(Defect::UnclosedBlock)
    }
}

/// Requires `checkout`, `switch`, and `merge` to name a branch created
/// earlier. Mermaid rejects such references; the upstream parser instead
/// invents an empty branch lane.
///
/// Upstream names a branch with the whole rest of its line and always calls
/// the main branch `main`, so attributes after a branch name (`branch feat
/// order: 1`, `merge feat tag: "v1"`) and `mainBranchName` are misread: they
/// would paint a parentless lane or a merge without its second parent.
fn branch_refs_resolve(source: &str, lines: &[&str], main_branch: &str) -> Result<(), Defect> {
    // Config only comes from `%%{init: ...}%%` directives; comments and
    // commit messages may mention the key freely.
    if source
        .lines()
        .any(|line| line.trim_start().starts_with("%%{") && line.contains("mainBranchName"))
    {
        return Err(Defect::Misread);
    }
    let mut branches = HashSet::from([main_branch]);
    for line in lines {
        let Some((keyword, name)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let name = name.trim();
        let declares = keyword.eq_ignore_ascii_case("branch");
        let references = ["checkout", "switch", "merge"]
            .iter()
            .any(|reference| keyword.eq_ignore_ascii_case(reference));
        if !declares && !references {
            continue;
        }
        // Every gitGraph attribute is a `key: value` pair.
        if name.contains(':') {
            return Err(Defect::Misread);
        }
        if declares {
            branches.insert(name);
        } else if !branches.contains(name) {
            return Err(Defect::Invalid);
        }
    }
    Ok(())
}
