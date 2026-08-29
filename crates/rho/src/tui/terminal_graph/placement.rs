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

/// One non-adjacent edge in side-lane terms: the cross-axis interval it
/// crosses plus its endpoints.
struct LaneSpan {
    start: usize,
    end: usize,
    from: usize,
    to: usize,
    index: usize,
}

fn lane_spans(graph: &Graph, ranks: &[usize], placed: &[Placed], axis: LaneAxis) -> Vec<LaneSpan> {
    graph
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| e.from != e.to && ranks[e.to] != ranks[e.from] + 1)
        .map(|(index, e)| {
            let (pf, pt) = (&placed[e.from], &placed[e.to]);
            let (start, end) = match axis {
                LaneAxis::Vertical => (pf.cy.min(pt.cy), pf.cy.max(pt.cy)),
                LaneAxis::Horizontal => (pf.cx.min(pt.cx), pf.cx.max(pt.cx)),
            };
            LaneSpan {
                start,
                end,
                from: e.from,
                to: e.to,
                index,
            }
        })
        .collect()
}

/// Widest painted row of a wrapped edge label; zero for unlabeled edges.
fn label_block_width(lines: &[String]) -> usize {
    lines.iter().map(|line| line.width()).max().unwrap_or(0)
}

/// Extra rows each rank gap must add so stacked edge-label rows have room
/// above their target's head row. Single-line labels need no extra row, which
/// keeps existing single-line geometry byte-identical.
fn label_gap_rows(
    graph: &Graph,
    ranks: &[usize],
    edge_labels: &[Vec<String>],
    max_rank: usize,
) -> Vec<usize> {
    let mut rows = vec![0usize; max_rank + 1];
    for (index, edge) in graph.edges.iter().enumerate() {
        let lines = edge_labels[index].len();
        if edge.from != edge.to && ranks[edge.to] > ranks[edge.from] && lines > 1 {
            let gap = ranks[edge.to] - 1;
            rows[gap] = rows[gap].max(lines - 1);
        }
    }
    rows
}

pub(super) fn place_td(
    ranks: &[usize],
    max_rank: usize,
    by_rank: &[Vec<usize>],
    sizes: &NodeSizes,
    graph: &Graph,
    edge_labels: &[Vec<String>],
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
    let label_rows = label_gap_rows(graph, ranks, edge_labels, max_rank);
    let mut rank_y = vec![0usize; max_rank + 1];
    for r in 1..=max_rank {
        let gap = GAP_Y.max(bus_tracks[r - 1] + 1) + label_rows[r - 1];
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
    for (index, e) in graph.edges.iter().enumerate() {
        if e.from == e.to {
            continue;
        }
        let lw = label_block_width(&edge_labels[index]);
        if lw == 0 {
            continue;
        }
        if ranks[e.to] == ranks[e.from] + 1 {
            content_w = content_w.max(placed[e.to].cx + 2 + lw);
        } else {
            content_w = content_w.max(diagram_w + lw + 1);
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
    edge_labels: &[Vec<String>],
    placed: &mut [Placed],
) -> RoutePlan {
    let col_w: Vec<usize> = by_rank
        .iter()
        .map(|row| row.iter().map(|&i| sizes.box_w[i]).max().unwrap_or(0))
        .collect();

    // Wrapped labels shrink the inter-column gap to the widest painted row;
    // self-loop labels remain single-line beside the node.
    let max_label = graph
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| e.from == e.to || ranks[e.to] == ranks[e.from] + 1)
        .filter_map(|(index, e)| {
            if e.from == e.to {
                e.label.as_ref().map(|l| l.width().min(MAX_LABEL))
            } else {
                Some(label_block_width(&edge_labels[index])).filter(|&w| w > 0)
            }
        })
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

/// Greedily pack items into the fewest tracks, in the given order: each item
/// joins the first track where it is compatible with every member, else it
/// opens a new track. Returns one slot per item plus the track count.
fn pack_tracks<T>(items: &[T], compatible: impl Fn(&T, &T) -> bool) -> (Vec<usize>, usize) {
    let mut tracks: Vec<Vec<usize>> = Vec::new();
    let mut slots = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let fits = |members: &Vec<usize>| {
            members
                .iter()
                .all(|&member| compatible(&items[member], item))
        };
        let slot = match tracks.iter().position(fits) {
            Some(slot) => slot,
            None => {
                tracks.push(Vec::new());
                tracks.len() - 1
            }
        };
        tracks[slot].push(index);
        slots.push(slot);
    }
    (slots, tracks.len())
}

fn assign_tracks(spans: &[LaneSpan]) -> (Vec<(usize, usize)>, usize) {
    let mut sorted: Vec<&LaneSpan> = spans.iter().collect();
    sorted.sort_by_key(|span| (span.start, span.end, span.from, span.to, span.index));
    // Spans may share a lane when they stay two cells apart or share an
    // endpoint: shared ink then still belongs to one node.
    let (slots, count) = pack_tracks(&sorted, |member, span| {
        member.end + 2 <= span.start
            || span.end + 2 <= member.start
            || member.from == span.from
            || member.to == span.to
    });
    let out = sorted
        .iter()
        .zip(slots)
        .map(|(span, slot)| (span.index, slot))
        .collect();
    (out, count)
}

/// A merged set of edges that must share one bus row. The source set drives
/// merging: target groups with an identical source set collapse into one
/// shared row.
struct BusGroup {
    start: usize,
    end: usize,
    sources: Vec<usize>,
    edges: Vec<usize>,
    joins: Vec<usize>,
    /// The row also carries a skip exit, so it runs out to the right lane.
    exits: bool,
}

impl BusGroup {
    fn empty() -> Self {
        Self {
            start: usize::MAX,
            end: 0,
            sources: Vec::new(),
            edges: Vec::new(),
            joins: Vec::new(),
            exits: false,
        }
    }

    /// `None` marks an open-ended row: a skip join or exit runs it from
    /// `start` out to the shared right lane, so nothing to its right can
    /// share it.
    fn bounded_end(&self) -> Option<usize> {
        (self.joins.is_empty() && !self.exits).then_some(self.end)
    }
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
    let mut by_target: BTreeMap<usize, BusGroup> = BTreeMap::new();
    for span in fan_in {
        let group = by_target.entry(span.to).or_insert_with(BusGroup::empty);
        group.sources.push(span.from);
        if span.jogs {
            group.start = group.start.min(span.start);
            group.end = group.end.max(span.end);
            group.edges.push(span.index);
        }
    }
    for skip in skip_joins {
        let group = by_target.entry(skip.to).or_insert_with(BusGroup::empty);
        group.sources.push(skip.from);
        group.start = group.start.min(skip.target_center);
        group.joins.push(skip.index);
    }
    let mut merged: Vec<BusGroup> = Vec::new();
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
                let group = by_source.entry(skip.from).or_insert_with(|| BusGroup {
                    sources: vec![skip.from],
                    exits: true,
                    ..BusGroup::empty()
                });
                group.start = group.start.min(skip.source_center);
                group.edges.push(skip.index);
            }
        }
    }
    // Exit groups without a same-source fan-out row stay separate, so they
    // never imply a join with another source's edges.
    let mut groups = merged;
    groups.extend(by_source.into_values());

    // Open-ended groups sort last: they reach past every bounded span.
    groups.sort_by_key(|group| {
        (
            group.start,
            group.bounded_end().is_none(),
            group.bounded_end(),
            group.edges.first().or(group.joins.first()).copied(),
        )
    });
    let (slots, tracks) = pack_tracks(&groups, |member, group| {
        match (member.bounded_end(), group.bounded_end()) {
            (Some(end), Some(group_end)) => end + 2 <= group.start || group_end + 2 <= member.start,
            // An open-ended member owns the row out to the right lane,
            // so only a group that ends before it starts can join.
            (None, Some(group_end)) => group_end + 2 <= member.start,
            (Some(end), None) => end + 2 <= group.start,
            // Two open-ended spans always overlap in the right lane.
            (None, None) => false,
        }
    });
    let mut out = BusAssignment {
        edges: Vec::new(),
        joins: Vec::new(),
        tracks,
    };
    for (group, slot) in groups.iter().zip(slots) {
        for &idx in &group.edges {
            out.edges.push((idx, slot));
        }
        for &idx in &group.joins {
            out.joins.push((idx, slot));
        }
    }
    out
}
