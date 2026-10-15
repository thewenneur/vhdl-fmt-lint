//! Layered (Sugiyama style) layout of state diagrams, drawn top to bottom.
//!
//! 1. A depth-first walk from the initial state finds the edges that close
//!    a loop (back to a state on the walk, or into the initial state); they
//!    are turned around for the placement, so the rest flows downwards.
//! 2. Each state goes one layer below the lowest state with an edge to it.
//!    A state with several successors thereby opens branches that run side
//!    by side, and the states of a branch stack in its column.
//! 3. Edges spanning layers get a waypoint in every layer between (shared
//!    by the unlabelled edges into the same state, so loops back to `IDLE`
//!    merge into one trunk), and a labelled edge gets a slot for its label
//!    in the layer below its source. Edges never cross a state or a label.
//! 4. Layers are ordered by repeated barycentre sweeps, starting from the
//!    order of the walk so that branches start out contiguous, keeping the
//!    order with the fewest crossings.
//! 5. Each node is pulled towards its neighbours, waypoints harder than
//!    states so long edges run straight, under the spacing constraints of
//!    its layer (solved exactly by pool-adjacent-violators).

use std::collections::BTreeMap;

/// Vertical space between consecutive layers.
const LAYER_GAP: f64 = 18.0;
/// The most vertical space between layers.
const MAX_LAYER_GAP: f64 = 44.0;
/// Vertical space per unit of sideways travel of an edge between layers.
const SLOPE: f64 = 0.1;
/// Horizontal space between two states, or a state and a waypoint.
const STATE_GAP: f64 = 28.0;
/// Horizontal space between two waypoints.
const WAYPOINT_GAP: f64 = 12.0;
/// Room right of a state for its self-loop.
pub(crate) const LOOP_ROOM: f64 = 30.0;
/// Space between an edge and its label.
pub(crate) const LABEL_OFFSET: f64 = 6.0;
const ORDER_SWEEPS: usize = 24;
const PLACE_SWEEPS: usize = 40;

/// An edge between two states; `label` is the size of its label.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Edge {
    pub from: usize,
    pub to: usize,
    pub label: Option<(f64, f64)>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Route {
    /// From the source's border to the target's border, through the
    /// waypoints; empty for a self-loop.
    pub points: Vec<(f64, f64)>,
    /// The left end of the label's middle line.
    pub label: Option<(f64, f64)>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Layout {
    /// The centre of each state.
    pub states: Vec<(f64, f64)>,
    /// One per edge, in the order given.
    pub routes: Vec<Route>,
}

#[derive(Debug, Clone)]
struct Node {
    layer: usize,
    left: f64,
    right: f64,
    height: f64,
    state: bool,
}

/// Lays out states of the given sizes (width, height).
pub(crate) fn layout(sizes: &[(f64, f64)], edges: &[Edge], initial: Option<usize>) -> Layout {
    let n = sizes.len();
    if n == 0 {
        return Layout::default();
    }
    let initial = initial.filter(|&i| i < n);
    let reversed = loop_edges(n, edges, initial);
    let rank = ranks(n, edges, &reversed, initial);

    // The layered graph: states first, then waypoints.
    let mut nodes: Vec<Node> = sizes
        .iter()
        .enumerate()
        .map(|(i, &(w, h))| Node {
            layer: 2 * rank[i],
            left: w / 2.0,
            right: w / 2.0,
            height: h,
            state: true,
        })
        .collect();
    for e in edges.iter().filter(|e| e.from == e.to && e.from < n) {
        let extra = LOOP_ROOM + e.label.map_or(0.0, |(w, _)| w + LABEL_OFFSET);
        let node = &mut nodes[e.from];
        node.right = node.right.max(sizes[e.from].0 / 2.0 + extra);
    }
    let mut chains: Vec<Vec<usize>> = vec![Vec::new(); edges.len()];
    let mut label_node: Vec<Option<usize>> = vec![None; edges.len()];
    let mut shared: BTreeMap<(usize, usize, bool), usize> = BTreeMap::new();
    let mut links: BTreeMap<(usize, usize), f64> = BTreeMap::new();
    for (i, e) in edges.iter().enumerate() {
        if e.from == e.to || e.from >= n || e.to >= n {
            continue;
        }
        let (top, bottom) = if reversed[i] {
            (e.to, e.from)
        } else {
            (e.from, e.to)
        };
        let (rt, rb) = (nodes[top].layer, nodes[bottom].layer);
        let label_layer = if reversed[i] { rb - 1 } else { rt + 1 };
        let mut chain = vec![top];
        for layer in rt + 1..rb {
            let mut waypoint = |label: Option<(f64, f64)>| {
                let (right, height) =
                    label.map_or((0.0, 0.0), |(w, h)| (w + LABEL_OFFSET + 2.0, h));
                nodes.push(Node {
                    layer,
                    left: if label.is_some() { 2.0 } else { 0.0 },
                    right,
                    height,
                    state: false,
                });
                nodes.len() - 1
            };
            let id = match e.label {
                // Loops back merge into one trunk; edges down to a state run
                // apart and join just above it.
                None if reversed[i] || layer + 1 == rb => *shared
                    .entry((e.to, layer, reversed[i]))
                    .or_insert_with(|| waypoint(None)),
                None => waypoint(None),
                Some(size) => {
                    let id = waypoint((layer == label_layer).then_some(size));
                    if layer == label_layer {
                        label_node[i] = Some(id);
                    }
                    id
                }
            };
            chain.push(id);
        }
        chain.push(bottom);
        for pair in chain.windows(2) {
            let weight = match (nodes[pair[0]].state, nodes[pair[1]].state) {
                (true, true) => 1.0,
                (false, false) => 8.0,
                _ => 2.0,
            };
            links.insert((pair[0], pair[1]), weight);
        }
        chains[i] = chain;
    }

    let count = nodes.len();
    let mut down: Vec<Vec<(usize, f64)>> = vec![Vec::new(); count];
    let mut up: Vec<Vec<(usize, f64)>> = vec![Vec::new(); count];
    for (&(a, b), &w) in &links {
        down[a].push((b, w));
        up[b].push((a, w));
    }
    let layers = order(&nodes, &down, &up, initial);
    let x = place(&nodes, &layers, &down, &up);

    // Layer heights, then the y of each layer's middle line. Edges that go
    // far sideways get more height, so that they do not run flat.
    let mut ys = Vec::with_capacity(layers.len());
    let mut top = 0.0;
    for layer in &layers {
        let h = layer.iter().map(|&v| nodes[v].height).fold(0.0, f64::max);
        ys.push(top + h / 2.0);
        let x = &x;
        let sideways = layer
            .iter()
            .flat_map(|&v| down[v].iter().map(move |&(w, _)| (x[w] - x[v]).abs()))
            .fold(0.0, f64::max);
        top += h + (sideways * SLOPE).clamp(LAYER_GAP, MAX_LAYER_GAP);
    }
    let y = |v: usize| ys[nodes[v].layer];

    // Where each edge leaves the bottom or enters the top of a state:
    // spread along its border in the order of the other ends.
    let port = |v: usize, other: usize, list: &[(usize, f64)]| -> f64 {
        let mut ends: Vec<usize> = list.iter().map(|&(o, _)| o).collect();
        ends.sort_by(|&a, &b| x[a].total_cmp(&x[b]).then(a.cmp(&b)));
        let k = ends.len();
        let i = ends.iter().position(|&o| o == other).unwrap_or(0);
        if k <= 1 {
            return x[v];
        }
        let span = (nodes[v].left.min(sizes[v].0 / 2.0)) * 1.4;
        x[v] - span / 2.0 + span * (i as f64 + 0.5) / k as f64
    };
    let routes = edges
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let chain = &chains[i];
            if chain.len() < 2 {
                let label = e.label.filter(|_| e.from < n).map(|_| {
                    let (cx, cy) = (x[e.from], y(e.from));
                    (cx + sizes[e.from].0 / 2.0 + LOOP_ROOM, cy)
                });
                return Route {
                    points: Vec::new(),
                    label,
                };
            }
            let (first, last) = (chain[0], chain[chain.len() - 1]);
            let mut points = Vec::with_capacity(chain.len());
            points.push((
                port(first, chain[1], &down[first]),
                y(first) + nodes[first].height / 2.0,
            ));
            for &w in &chain[1..chain.len() - 1] {
                // Straight down past a label.
                let h = nodes[w].height / 2.0;
                points.push((x[w], y(w) - h));
                if h > 0.0 {
                    points.push((x[w], y(w) + h));
                }
            }
            points.push((
                port(last, chain[chain.len() - 2], &up[last]),
                y(last) - nodes[last].height / 2.0,
            ));
            if reversed[i] {
                points.reverse();
            }
            let label = label_node[i].map(|w| (x[w] + LABEL_OFFSET, y(w)));
            Route { points, label }
        })
        .collect();
    Layout {
        states: (0..n).map(|v| (x[v], y(v))).collect(),
        routes,
    }
}

/// The edges to turn around so that the rest has no cycle: edges back to a
/// state on the depth-first walk from the initial state, and edges into
/// the initial state.
fn loop_edges(n: usize, edges: &[Edge], initial: Option<usize>) -> Vec<bool> {
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, e) in edges.iter().enumerate() {
        if e.from != e.to && e.from < n && e.to < n {
            successors[e.from].push(i);
        }
    }
    // 0: not seen, 1: on the walk, 2: done.
    let mut seen = vec![0u8; n];
    let mut reversed = vec![false; edges.len()];
    for root in initial.into_iter().chain(0..n) {
        if seen[root] != 0 {
            continue;
        }
        seen[root] = 1;
        let mut stack = vec![(root, 0usize)];
        while let Some(top) = stack.len().checked_sub(1) {
            let (v, k) = stack[top];
            let Some(&e) = successors[v].get(k) else {
                seen[v] = 2;
                stack.pop();
                continue;
            };
            stack[top].1 += 1;
            let w = edges[e].to;
            if Some(w) == initial || seen[w] == 1 {
                reversed[e] = true;
            } else if seen[w] == 0 {
                seen[w] = 1;
                stack.push((w, 0));
            }
        }
    }
    reversed
}

/// The layer of each state: one below the lowest predecessor, with
/// sources other than the initial state pulled down next to their first
/// successor.
fn ranks(n: usize, edges: &[Edge], reversed: &[bool], initial: Option<usize>) -> Vec<usize> {
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indegree = vec![0usize; n];
    for (i, e) in edges.iter().enumerate() {
        if e.from == e.to || e.from >= n || e.to >= n {
            continue;
        }
        let (a, b) = if reversed[i] {
            (e.to, e.from)
        } else {
            (e.from, e.to)
        };
        successors[a].push(b);
        indegree[b] += 1;
    }
    let sources: Vec<usize> = (0..n).filter(|&v| indegree[v] == 0).collect();
    let mut rank = vec![0usize; n];
    let mut ready = sources.clone();
    let mut left = indegree.clone();
    while let Some(v) = ready.pop() {
        for &w in &successors[v] {
            rank[w] = rank[w].max(rank[v] + 1);
            left[w] -= 1;
            if left[w] == 0 {
                ready.push(w);
            }
        }
    }
    for v in sources {
        if Some(v) == initial {
            continue;
        }
        if let Some(lowest) = successors[v].iter().map(|&w| rank[w]).min() {
            rank[v] = lowest.saturating_sub(1);
        }
    }
    rank
}

/// The nodes of each layer, left to right.
fn order(
    nodes: &[Node],
    down: &[Vec<(usize, f64)>],
    up: &[Vec<(usize, f64)>],
    initial: Option<usize>,
) -> Vec<Vec<usize>> {
    let count = nodes.len();
    let depth = nodes.iter().map(|v| v.layer).max().unwrap_or(0) + 1;
    // Start from the order of a depth-first walk down the layered graph,
    // from the initial state, then the states that follow it in the
    // source, so that each branch starts out in one block.
    let states = nodes.iter().take_while(|v| v.state).count();
    let mut key = vec![usize::MAX; count];
    let mut next = 0;
    for root in initial.into_iter().chain(0..states) {
        if key[root] != usize::MAX {
            continue;
        }
        let mut stack = vec![root];
        while let Some(v) = stack.pop() {
            if key[v] != usize::MAX {
                continue;
            }
            key[v] = next;
            next += 1;
            for &(w, _) in down[v].iter().rev() {
                if key[w] == usize::MAX {
                    stack.push(w);
                }
            }
        }
    }
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); depth];
    for (v, node) in nodes.iter().enumerate() {
        layers[node.layer].push(v);
    }
    for layer in &mut layers {
        layer.sort_by_key(|&v| key[v]);
    }
    let mut pos = vec![0usize; count];
    let index = |layers: &[Vec<usize>], pos: &mut [usize]| {
        for layer in layers {
            for (i, &v) in layer.iter().enumerate() {
                pos[v] = i;
            }
        }
    };
    index(&layers, &mut pos);
    let mut best = layers.clone();
    let mut fewest = crossings(&layers, down, &pos);
    for sweep in 0..ORDER_SWEEPS {
        if fewest == 0 {
            break;
        }
        let downwards = sweep % 2 == 0;
        let range: Vec<usize> = if downwards {
            (1..depth).collect()
        } else {
            (0..depth.saturating_sub(1)).rev().collect()
        };
        for l in range {
            let neighbours = if downwards { up } else { down };
            let mut keyed: Vec<(f64, usize)> = layers[l]
                .iter()
                .map(|&v| {
                    let list = &neighbours[v];
                    let bary = if list.is_empty() {
                        pos[v] as f64
                    } else {
                        list.iter().map(|&(w, _)| pos[w] as f64).sum::<f64>() / list.len() as f64
                    };
                    (bary, v)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(pos[a.1].cmp(&pos[b.1])));
            layers[l] = keyed.into_iter().map(|(_, v)| v).collect();
            for (i, &v) in layers[l].iter().enumerate() {
                pos[v] = i;
            }
        }
        transpose(&mut layers, &mut pos, down, up);
        let c = crossings(&layers, down, &pos);
        if c < fewest {
            fewest = c;
            best = layers.clone();
        }
    }
    best
}

/// Swaps neighbours of a layer while that removes crossings.
fn transpose(
    layers: &mut [Vec<usize>],
    pos: &mut [usize],
    down: &[Vec<(usize, f64)>],
    up: &[Vec<(usize, f64)>],
) {
    // Crossings between the edges of `a` and of `b` (a left of b).
    let between = |a: usize, b: usize, pos: &[usize]| -> usize {
        let mut c = 0;
        for list in [down, up] {
            for &(p, _) in &list[a] {
                for &(q, _) in &list[b] {
                    if pos[p] > pos[q] {
                        c += 1;
                    }
                }
            }
        }
        c
    };
    for _ in 0..4 {
        let mut improved = false;
        for layer in layers.iter_mut() {
            for i in 1..layer.len() {
                let (a, b) = (layer[i - 1], layer[i]);
                if between(b, a, pos) < between(a, b, pos) {
                    layer.swap(i - 1, i);
                    pos[a] = i;
                    pos[b] = i - 1;
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
}

/// Edge crossings between all consecutive layers.
fn crossings(layers: &[Vec<usize>], down: &[Vec<(usize, f64)>], pos: &[usize]) -> usize {
    let mut total = 0;
    for layer in layers {
        let mut pairs: Vec<(usize, usize)> = layer
            .iter()
            .flat_map(|&v| down[v].iter().map(move |&(w, _)| (pos[v], pos[w])))
            .collect();
        pairs.sort_unstable();
        let mut ends: Vec<usize> = pairs.into_iter().map(|(_, b)| b).collect();
        total += inversions(&mut ends);
    }
    total
}

/// Counts the pairs out of order, sorting `v` (merge sort).
fn inversions(v: &mut [usize]) -> usize {
    if v.len() < 2 {
        return 0;
    }
    let mid = v.len() / 2;
    let mut count = inversions(&mut v[..mid]) + inversions(&mut v[mid..]);
    let mut merged = Vec::with_capacity(v.len());
    let (mut i, mut j) = (0, mid);
    while i < mid && j < v.len() {
        if v[j] < v[i] {
            count += mid - i;
            merged.push(v[j]);
            j += 1;
        } else {
            merged.push(v[i]);
            i += 1;
        }
    }
    merged.extend_from_slice(&v[i..mid]);
    merged.extend_from_slice(&v[j..]);
    v.copy_from_slice(&merged);
    count
}

/// The x of every node.
fn place(
    nodes: &[Node],
    layers: &[Vec<usize>],
    down: &[Vec<(usize, f64)>],
    up: &[Vec<(usize, f64)>],
) -> Vec<f64> {
    let gap = |a: usize, b: usize| {
        let spacing = if nodes[a].state || nodes[b].state {
            STATE_GAP
        } else {
            WAYPOINT_GAP
        };
        nodes[a].right + spacing + nodes[b].left
    };
    let mut x = vec![0.0; nodes.len()];
    for layer in layers {
        let mut at = 0.0;
        for (i, &v) in layer.iter().enumerate() {
            if i > 0 {
                at += gap(layer[i - 1], v);
            }
            x[v] = at;
        }
        // Centre each layer on 0.
        let shift = at / 2.0;
        for &v in layer {
            x[v] -= shift;
        }
    }
    let depth = layers.len();
    for sweep in 0..PLACE_SWEEPS {
        let range: Vec<usize> = if sweep % 2 == 0 {
            (0..depth).collect()
        } else {
            (0..depth).rev().collect()
        };
        for l in range {
            let layer = &layers[l];
            let mut desired = Vec::with_capacity(layer.len());
            let mut weight = Vec::with_capacity(layer.len());
            for &v in layer {
                let (mut sum, mut total) = (0.0, 0.0);
                for &(w, k) in down[v].iter().chain(&up[v]) {
                    sum += k * x[w];
                    total += k;
                }
                if total > 0.0 {
                    desired.push(sum / total);
                    weight.push(total);
                } else {
                    desired.push(x[v]);
                    weight.push(0.1);
                }
            }
            let gaps: Vec<f64> = layer.windows(2).map(|p| gap(p[0], p[1])).collect();
            for (&v, at) in layer.iter().zip(spaced(&desired, &weight, &gaps)) {
                x[v] = at;
            }
        }
    }
    let min = nodes
        .iter()
        .zip(&x)
        .map(|(v, &x)| x - v.left)
        .fold(f64::MAX, f64::min);
    for x in &mut x {
        *x -= min;
    }
    x
}

/// The positions closest to `desired` (weighted least squares) that keep
/// at least `gaps[i]` between positions `i` and `i + 1`.
fn spaced(desired: &[f64], weight: &[f64], gaps: &[f64]) -> Vec<f64> {
    let mut offset = Vec::with_capacity(desired.len());
    let mut at = 0.0;
    for i in 0..desired.len() {
        if i > 0 {
            at += gaps[i - 1];
        }
        offset.push(at);
    }
    // Pool adjacent violators on desired - offset, which must not decrease.
    // Blocks: (total weight, weighted sum, length).
    let mut blocks: Vec<(f64, f64, usize)> = Vec::new();
    for i in 0..desired.len() {
        blocks.push((weight[i], weight[i] * (desired[i] - offset[i]), 1));
        while blocks.len() >= 2 {
            let (w1, s1, n1) = blocks[blocks.len() - 1];
            let (w0, s0, n0) = blocks[blocks.len() - 2];
            if s0 / w0 <= s1 / w1 {
                break;
            }
            blocks.pop();
            *blocks.last_mut().unwrap() = (w0 + w1, s0 + s1, n0 + n1);
        }
    }
    let mut out = Vec::with_capacity(desired.len());
    for (w, s, len) in blocks {
        for _ in 0..len {
            out.push(s / w + offset[out.len()]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(from: usize, to: usize) -> Edge {
        Edge {
            from,
            to,
            label: None,
        }
    }

    #[test]
    fn spaced_keeps_gaps_and_order() {
        let x = spaced(&[0.0, 0.0, 100.0], &[1.0, 1.0, 1.0], &[10.0, 10.0]);
        assert!((x[1] - x[0] - 10.0).abs() < 1e-9);
        assert!((x[0] + 5.0).abs() < 1e-9);
        assert!((x[2] - 100.0).abs() < 1e-9);
    }

    #[test]
    fn inversions_are_counted() {
        assert_eq!(inversions(&mut [3, 1, 2]), 2);
        assert_eq!(inversions(&mut [1, 2, 3]), 0);
    }

    #[test]
    fn branches_run_side_by_side() {
        // IDLE -> A1 -> A2 -> IDLE, IDLE -> B1 -> B2 -> IDLE.
        let sizes = vec![(60.0, 30.0); 5];
        let edges = [
            edge(0, 1),
            edge(1, 2),
            edge(2, 0),
            edge(0, 3),
            edge(3, 4),
            edge(4, 0),
        ];
        let l = layout(&sizes, &edges, Some(0));
        let s = &l.states;
        // Layers: IDLE above A1 and B1, above A2 and B2.
        assert!(s[0].1 < s[1].1 && s[1].1 < s[2].1);
        assert_eq!(s[1].1, s[3].1);
        assert_eq!(s[2].1, s[4].1);
        // The branches run side by side.
        assert!((s[1].0 - s[3].0).abs() >= 60.0 + STATE_GAP - 1e-6);
        assert!((s[2].0 - s[4].0).abs() >= 60.0 + STATE_GAP - 1e-6);
        assert_eq!(s[1].0 < s[3].0, s[2].0 < s[4].0);
        // The loop back ends at the bottom of IDLE, going up.
        let back = &l.routes[2].points;
        assert_eq!(back.last().unwrap().1, s[0].1 + 15.0);
    }

    #[test]
    fn states_never_overlap() {
        // 60 states with branches, retries and a common error state.
        let n = 60;
        let sizes = vec![(70.0, 30.0); n];
        let mut edges = Vec::new();
        for i in 1..n {
            edges.push(edge((i - 1) / 3, i));
            edges.push(edge(i, 0));
            if i % 7 == 0 {
                edges.push(edge(i, n - 1));
                edges.push(Edge {
                    from: i,
                    to: i / 2,
                    label: Some((40.0, 14.0)),
                });
            }
        }
        let l = layout(&sizes, &edges, Some(0));
        for a in 0..n {
            for b in a + 1..n {
                let (pa, pb) = (l.states[a], l.states[b]);
                assert!(
                    (pa.1 - pb.1).abs() >= 30.0 || (pa.0 - pb.0).abs() >= 70.0,
                    "{a} and {b} overlap"
                );
            }
        }
        assert_eq!(l.routes.len(), edges.len());
        assert!(l.routes.iter().all(|r| r.points.len() >= 2));
    }
}
