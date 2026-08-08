// Adapted from Grok Build's terminal Mermaid renderer:
// https://github.com/xai-org/grok-build/blob/b189869b7755d2b482969acf6c92da3ecfeffd36/crates/codegen/xai-grok-markdown/src/mermaid.rs
// Copyright 2023-2026 SpaceXAI. Licensed under Apache-2.0.
use std::collections::BTreeMap;

use unicode_width::UnicodeWidthStr;

use super::flow::{NodeSizes, Placed};
use super::ordering::assign_positions;
use super::Graph;
use crate::tui::terminal_graph::painter::{GAP_X, GAP_Y, MAX_LABEL};

#[derive(Clone, Copy)]
enum CrossAxisAlignment {
    Exact,
    Near,
}

#[derive(Clone, Copy)]
enum LaneAxis {
    Vertical,
    Horizontal,
}

impl CrossAxisAlignment {
    fn has_jog(self, from: usize, to: usize) -> bool {
        match self {
            Self::Exact => from != to,
            Self::Near => !near_aligned(from, to),
        }
    }
}

fn near_aligned(from: usize, to: usize) -> bool {
    from.abs_diff(to) <= 1
}

/// One forward edge inside a rank band, in bus track terms.
struct BusSpan {
    start: usize,
    end: usize,
    from: usize,
    to: usize,
    index: usize,
    /// Straight drops need no bus row but still join their target's group so
    /// complete fan-in layers merge onto one shared row.
    jogs: bool,
}

fn bus_spans(
    graph: &Graph,
    ranks: &[usize],
    centers: &[usize],
    r: usize,
    alignment: CrossAxisAlignment,
) -> Vec<BusSpan> {
    graph
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| e.from != e.to && ranks[e.from] == r && ranks[e.to] == r + 1)
        .map(|(i, e)| BusSpan {
            start: centers[e.from].min(centers[e.to]),
            end: centers[e.from].max(centers[e.to]),
            from: e.from,
            to: e.to,
            index: i,
            jogs: alignment.has_jog(centers[e.from], centers[e.to]),
        })
        .collect()
}

/// A forward edge that skips at least one rank. It leaves its source's rank
/// through the shared right lane and re-enters from above its target's rank.
struct SkipEdge {
    index: usize,
    from: usize,
    to: usize,
    source_center: usize,
    source_rank: usize,
    target_rank: usize,
}

fn skip_edges(graph: &Graph, ranks: &[usize], centers: &[usize]) -> Vec<SkipEdge> {
    graph
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| e.from != e.to && ranks[e.to] > ranks[e.from] + 1)
        .map(|(index, e)| SkipEdge {
            index,
            from: e.from,
            to: e.to,
            source_center: centers[e.from],
            source_rank: ranks[e.from],
            target_rank: ranks[e.to],
        })
        .collect()
}

fn td_source_anchors(graph: &Graph, ranks: &[usize], centers: &[usize]) -> Vec<usize> {
    let mut source_anchors = centers.to_vec();
    // Rank spacing guarantees at most one distinct near-aligned child, so all
    // forward siblings resolve to one stable exit for their source.
    for edge in &graph.edges {
        let source_x = centers[edge.from];
        let target_x = centers[edge.to];
        if edge.from != edge.to
            && ranks[edge.to] == ranks[edge.from] + 1
            && near_aligned(source_x, target_x)
        {
            source_anchors[edge.from] = target_x;
        }
    }
    source_anchors
}

fn lane_spans(
    graph: &Graph,
    ranks: &[usize],
    placed: &[Placed],
    axis: LaneAxis,
) -> Vec<(usize, usize, usize, usize, usize)> {
    graph
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| e.from != e.to && ranks[e.to] != ranks[e.from] + 1)
        .map(|(i, e)| {
            let (pf, pt) = (&placed[e.from], &placed[e.to]);
            let (a, b) = match axis {
                LaneAxis::Vertical => (pf.cy.min(pt.cy), pf.cy.max(pt.cy)),
                LaneAxis::Horizontal => (pf.cx.min(pt.cx), pf.cx.max(pt.cx)),
            };
            (a, b, e.from, e.to, i)
        })
        .collect()
}

pub(super) fn place_td(
    ranks: &[usize],
    max_rank: usize,
    by_rank: &[Vec<usize>],
    sizes: &NodeSizes,
    graph: &Graph,
    placed: &mut [Placed],
) -> RoutePlan {
    let centers = assign_positions(by_rank, &sizes.lay_w, GAP_X, &graph.edges, ranks);
    let source_anchors = td_source_anchors(graph, ranks, &centers);
    let skips = skip_edges(graph, ranks, &centers);

    let mut edge_bus = vec![0usize; graph.edges.len()];
    let mut bus_tracks = vec![0usize; max_rank + 1];
    for (r, tracks) in bus_tracks.iter_mut().enumerate().take(max_rank) {
        let spans = bus_spans(graph, ranks, &centers, r, CrossAxisAlignment::Near);
        let exits: Vec<&SkipEdge> = skips.iter().filter(|skip| skip.source_rank == r).collect();
        if spans.is_empty() && exits.is_empty() {
            continue;
        }
        let (assigned, count) = assign_bus_tracks(&spans, &exits);
        for (idx, slot) in assigned {
            edge_bus[idx] = slot;
        }
        *tracks = count;
    }

    // Rank-skipping edges approach their target from above through reserved
    // rows in the gap over the target's rank, one row per distinct target.
    let mut approach_targets: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for skip in &skips {
        approach_targets
            .entry(skip.target_rank)
            .or_default()
            .push(skip.to);
    }
    let mut approach_tracks = vec![0usize; max_rank + 1];
    for (&rank, targets) in &mut approach_targets {
        targets.sort_unstable();
        targets.dedup();
        approach_tracks[rank] = targets.len();
    }
    let approach_slots: Vec<usize> = skips
        .iter()
        .map(|skip| {
            approach_targets[&skip.target_rank]
                .iter()
                .position(|&target| target == skip.to)
                .expect("skip targets cover every rank-skipping edge")
        })
        .collect();

    let rank_h: Vec<usize> = by_rank
        .iter()
        .map(|row| {
            row.iter()
                .map(|&i| sizes.box_h[i] + sizes.extra_h[i])
                .max()
                .unwrap_or(3)
        })
        .collect();
    let mut rank_y = vec![0usize; max_rank + 1];
    for r in 1..=max_rank {
        let gap = GAP_Y.max(bus_tracks[r - 1] + approach_tracks[r] + 1);
        rank_y[r] = rank_y[r - 1] + rank_h[r - 1] + gap;
    }
    let canvas_h = rank_y[max_rank] + rank_h[max_rank];
    let band_end: Vec<usize> = (0..=max_rank).map(|r| rank_y[r] + rank_h[r]).collect();

    let mut diagram_w = 1;
    for (r, row) in by_rank.iter().enumerate() {
        for &idx in row {
            let w = sizes.box_w[idx];
            let h = sizes.box_h[idx];
            let cx = centers[idx];
            let x = cx.saturating_sub(w / 2);
            let y = rank_y[r] + (rank_h[r] - h - sizes.extra_h[idx]) / 2;
            placed[idx] = Placed {
                x,
                y,
                w,
                h,
                cx,
                cy: y + h / 2,
                rank: r,
            };
            diagram_w = diagram_w.max(x + w);
            if sizes.extra_h[idx] > 0 && sizes.self_label_w[idx] > 0 {
                diagram_w = diagram_w.max(x + w + 2 + sizes.self_label_w[idx]);
            }
        }
    }
    let mut content_w = diagram_w;
    for e in &graph.edges {
        if e.from == e.to {
            continue;
        }
        if let Some(label) = &e.label {
            let lw = label.width().min(MAX_LABEL);
            if ranks[e.to] == ranks[e.from] + 1 {
                content_w = content_w.max(placed[e.to].cx + 2 + lw);
            } else {
                content_w = content_w.max(diagram_w + lw + 1);
            }
        }
    }

    let mut edge_lane = vec![0usize; graph.edges.len()];
    let lanes = lane_spans(graph, ranks, placed, LaneAxis::Vertical);
    let (canvas_w, lane_base) = if lanes.is_empty() {
        (content_w, 0)
    } else {
        let (assigned, count) = assign_tracks(&lanes);
        for (idx, slot) in assigned {
            edge_lane[idx] = slot;
        }
        (content_w + 1 + count, content_w + 1)
    };

    let mut edge_approach = vec![0usize; graph.edges.len()];
    for (skip, slot) in skips.iter().zip(&approach_slots) {
        edge_approach[skip.index] =
            band_end[skip.target_rank - 1] + bus_tracks[skip.target_rank - 1] + slot;
    }

    RoutePlan {
        canvas: (canvas_w, canvas_h),
        band_end,
        edge_bus,
        source_anchors,
        lane_base,
        edge_lane,
        edge_approach,
    }
}

pub(super) fn place_lr(
    ranks: &[usize],
    max_rank: usize,
    by_rank: &[Vec<usize>],
    sizes: &NodeSizes,
    graph: &Graph,
    placed: &mut [Placed],
) -> RoutePlan {
    let col_w: Vec<usize> = by_rank
        .iter()
        .map(|row| row.iter().map(|&i| sizes.box_w[i]).max().unwrap_or(0))
        .collect();

    let max_label = graph
        .edges
        .iter()
        .filter(|e| e.from == e.to || ranks[e.to] == ranks[e.from] + 1)
        .filter_map(|e| e.label.as_ref().map(|l| l.width().min(MAX_LABEL)))
        .max()
        .unwrap_or(0);
    let base_gap = (GAP_X + 1).max(max_label + 3);

    let centers = assign_positions(by_rank, &sizes.lay_h, 1, &graph.edges, ranks);

    let mut edge_bus = vec![0usize; graph.edges.len()];
    let mut bus_tracks = vec![0usize; max_rank + 1];
    for (r, tracks) in bus_tracks.iter_mut().enumerate().take(max_rank) {
        let spans = bus_spans(graph, ranks, &centers, r, CrossAxisAlignment::Exact);
        if spans.is_empty() {
            continue;
        }
        let (assigned, count) = assign_bus_tracks(&spans, &[]);
        for (idx, slot) in assigned {
            edge_bus[idx] = slot;
        }
        *tracks = count;
    }

    let mut rank_x = vec![0usize; max_rank + 1];
    for r in 1..=max_rank {
        let gap = base_gap.max(bus_tracks[r - 1] + 1);
        rank_x[r] = rank_x[r - 1] + col_w[r - 1] + gap;
    }
    let canvas_w = rank_x[max_rank]
        + col_w[max_rank]
        + by_rank[max_rank]
            .iter()
            .filter(|&&i| sizes.extra_h[i] > 0 && sizes.self_label_w[i] > 0)
            .map(|&i| 2 + sizes.self_label_w[i])
            .max()
            .unwrap_or(0);
    let band_end: Vec<usize> = (0..=max_rank).map(|r| rank_x[r] + col_w[r]).collect();

    let mut diagram_h = 1;
    for (r, row) in by_rank.iter().enumerate() {
        let x = rank_x[r];
        for &idx in row {
            let w = sizes.box_w[idx];
            let h = sizes.box_h[idx];
            let cy = centers[idx];
            let y = cy.saturating_sub((h + sizes.extra_h[idx]) / 2);
            placed[idx] = Placed {
                x,
                y,
                w,
                h,
                cx: x + w / 2,
                cy: y + h / 2,
                rank: r,
            };
            diagram_h = diagram_h.max(y + h + sizes.extra_h[idx]);
        }
    }
    let source_anchors = placed.iter().map(|node| node.cy).collect();

    let mut edge_lane = vec![0usize; graph.edges.len()];
    let lanes = lane_spans(graph, ranks, placed, LaneAxis::Horizontal);
    let (canvas_h, lane_base) = if lanes.is_empty() {
        (diagram_h, 0)
    } else {
        let (assigned, count) = assign_tracks(&lanes);
        for (idx, slot) in assigned {
            edge_lane[idx] = slot;
        }
        (diagram_h + 1 + count, diagram_h + 1)
    };

    RoutePlan {
        canvas: (canvas_w, canvas_h),
        band_end,
        edge_bus,
        source_anchors,
        lane_base,
        edge_lane,
        edge_approach: vec![0; graph.edges.len()],
    }
}

pub(super) struct RoutePlan {
    pub(super) canvas: (usize, usize),
    pub(super) band_end: Vec<usize>,
    pub(super) edge_bus: Vec<usize>,
    pub(super) source_anchors: Vec<usize>,
    pub(super) lane_base: usize,
    pub(super) edge_lane: Vec<usize>,
    /// Absolute approach row above the target rank for rank-skipping top-down
    /// edges; zero for every other edge.
    pub(super) edge_approach: Vec<usize>,
}

fn assign_tracks(spans: &[(usize, usize, usize, usize, usize)]) -> (Vec<(usize, usize)>, usize) {
    let mut sorted = spans.to_vec();
    sorted.sort_unstable();
    let mut tracks: Vec<Vec<(usize, usize, usize, usize)>> = Vec::new();
    let mut out = Vec::with_capacity(sorted.len());
    for &(s, e, f, t, idx) in &sorted {
        let compatible = |members: &Vec<(usize, usize, usize, usize)>| {
            members
                .iter()
                .all(|&(s2, e2, f2, t2)| e2 + 2 <= s || e + 2 <= s2 || f2 == f || t2 == t)
        };
        let slot = match tracks.iter().position(compatible) {
            Some(x) => x,
            None => {
                tracks.push(Vec::new());
                tracks.len() - 1
            }
        };
        tracks[slot].push((s, e, f, t));
        out.push((idx, slot));
    }
    (out, tracks.len())
}

/// A merged set of edges that must share one bus row.
struct BusGroup {
    start: usize,
    /// `None` marks an open-ended span: the row runs from `start` to the shared
    /// right lane, so nothing to its right can share it.
    end: Option<usize>,
    edges: Vec<usize>,
}

/// Fan-in edges collected per target before they merge into bus rows. The
/// source set drives merging, so it stays here rather than on `BusGroup`.
struct FanInGroup {
    start: usize,
    end: usize,
    sources: Vec<usize>,
    edges: Vec<usize>,
}

/// Assign bus rows so every fan-in target owns exactly one row. Edges that
/// share a target always share a track, target groups with an identical source
/// set collapse into one shared bus, and anything else may share a row only
/// when the spans stay two cells apart. This keeps one arrow drop per target
/// instead of weaving a target's edges across several rows.
fn assign_bus_tracks(fan_in: &[BusSpan], skip_exits: &[&SkipEdge]) -> (Vec<(usize, usize)>, usize) {
    let mut by_target: BTreeMap<usize, FanInGroup> = BTreeMap::new();
    for span in fan_in {
        let group = by_target.entry(span.to).or_insert(FanInGroup {
            start: usize::MAX,
            end: 0,
            sources: Vec::new(),
            edges: Vec::new(),
        });
        group.sources.push(span.from);
        if span.jogs {
            group.start = group.start.min(span.start);
            group.end = group.end.max(span.end);
            group.edges.push(span.index);
        }
    }
    let mut merged: Vec<FanInGroup> = Vec::new();
    for (_, mut group) in by_target {
        // Straight drops join their group for source-set merging but never
        // demand a row of their own.
        if group.edges.is_empty() {
            continue;
        }
        group.sources.sort_unstable();
        group.sources.dedup();
        match merged
            .iter_mut()
            .find(|existing| existing.sources == group.sources)
        {
            // Identical source sets form a complete bipartite fan; one shared
            // bus row states exactly those connections.
            Some(existing) => {
                existing.start = existing.start.min(group.start);
                existing.end = existing.end.max(group.end);
                existing.edges.extend(group.edges);
            }
            None => merged.push(group),
        }
    }
    let mut groups: Vec<BusGroup> = merged
        .into_iter()
        .map(|group| BusGroup {
            start: group.start,
            end: Some(group.end),
            edges: group.edges,
        })
        .collect();
    let mut by_source: BTreeMap<usize, BusGroup> = BTreeMap::new();
    for skip in skip_exits {
        let group = by_source.entry(skip.from).or_insert(BusGroup {
            start: skip.source_center,
            end: None,
            edges: Vec::new(),
        });
        group.start = group.start.min(skip.source_center);
        group.edges.push(skip.index);
    }
    // Skip-exit groups are appended after fan-in merging, so they never merge
    // into a fan-in group; their exits lead to distinct right-lane columns.
    groups.extend(by_source.into_values());

    // Open-ended groups sort last: they reach past every bounded span.
    groups.sort_by_key(|group| {
        (
            group.start,
            group.end.is_none(),
            group.end,
            group.edges.first().copied(),
        )
    });
    let mut tracks: Vec<Vec<(usize, Option<usize>)>> = Vec::new();
    let mut out = Vec::new();
    for group in &groups {
        let compatible = |members: &Vec<(usize, Option<usize>)>| {
            members.iter().all(|&(start, end)| match (end, group.end) {
                (Some(end), Some(group_end)) => end + 2 <= group.start || group_end + 2 <= start,
                // An open-ended member owns the row out to the right lane,
                // so only a group that ends before it starts can join.
                (None, Some(group_end)) => group_end + 2 <= start,
                (Some(end), None) => end + 2 <= group.start,
                // Two open-ended spans always overlap in the right lane.
                (None, None) => false,
            })
        };
        let slot = match tracks.iter().position(compatible) {
            Some(slot) => slot,
            None => {
                tracks.push(Vec::new());
                tracks.len() - 1
            }
        };
        tracks[slot].push((group.start, group.end));
        for &idx in &group.edges {
            out.push((idx, slot));
        }
    }
    (out, tracks.len())
}
