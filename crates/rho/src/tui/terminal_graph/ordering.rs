// Adapted from Grok Build's terminal Mermaid renderer:
// https://github.com/xai-org/grok-build/blob/b189869b7755d2b482969acf6c92da3ecfeffd36/crates/codegen/xai-grok-markdown/src/mermaid.rs
// Copyright 2023-2026 SpaceXAI. Licensed under Apache-2.0.
use super::Edge;

/// Reorder nodes within each rank to minimize edge crossings (Sugiyama-style
/// barycenter sweeps), while keeping the source order as the stable tie-breaker.
pub(in crate::tui) fn order_ranks(by_rank: &mut [Vec<usize>], edges: &[Edge], ranks: &[usize]) {
    let n = ranks.len();
    if by_rank.len() < 2 || n < 3 {
        return;
    }
    let mut parents: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for edge in edges {
        if edge.from != edge.to && ranks[edge.to] > ranks[edge.from] {
            parents[edge.to].push(edge.from);
            children[edge.from].push(edge.to);
        }
    }

    let mut pos = vec![0usize; n];
    let set_pos = |by_rank: &[Vec<usize>], pos: &mut Vec<usize>| {
        for row in by_rank {
            for (i, &v) in row.iter().enumerate() {
                pos[v] = i;
            }
        }
    };
    set_pos(by_rank, &mut pos);

    let mut best: Vec<Vec<usize>> = by_rank.to_vec();
    let mut best_crossings = count_crossings(edges, ranks, &pos);
    if best_crossings == 0 {
        return;
    }

    for iteration in 0..8 {
        if iteration % 2 == 0 {
            for row in by_rank.iter_mut().skip(1) {
                sort_by_barycenter(row, &parents, &pos);
                for (i, &v) in row.iter().enumerate() {
                    pos[v] = i;
                }
            }
        } else {
            let last = by_rank.len() - 1;
            for row in by_rank[..last].iter_mut().rev() {
                sort_by_barycenter(row, &children, &pos);
                for (i, &v) in row.iter().enumerate() {
                    pos[v] = i;
                }
            }
        }
        let crossings = count_crossings(edges, ranks, &pos);
        if crossings < best_crossings {
            best_crossings = crossings;
            best = by_rank.to_vec();
        }
        if best_crossings == 0 {
            break;
        }
    }

    for (row, best_row) in by_rank.iter_mut().zip(best) {
        *row = best_row;
    }
}

fn sort_by_barycenter(row: &mut [usize], neighbours: &[Vec<usize>], pos: &[usize]) {
    let mut keyed: Vec<(f64, usize)> = row
        .iter()
        .map(|&node| {
            let key = if neighbours[node].is_empty() {
                pos[node] as f64
            } else {
                neighbours[node]
                    .iter()
                    .map(|&neighbour| pos[neighbour] as f64)
                    .sum::<f64>()
                    / neighbours[node].len() as f64
            };
            (key, node)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (slot, (_, node)) in row.iter_mut().zip(keyed) {
        *slot = node;
    }
}

fn count_crossings(edges: &[Edge], ranks: &[usize], pos: &[usize]) -> usize {
    let adjacent: Vec<(usize, usize, usize)> = edges
        .iter()
        .filter(|edge| edge.from != edge.to && ranks[edge.to] == ranks[edge.from] + 1)
        .map(|edge| (ranks[edge.from], pos[edge.from], pos[edge.to]))
        .collect();
    let mut crossings = 0;
    for (index, first) in adjacent.iter().enumerate() {
        for second in &adjacent[index + 1..] {
            if first.0 == second.0
                && ((first.1 < second.1 && first.2 > second.2)
                    || (first.1 > second.1 && first.2 < second.2))
            {
                crossings += 1;
            }
        }
    }
    crossings
}

/// Assign a center coordinate to every node along the cross-axis. Rank order
/// and a minimum separation remain fixed while barycenters straighten chains.
pub(in crate::tui) fn assign_positions(
    by_rank: &[Vec<usize>],
    size: &[usize],
    sep: usize,
    edges: &[Edge],
    ranks: &[usize],
) -> Vec<usize> {
    let n = size.len();
    let mut parents: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for edge in edges {
        if edge.from != edge.to && ranks[edge.to] > ranks[edge.from] {
            parents[edge.to].push(edge.from);
            children[edge.from].push(edge.to);
        }
    }

    let mut pos = vec![0f64; n];
    for row in by_rank {
        let mut coordinate = 0f64;
        for &node in row {
            let half = size[node] as f64 / 2.0;
            coordinate += half;
            pos[node] = coordinate;
            coordinate += half + sep as f64;
        }
    }

    for iteration in 0..10 {
        if iteration % 2 == 0 {
            for row in by_rank {
                relax_rank(row, &parents, &mut pos, size, sep);
            }
        } else {
            for row in by_rank.iter().rev() {
                relax_rank(row, &children, &mut pos, size, sep);
            }
        }
    }

    let min_left = (0..n)
        .map(|node| pos[node] - size[node] as f64 / 2.0)
        .fold(f64::INFINITY, f64::min);
    let min_left = if min_left.is_finite() { min_left } else { 0.0 };
    (0..n)
        .map(|node| (pos[node] - min_left).round().max(0.0) as usize)
        .collect()
}

fn relax_rank(
    nodes: &[usize],
    neighbours: &[Vec<usize>],
    pos: &mut [f64],
    size: &[usize],
    sep: usize,
) {
    let n = nodes.len();
    if n == 0 {
        return;
    }
    let desired: Vec<f64> = nodes
        .iter()
        .map(|&node| {
            if neighbours[node].is_empty() {
                pos[node]
            } else {
                neighbours[node]
                    .iter()
                    .map(|&neighbour| pos[neighbour])
                    .sum::<f64>()
                    / neighbours[node].len() as f64
            }
        })
        .collect();

    let half = |index: usize| size[nodes[index]] as f64 / 2.0;
    let mut left = vec![0f64; n];
    let mut right = vec![0f64; n];
    for index in 0..n {
        left[index] = if index == 0 {
            desired[index]
        } else {
            desired[index].max(left[index - 1] + half(index - 1) + sep as f64 + half(index))
        };
    }
    for index in (0..n).rev() {
        right[index] = if index == n - 1 {
            desired[index]
        } else {
            desired[index].min(right[index + 1] - half(index + 1) - sep as f64 - half(index))
        };
    }
    for index in 0..n {
        pos[nodes[index]] = (left[index] + right[index]) / 2.0;
    }
    for index in 1..n {
        let minimum = pos[nodes[index - 1]] + half(index - 1) + sep as f64 + half(index);
        if pos[nodes[index]] < minimum {
            pos[nodes[index]] = minimum;
        }
    }
}
