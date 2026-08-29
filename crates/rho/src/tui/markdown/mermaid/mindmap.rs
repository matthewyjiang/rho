use std::collections::{HashMap, HashSet};

use unicode_width::UnicodeWidthStr;

use crate::tui::terminal_graph::{wrap_label, GraphStyles, Oversize};

use super::MermaidArt;

const TEXT_FLOOR: usize = 12;
const INDENT: usize = 3;
const WRAP_LINES: usize = 4;

#[derive(Clone, Debug)]
pub(super) struct MindmapModel {
    pub(super) entries: Vec<MindmapEntry>,
}

#[derive(Clone, Debug)]
pub(super) struct MindmapEntry {
    pub(super) depth: usize,
    pub(super) label: String,
    pub(super) last_child: bool,
    pub(super) ancestor_last: Vec<bool>,
}

pub(super) fn from_ir(ir: &mermaid_rs_renderer::Graph) -> Option<MindmapModel> {
    if ir.mindmap.nodes.is_empty() {
        return None;
    }
    let mut by_id: HashMap<&str, usize> = HashMap::new();
    for (index, node) in ir.mindmap.nodes.iter().enumerate() {
        by_id.insert(node.id.as_str(), index);
    }
    let root = ir
        .mindmap
        .root_id
        .as_deref()
        .and_then(|id| by_id.get(id).copied())
        .unwrap_or(0);
    let mut entries = Vec::new();
    let mut visited = HashSet::new();
    walk(
        &mut WalkCtx {
            ir,
            by_id: &by_id,
            visited: &mut visited,
            entries: &mut entries,
        },
        root,
        0,
        true,
        &[],
    );
    if entries.is_empty() {
        return None;
    }
    Some(MindmapModel { entries })
}

pub(super) fn complexity(ir: &mermaid_rs_renderer::Graph) -> (usize, usize, usize, usize) {
    (ir.mindmap.nodes.len(), 0, 0, 0)
}

pub(super) fn layout_mindmap(
    model: &MindmapModel,
    styles: &GraphStyles,
    max_width: Option<usize>,
) -> Result<MermaidArt, Oversize> {
    if max_width.is_some_and(|width| width < TEXT_FLOOR + INDENT) {
        return Err(Oversize::Width);
    }
    let max_depth = max_width.map(|width| width.saturating_sub(TEXT_FLOOR) / INDENT);
    let mut lines = Vec::new();
    for entry in &model.entries {
        let (depth, truncated) = match max_depth {
            Some(max_depth) if entry.depth > max_depth => (max_depth, true),
            _ => (entry.depth, false),
        };
        let prefix = tree_prefix(entry, depth, truncated, /*continuation*/ false);
        let wrap_width = match max_width {
            Some(width) => width.saturating_sub(prefix.width()).max(1),
            None => entry.label.width().max(1),
        };
        let wrapped = if max_width.is_some() {
            wrap_label(&entry.label, wrap_width, WRAP_LINES)
        } else {
            vec![entry.label.clone()]
        };
        for (index, line) in wrapped.iter().enumerate() {
            if index == 0 {
                lines.push(format!("{prefix}{line}"));
            } else {
                let cont = tree_prefix(entry, depth, truncated, /*continuation*/ true);
                lines.push(format!("{cont}{line}"));
            }
        }
    }
    if max_width.is_some_and(|width| lines.iter().any(|line| line.width() > width)) {
        return Err(Oversize::Width);
    }
    Ok(super::art_from_plain(lines, styles))
}

struct WalkCtx<'a> {
    ir: &'a mermaid_rs_renderer::Graph,
    by_id: &'a HashMap<&'a str, usize>,
    visited: &'a mut HashSet<usize>,
    entries: &'a mut Vec<MindmapEntry>,
}

fn walk(
    ctx: &mut WalkCtx<'_>,
    index: usize,
    depth: usize,
    last_child: bool,
    ancestor_last: &[bool],
) {
    if !ctx.visited.insert(index) {
        return;
    }
    let Some(node) = ctx.ir.mindmap.nodes.get(index) else {
        return;
    };
    ctx.entries.push(MindmapEntry {
        depth,
        label: node.label.clone(),
        last_child,
        ancestor_last: ancestor_last.to_vec(),
    });
    let children: Vec<usize> = node
        .children
        .iter()
        .filter_map(|id| ctx.by_id.get(id.as_str()).copied())
        .collect();
    let mut next_ancestors = ancestor_last.to_vec();
    next_ancestors.push(last_child);
    let last = children.len().saturating_sub(1);
    for (child_index, child) in children.into_iter().enumerate() {
        walk(ctx, child, depth + 1, child_index == last, &next_ancestors);
    }
}

fn tree_prefix(entry: &MindmapEntry, depth: usize, truncated: bool, continuation: bool) -> String {
    if depth == 0 {
        return String::new();
    }
    let mut prefix = String::new();
    for last in entry.ancestor_last.iter().take(depth.saturating_sub(1)) {
        prefix.push_str(if *last { "   " } else { "│  " });
    }
    if truncated {
        prefix.push_str(if continuation { "   " } else { "… " });
        return prefix;
    }
    if continuation {
        prefix.push_str(if entry.last_child { "   " } else { "│  " });
    } else if entry.last_child {
        prefix.push_str("└─ ");
    } else {
        prefix.push_str("├─ ");
    }
    prefix
}
