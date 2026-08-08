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
/// through the shared right lane and joins its target's fan-in bus row.
struct SkipEdge {
    index: usize,
    from: usize,
    to: usize,
    source_center: usize,
    target_center: usize,
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
            target_center: centers[e.to],
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
    let mut join_slots = vec![0usize; graph.edges.len()];
    let mut bus_tracks = vec![0usize; max_rank + 1];
    for (r, tracks) in bus_tracks.iter_mut().enumerate().take(max_rank) {
        let spans = bus_spans(graph, ranks, &centers, r, CrossAxisAlignment::Near);
        let exits: Vec<&SkipEdge> = skips.iter().filter(|skip| skip.source_rank == r).collect();
        let joins: Vec<&SkipEdge> = skips
            .iter()
            .filter(|skip| skip.target_rank == r + 1)
            .collect();
        if spans.is_empty() && exits.is_empty() && joins.is_empty() {
            continue;
        }
        let assigned = assign_bus_tracks(&spans, &exits, &joins);
        for (idx, slot) in assigned.edges {
            edge_bus[idx] = slot;
        }
        for (idx, slot) in assigned.joins {
            join_slots[idx] = slot;
        }
        *tracks = assigned.tracks;
    }

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
        let gap = GAP_Y.max(bus_tracks[r - 1] + 1);
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

    let mut edge_join = vec![0usize; graph.edges.len()];
    for skip in &skips {
        edge_join[skip.index] = band_end[skip.target_rank - 1] + join_slots[skip.index];
    }

    RoutePlan {
        canvas: (canvas_w, canvas_h),
        band_end,
        edge_bus,
        source_anchors,
        lane_base,
        edge_lane,
        edge_join,
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
        let assigned = assign_bus_tracks(&spans, &[], &[]);
        for (idx, slot) in assigned.edges {
            edge_bus[idx] = slot;
        }
        *tracks = assigned.tracks;
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
        edge_join: vec![0; graph.edges.len()],
    }
}

pub(super) struct RoutePlan {
    pub(super) canvas: (usize, usize),
    pub(super) band_end: Vec<usize>,
    pub(super) edge_bus: Vec<usize>,
    pub(super) source_anchors: Vec<usize>,
    pub(super) lane_base: usize,
    pub(super) edge_lane: Vec<usize>,
    /// Absolute row of the target's fan-in bus for rank-skipping top-down
    /// edges; zero for every other edge. The skip edge joins that shared row
    /// from the right lane, so joined edges share ink and separate edges do
    /// not.
    pub(super) edge_join: Vec<usize>,
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
    joins: Vec<usize>,
}

/// Fan-in edges collected per target before they merge into bus rows. The
/// source set drives merging, so it stays here rather than on `BusGroup`.
struct FanInGroup {
    start: usize,
    end: usize,
    sources: Vec<usize>,
    edges: Vec<usize>,
    joins: Vec<usize>,
    /// The row also carries a skip exit, so it runs out to the right lane.
    exits: bool,
}

/// Bus track slots for one rank gap, split by how an edge uses its row.
struct BusAssignment {
    /// Adjacent forward edges and skip exits: slot in the gap below the source.
    edges: Vec<(usize, usize)>,
    /// Rank-skipping edges: slot of the target's fan-in row they join.
    joins: Vec<(usize, usize)>,
    tracks: usize,
}

/// Assign bus rows so every fan-in target owns exactly one row. Edges that
/// share a target always share a track, target groups with an identical source
/// set collapse into one shared bus, and anything else may share a row only
/// when the spans stay two cells apart. Rank-skipping edges join their
/// target's row from the right lane, which keeps every connection into a
/// target on the same shared ink instead of a separate crossing row.
fn assign_bus_tracks(
    fan_in: &[BusSpan],
    skip_exits: &[&SkipEdge],
    skip_joins: &[&SkipEdge],
) -> BusAssignment {
    let mut by_target: BTreeMap<usize, FanInGroup> = BTreeMap::new();
    let empty_group = || FanInGroup {
        start: usize::MAX,
        end: 0,
        sources: Vec::new(),
        edges: Vec::new(),
        joins: Vec::new(),
        exits: false,
    };
    for span in fan_in {
        let group = by_target.entry(span.to).or_insert_with(empty_group);
        group.sources.push(span.from);
        if span.jogs {
            group.start = group.start.min(span.start);
            group.end = group.end.max(span.end);
            group.edges.push(span.index);
        }
    }
    for skip in skip_joins {
        let group = by_target.entry(skip.to).or_insert_with(empty_group);
        group.sources.push(skip.from);
        group.start = group.start.min(skip.target_center);
        group.joins.push(skip.index);
    }
    let mut merged: Vec<FanInGroup> = Vec::new();
    for (_, mut group) in by_target {
        // Straight drops join their group for source-set merging but never
        // demand a row of their own.
        if group.edges.is_empty() && group.joins.is_empty() {
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
                existing.joins.extend(group.joins);
            }
            None => merged.push(group),
        }
    }
    // A skip exit whose source owns a whole fan-out row rides that same row
    // out to the lane: every line on the row still leaves one source, so the
    // shared ink stays unambiguous and the gap needs one row fewer.
    let mut by_source: BTreeMap<usize, BusGroup> = BTreeMap::new();
    for skip in skip_exits {
        match merged.iter_mut().find(|group| group.sources == [skip.from]) {
            Some(group) => {
                group.start = group.start.min(skip.source_center);
                group.edges.push(skip.index);
                group.exits = true;
            }
            None => {
                let group = by_source.entry(skip.from).or_insert(BusGroup {
                    start: skip.source_center,
                    end: None,
                    edges: Vec::new(),
                    joins: Vec::new(),
                });
                group.start = group.start.min(skip.source_center);
                group.edges.push(skip.index);
            }
        }
    }
    let mut groups: Vec<BusGroup> = merged
        .into_iter()
        .map(|group| BusGroup {
            start: group.start,
            // A joined or exiting row runs out to the right lane, so it stays
            // open-ended.
            end: (group.joins.is_empty() && !group.exits).then_some(group.end),
            edges: group.edges,
            joins: group.joins,
        })
        .collect();
    // Exit groups without a same-source fan-out row stay separate, so they
    // never imply a join with another source's edges.
    groups.extend(by_source.into_values());

    // Open-ended groups sort last: they reach past every bounded span.
    groups.sort_by_key(|group| {
        (
            group.start,
            group.end.is_none(),
            group.end,
            group.edges.first().or(group.joins.first()).copied(),
        )
    });
    let mut tracks: Vec<Vec<(usize, Option<usize>)>> = Vec::new();
    let mut out = BusAssignment {
        edges: Vec::new(),
        joins: Vec::new(),
        tracks: 0,
    };
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
            out.edges.push((idx, slot));
        }
        for &idx in &group.joins {
            out.joins.push((idx, slot));
        }
    }
    out.tracks = tracks.len();
    out
}
