use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use mermaid_rs_renderer::ir::GitGraphCommitType;
use mermaid_rs_renderer::Direction as MermaidDirection;
use unicode_width::UnicodeWidthStr;

use crate::tui::terminal_graph::{
    draw_seq_text, fit_label, Canvas, CellClass as Cls, GraphStyles, Oversize, MAX_CANVAS_CELLS,
};

use super::MermaidArt;

const TEXT_FLOOR: usize = 12;

#[derive(Clone, Debug)]
pub(super) struct GitGraphModel {
    lanes: Vec<String>,
    rows: Vec<GitRow>,
}

#[derive(Clone, Debug)]
struct GitRow {
    lane: usize,
    glyph: char,
    text: String,
    /// Other lane that forks into or joins this commit.
    connect_from: Option<usize>,
    /// Merge joins share the commit row; branch-offs use an extra row above.
    join_on_row: bool,
}

pub(super) fn from_ir(ir: &mermaid_rs_renderer::Graph) -> Option<GitGraphModel> {
    if ir.gitgraph.commits.is_empty() {
        return None;
    }

    let mut branches = ir.gitgraph.branches.clone();
    branches.sort_by(|left, right| {
        let left_order = left
            .order
            .unwrap_or_else(|| default_branch_order(left.insertion_index));
        let right_order = right
            .order
            .unwrap_or_else(|| default_branch_order(right.insertion_index));
        left_order
            .partial_cmp(&right_order)
            .unwrap_or(Ordering::Equal)
            .then(left.insertion_index.cmp(&right.insertion_index))
    });

    let mut lanes = Vec::new();
    let mut lane_index: HashMap<String, usize> = HashMap::new();
    for branch in &branches {
        if lane_index.contains_key(&branch.name) {
            continue;
        }
        lane_index.insert(branch.name.clone(), lanes.len());
        lanes.push(branch.name.clone());
    }

    let mut commits: Vec<_> = ir.gitgraph.commits.iter().collect();
    commits.sort_by_key(|commit| commit.seq);
    if ir.direction == MermaidDirection::BottomTop {
        commits.reverse();
    }

    let mut id_lane: HashMap<String, usize> = HashMap::new();
    let mut seen_lanes: HashSet<usize> = HashSet::new();
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits {
        let lane = *lane_index.entry(commit.branch.clone()).or_insert_with(|| {
            lanes.push(commit.branch.clone());
            lanes.len() - 1
        });
        let first_on_lane = seen_lanes.insert(lane);
        let kind = commit.custom_type.unwrap_or(commit.commit_type);
        let join_on_row = kind == GitGraphCommitType::Merge;
        let connect_from = incoming_lane(commit, lane, &id_lane, first_on_lane, join_on_row);
        let text = row_text(
            commit,
            first_on_lane,
            lane,
            lanes
                .get(lane)
                .map(String::as_str)
                .unwrap_or(&commit.branch),
            ir.gitgraph.main_branch.as_str(),
        );
        rows.push(GitRow {
            lane,
            glyph: glyph(kind),
            text,
            connect_from,
            join_on_row,
        });
        id_lane.insert(commit.id.clone(), lane);
    }

    let (lanes, rows) = compact_lanes(lanes, rows);
    Some(GitGraphModel { lanes, rows })
}

pub(super) fn layout_gitgraph(
    model: &GitGraphModel,
    styles: &GraphStyles,
    max_width: Option<usize>,
) -> Result<MermaidArt, Oversize> {
    let lane_count = model.lanes.len().max(1);
    let text_x = 2 * lane_count;
    let longest = model
        .rows
        .iter()
        .map(|row| row.text.width())
        .max()
        .unwrap_or(0);
    let mut text_width = longest;
    if let Some(max_width) = max_width {
        let available = max_width.saturating_sub(text_x);
        if available < TEXT_FLOOR && longest > 0 {
            return Err(Oversize::Width);
        }
        text_width = longest.min(available);
    }
    let texts: Vec<String> = model
        .rows
        .iter()
        .map(|row| {
            if text_width == 0 || row.text.width() <= text_width {
                row.text.clone()
            } else {
                fit_label(&row.text, text_width)
            }
        })
        .collect();

    let canvas_w = (text_x + text_width).max(1);
    if max_width.is_some_and(|max_width| canvas_w > max_width) {
        return Err(Oversize::Width);
    }

    let mut paint_y = Vec::with_capacity(model.rows.len());
    let mut y = 0usize;
    for row in &model.rows {
        if row.connect_from.is_some() && !row.join_on_row {
            y += 1;
        }
        paint_y.push(y);
        y += 1;
    }
    let canvas_h = y.max(1);
    if canvas_w.saturating_mul(canvas_h) > MAX_CANVAS_CELLS {
        return Err(Oversize::Cells);
    }

    let mut canvas = Canvas::new(canvas_w, canvas_h);
    let mut lane_min = vec![usize::MAX; lane_count];
    let mut lane_max = vec![0usize; lane_count];
    let touch = |lane_min: &mut [usize], lane_max: &mut [usize], lane: usize, row_y: usize| {
        if lane >= lane_min.len() {
            return;
        }
        lane_min[lane] = lane_min[lane].min(row_y);
        lane_max[lane] = lane_max[lane].max(row_y);
    };

    for (index, row) in model.rows.iter().enumerate() {
        let commit_y = paint_y[index];
        touch(&mut lane_min, &mut lane_max, row.lane, commit_y);
        if let Some(from) = row.connect_from {
            let connector_y = if row.join_on_row {
                commit_y
            } else {
                commit_y.saturating_sub(1)
            };
            touch(&mut lane_min, &mut lane_max, from, connector_y);
            touch(&mut lane_min, &mut lane_max, row.lane, connector_y);
        }
    }
    for (lane, (&min_y, &max_y)) in lane_min.iter().zip(&lane_max).enumerate() {
        if min_y == usize::MAX {
            continue;
        }
        canvas.seg_v(lane_x(lane), min_y, max_y);
    }

    for (index, row) in model.rows.iter().enumerate() {
        let commit_y = paint_y[index];
        if let Some(from) = row.connect_from {
            let connector_y = if row.join_on_row {
                commit_y
            } else {
                commit_y.saturating_sub(1)
            };
            canvas.seg_h(connector_y, lane_x(from), lane_x(row.lane));
        }
        canvas.set(lane_x(row.lane), commit_y, row.glyph, Cls::Border);
        if !texts[index].is_empty() {
            draw_seq_text(&mut canvas, &texts[index], text_x, commit_y, Cls::Text);
        }
    }

    canvas.finalize_mask();
    let (styled_lines, plain_lines) = canvas.to_lines(styles);
    Ok(MermaidArt {
        styled_lines,
        plain_lines,
    })
}

fn lane_x(lane: usize) -> usize {
    2 * lane
}

fn glyph(kind: GitGraphCommitType) -> char {
    match kind {
        GitGraphCommitType::Merge => '◉',
        GitGraphCommitType::Highlight => '◆',
        GitGraphCommitType::Reverse => '⊘',
        GitGraphCommitType::Normal | GitGraphCommitType::CherryPick => '●',
    }
}

fn incoming_lane(
    commit: &mermaid_rs_renderer::ir::GitGraphCommit,
    lane: usize,
    id_lane: &HashMap<String, usize>,
    first_on_lane: bool,
    join_on_row: bool,
) -> Option<usize> {
    let other = |parent: &String| {
        id_lane
            .get(parent)
            .copied()
            .filter(|&parent_lane| parent_lane != lane)
    };
    if join_on_row {
        return commit.parents.iter().rev().find_map(other);
    }
    if first_on_lane {
        return commit.parents.first().and_then(other);
    }
    None
}

fn row_text(
    commit: &mermaid_rs_renderer::ir::GitGraphCommit,
    first_on_lane: bool,
    lane: usize,
    lane_name: &str,
    main_branch: &str,
) -> String {
    let mut text = if let Some(message) = commit
        .message
        .as_deref()
        .map(str::trim)
        .filter(|message| !message.is_empty())
    {
        message.to_string()
    } else if commit.custom_id {
        commit.id.clone()
    } else {
        String::new()
    };
    if first_on_lane && (lane != 0 || lane_name != main_branch) {
        if text.is_empty() {
            text = format!("({lane_name})");
        } else {
            text.push_str(" (");
            text.push_str(lane_name);
            text.push(')');
        }
    }
    if !commit.tags.is_empty() {
        text.push_str(" [");
        text.push_str(&commit.tags.join(" "));
        text.push(']');
    }
    text
}

fn compact_lanes(lanes: Vec<String>, rows: Vec<GitRow>) -> (Vec<String>, Vec<GitRow>) {
    let mut used = vec![false; lanes.len()];
    for row in &rows {
        if let Some(used_lane) = used.get_mut(row.lane) {
            *used_lane = true;
        }
        if let Some(used_lane) = row.connect_from.and_then(|from| used.get_mut(from)) {
            *used_lane = true;
        }
    }
    if used.iter().all(|&lane| lane) {
        return (lanes, rows);
    }
    let mut map = vec![0usize; lanes.len()];
    let mut kept = Vec::new();
    for (index, name) in lanes.into_iter().enumerate() {
        if used.get(index).copied().unwrap_or(false) {
            map[index] = kept.len();
            kept.push(name);
        }
    }
    let rows = rows
        .into_iter()
        .map(|row| GitRow {
            lane: map.get(row.lane).copied().unwrap_or(0),
            connect_from: row.connect_from.and_then(|from| map.get(from).copied()),
            ..row
        })
        .collect();
    (kept, rows)
}

fn default_branch_order(index: usize) -> f32 {
    if index == 0 {
        return 0.0;
    }
    let mut denom = 1.0f32;
    let mut value = index;
    while value > 0 {
        denom *= 10.0;
        value /= 10;
    }
    index as f32 / denom
}
