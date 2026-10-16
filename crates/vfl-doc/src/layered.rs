//! Layered (Sugiyama style) layout of state diagrams, drawn top to bottom
//! with orthogonal edges.
//!
//! 1. A depth-first walk from the initial state finds the edges that close
//!    a loop (back to a state on the walk, or into the initial state); they
//!    are turned around for the placement, so the rest flows downwards.
//! 2. Each state goes one layer below the lowest state with an edge to it.
//!    A state with several successors thereby opens branches that run side
//!    by side, and the states of a branch stack in its column.
//! 3. Edges spanning layers get a waypoint in every layer between. The
//!    edges into the same state share theirs, so that they merge into one
//!    trunk with one arrowhead (all the loops back to `IDLE`, all the
//!    errors into `FAULT`); a labelled edge first gets a waypoint of its
//!    own with room for its label, which sits on it just after its source. Edges never
//!    cross a state or a label.
//! 4. Layers are ordered by repeated barycentre sweeps, starting from the
//!    order of the walk so that branches start out contiguous, keeping the
//!    order with the fewest crossings.
//! 5. Each node is pulled towards its neighbours, waypoints harder than
//!    states so long edges run straight, under the spacing constraints of
//!    its layer (solved exactly by pool-adjacent-violators).
//! 6. Edges leave and enter states through ports spread along their top
//!    and bottom sides, and run vertically through the layers. Between two
//!    layers, the segments of the edges going to the same node merge into
//!    one horizontal track; tracks are stacked in the order that crosses
//!    least.

use std::collections::BTreeMap;

/// Vertical space between the tracks of a channel, and between a track
/// and the layers around it.
const TRACK_GAP: f64 = 9.0;
/// The least height of a channel between two layers.
const CHANNEL_MIN: f64 = 16.0;
/// Horizontal space between the ends of two segments sharing a track.
const TRACK_SEP: f64 = 8.0;
/// The least space between two ports on a side of a state.
const PORT_GAP: f64 = 10.0;
/// Room kept free of ports at each end of a side of a state.
pub(crate) const CORNER: f64 = 9.0;
/// Horizontal space between two states, or a state and a waypoint.
const STATE_GAP: f64 = 28.0;
/// Horizontal space between two waypoints.
const WAYPOINT_GAP: f64 = 12.0;
/// Room right of a state for its self-loop.
pub(crate) const LOOP_ROOM: f64 = 26.0;
/// Space between an edge and its label.
pub(crate) const LABEL_OFFSET: f64 = 6.0;
/// Trunks this many layers long or more, joined by this many edges or
/// more, run down the side of the diagram.
const MIN_SIDE_TRUNK: usize = 3;
const MIN_SIDE_EDGES: usize = 4;
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
    /// The corners of the edge, from the source's border to the target's
    /// border; empty for a self-loop.
    pub points: Vec<(f64, f64)>,
    /// The left end of the middle line of the label, which sits on the
    /// edge.
    pub label: Option<(f64, f64)>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Layout {
    /// The centre of each state.
    pub states: Vec<(f64, f64)>,
    /// The width of each state, widened to fit its ports.
    pub widths: Vec<f64>,
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

/// The edges going into a state: the state, and whether they go up.
type Net = (usize, bool);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Side {
    Top,
    Bottom,
}

/// What a port of a state is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum End {
    /// Every edge arriving on that side.
    Arrive,
    /// The edges fanning out on that side.
    Fan,
    /// The edge leaving through the given neighbour.
    Leave(usize),
}

/// The segments between two layers that share a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    /// Those of the edges into a state that go to the given node.
    Into(Net, usize),
    /// Those leaving that side of a state, apart from the above.
    Out(usize, Side),
    /// A segment on its own.
    One(usize, usize),
}

/// The segments of a channel that merge: their ends on the layer above
/// and on the layer below.
#[derive(Debug, Clone, Default)]
struct Hyper {
    tops: Vec<f64>,
    bottoms: Vec<f64>,
    lo: f64,
    hi: f64,
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
    // The waypoints of the edges into a state, by layer: all of them share
    // one, so that they merge into one trunk.
    let mut trunks: BTreeMap<(Net, usize), usize> = BTreeMap::new();
    let mut links: BTreeMap<(usize, usize), (f64, Net)> = BTreeMap::new();
    for (i, e) in edges.iter().enumerate() {
        if e.from == e.to || e.from >= n || e.to >= n {
            continue;
        }
        let net = (e.to, reversed[i]);
        let (top, bottom) = if reversed[i] {
            (e.to, e.from)
        } else {
            (e.from, e.to)
        };
        let (rt, rb) = (nodes[top].layer, nodes[bottom].layer);
        // The label goes next to the edge where it leaves its source,
        // before it joins the others.
        let label_layer = if reversed[i] { rb - 1 } else { rt + 1 };
        let mut chain = vec![top];
        for layer in rt + 1..rb {
            let id = match e.label {
                Some(size) if layer == label_layer => {
                    let id = waypoint(&mut nodes, layer, Some(size));
                    label_node[i] = Some(id);
                    id
                }
                _ => match trunks.get(&(net, layer)) {
                    Some(&id) => id,
                    None => {
                        let id = waypoint(&mut nodes, layer, None);
                        trunks.insert((net, layer), id);
                        id
                    }
                },
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
            links.insert((pair[0], pair[1]), (weight, net));
        }
        chains[i] = chain;
    }

    // The segments between two layers that merge: those of the edges into
    // the same state that go to the same node, and else those leaving the
    // same side of a state. Merging only one way keeps every merged line
    // telling where it goes.
    let mut into: BTreeMap<(Net, usize), usize> = BTreeMap::new();
    for (&(a, b), &(_, net)) in &links {
        *into.entry((net, if net.1 { a } else { b })).or_default() += 1;
    }
    let mut out: BTreeMap<(usize, Side), usize> = BTreeMap::new();
    for (&(a, b), &(_, net)) in &links {
        let (d, u, side) = if net.1 {
            (a, b, Side::Top)
        } else {
            (b, a, Side::Bottom)
        };
        if into[&(net, d)] == 1 && nodes[u].state {
            *out.entry((u, side)).or_default() += 1;
        }
    }
    let group: BTreeMap<(usize, usize), Group> = links
        .iter()
        .map(|(&(a, b), &(_, net))| {
            let (d, u, side) = if net.1 {
                (a, b, Side::Top)
            } else {
                (b, a, Side::Bottom)
            };
            let g = if into[&(net, d)] > 1 {
                Group::Into(net, d)
            } else if out.get(&(u, side)).is_some_and(|&k| k > 1) {
                Group::Out(u, side)
            } else {
                Group::One(a, b)
            };
            ((a, b), g)
        })
        .collect();
    // How link (a, b) meets state `v`, one of its ends.
    let end = |v: usize, a: usize, b: usize| -> End {
        let net = links[&(a, b)].1;
        if net.0 == v {
            End::Arrive
        } else if matches!(group[&(a, b)], Group::Out(..)) {
            End::Fan
        } else {
            End::Leave(if v == a { b } else { a })
        }
    };

    // The ports of each side of each state: one for the edges arriving,
    // one for those fanning out, one for each other edge leaving. States
    // widen to fit them.
    let mut ports: BTreeMap<(usize, Side), Vec<End>> = BTreeMap::new();
    for &(a, b) in links.keys() {
        if nodes[a].state {
            ports
                .entry((a, Side::Bottom))
                .or_default()
                .push(end(a, a, b));
        }
        if nodes[b].state {
            ports.entry((b, Side::Top)).or_default().push(end(b, a, b));
        }
    }
    let mut widths: Vec<f64> = sizes.iter().map(|s| s.0).collect();
    for (&(v, _), ends) in &mut ports {
        ends.sort_unstable();
        ends.dedup();
        let need = (ends.len() - 1) as f64 * PORT_GAP + 2.0 * CORNER;
        if need > widths[v] {
            let grow = (need - widths[v]) / 2.0;
            widths[v] = need;
            nodes[v].left += grow;
            nodes[v].right += grow;
        }
    }

    let count = nodes.len();
    let mut down: Vec<Vec<(usize, f64)>> = vec![Vec::new(); count];
    let mut up: Vec<Vec<(usize, f64)>> = vec![Vec::new(); count];
    for (&(a, b), &(w, _)) in &links {
        down[a].push((b, w));
        up[b].push((a, w));
    }
    let mut layers = order(&nodes, &down, &up, initial);
    to_the_side(&mut layers, &trunks, edges, &reversed);
    let mut x = place(&nodes, &layers, &down, &up);
    straighten(&nodes, &layers, &mut x, &down, &up, &trunks, &widths);
    for _ in 0..3 {
        snap(&nodes, &layers, &mut x, &down, &up, &widths);
        straighten(&nodes, &layers, &mut x, &down, &up, &trunks, &widths);
    }

    // Port positions: under or over the other end if there is room, else
    // as close as the spacing allows.
    let place_ports = |x: &[f64]| {
        let mut port_x: BTreeMap<(usize, Side, End), f64> = BTreeMap::new();
        for (&(v, side), ends) in &ports {
            // The mean x of the other ends of the links through each port.
            let mut sums: BTreeMap<End, (f64, f64)> = BTreeMap::new();
            let list = if side == Side::Top { &up[v] } else { &down[v] };
            for &(o, _) in list {
                let (a, b) = if side == Side::Top { (o, v) } else { (v, o) };
                let sum = sums.entry(end(v, a, b)).or_default();
                sum.0 += x[o];
                sum.1 += 1.0;
            }
            let others: Vec<f64> = ends
                .iter()
                .map(|e| sums.get(e).map_or(x[v], |&(s, k)| s / k))
                .collect();
            let mut sorted: Vec<usize> = (0..ends.len()).collect();
            sorted.sort_by(|&a, &b| others[a].total_cmp(&others[b]).then(a.cmp(&b)));
            let (lo, hi) = (
                x[v] - widths[v] / 2.0 + CORNER,
                x[v] + widths[v] / 2.0 - CORNER,
            );
            let desired: Vec<f64> = sorted.iter().map(|&k| clamp(others[k], lo, hi)).collect();
            let mut at = spaced(
                &desired,
                &vec![1.0; desired.len()],
                &vec![PORT_GAP; desired.len().saturating_sub(1)],
            );
            if let (Some(&first), Some(&last)) = (at.first(), at.last()) {
                let shift = if first < lo {
                    lo - first
                } else if last > hi {
                    (hi - last).max(lo - first)
                } else {
                    0.0
                };
                for p in &mut at {
                    *p += shift;
                }
            }
            // A port a hair off its other end lines up with it where the
            // spacing allows: a tiny step reads as a glitch.
            for i in 0..at.len() {
                let want = others[sorted[i]];
                let fits = want >= lo
                    && want <= hi
                    && (i == 0 || want - at[i - 1] >= PORT_GAP)
                    && (i + 1 == at.len() || at[i + 1] - want >= PORT_GAP);
                if (want - at[i]).abs() < 6.0 && fits {
                    at[i] = want;
                }
            }
            for (&k, p) in sorted.iter().zip(at) {
                port_x.insert((v, side, ends[k]), p);
            }
        }
        port_x
    };
    let mut port_x = place_ports(&x);
    // Then each waypoint moves, if its layer leaves it room, to where more
    // of its links run straight to the ports as placed.
    for _ in 0..3 {
        let mut moved = false;
        for layer in &layers {
            for (i, &w) in layer.iter().enumerate() {
                if nodes[w].state {
                    continue;
                }
                let lo = i
                    .checked_sub(1)
                    .map_or(f64::MIN, |j| x[layer[j]] + gap(&nodes, layer[j], w));
                let hi = layer
                    .get(i + 1)
                    .map_or(f64::MAX, |&r| x[r] - gap(&nodes, w, r));
                let ends: Vec<f64> = up[w]
                    .iter()
                    .map(|&(o, _)| (o, w))
                    .chain(down[w].iter().map(|&(o, _)| (w, o)))
                    .map(|(a, b)| {
                        let o = if a == w { b } else { a };
                        if nodes[o].state {
                            let side = if o == a { Side::Bottom } else { Side::Top };
                            port_x[&(o, side, end(o, a, b))]
                        } else {
                            x[o]
                        }
                    })
                    .collect();
                let straight = |p: f64| ends.iter().filter(|&&e| (e - p).abs() < 0.5).count();
                let now = straight(x[w]);
                if let Some(&best) = ends
                    .iter()
                    .filter(|&&p| p >= lo && p <= hi && straight(p) > now)
                    .max_by_key(|&&p| straight(p))
                {
                    x[w] = best;
                    moved = true;
                }
            }
        }
        if !moved {
            break;
        }
        port_x = place_ports(&x);
    }
    let x = x;
    let end_x = |v: usize, a: usize, b: usize| -> f64 {
        if nodes[v].state {
            let side = if v == a { Side::Bottom } else { Side::Top };
            port_x[&(v, side, end(v, a, b))]
        } else {
            x[v]
        }
    };

    // The channels between layers: the segments of each group share one
    // horizontal track, and the tracks are stacked to keep crossings few.
    let depth = layers.len();
    let mut hypers: Vec<Vec<Hyper>> = vec![Vec::new(); depth.saturating_sub(1)];
    let mut hyper_of: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    let mut groups: BTreeMap<Group, usize> = BTreeMap::new();
    for &(a, b) in links.keys() {
        let channel = nodes[a].layer;
        let list = &mut hypers[channel];
        let h = *groups.entry(group[&(a, b)]).or_insert_with(|| {
            list.push(Hyper::default());
            list.len() - 1
        });
        let (top, bottom) = (end_x(a, a, b), end_x(b, a, b));
        let hyper = &mut list[h];
        hyper.tops.push(top);
        hyper.bottoms.push(bottom);
        hyper_of.insert((a, b), h);
    }
    let mut track_y: Vec<Vec<Option<f64>>> = Vec::with_capacity(hypers.len());
    let mut channel_height = Vec::with_capacity(hypers.len());
    for list in &mut hypers {
        for h in list.iter_mut() {
            let all = h.tops.iter().chain(&h.bottoms);
            h.lo = all.clone().fold(f64::MAX, |a, &b| a.min(b));
            h.hi = all.fold(f64::MIN, |a, &b| a.max(b));
        }
        let (tracks, count) = tracks(list);
        let height = ((count + 1) as f64 * TRACK_GAP).max(CHANNEL_MIN);
        let offset = (height - (count + 1) as f64 * TRACK_GAP) / 2.0;
        track_y.push(
            tracks
                .iter()
                .map(|t| t.map(|t| offset + (t + 1) as f64 * TRACK_GAP))
                .collect(),
        );
        channel_height.push(height);
    }

    // Layer heights, then the y of each layer's middle line.
    let mut ys = Vec::with_capacity(depth);
    let mut channel_top = Vec::with_capacity(depth);
    let mut top = 0.0;
    for (l, layer) in layers.iter().enumerate() {
        let h = layer.iter().map(|&v| nodes[v].height).fold(0.0, f64::max);
        ys.push(top + h / 2.0);
        top += h;
        channel_top.push(top);
        top += channel_height.get(l).copied().unwrap_or(0.0);
    }
    let y = |v: usize| ys[nodes[v].layer];

    let routes = edges
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let chain = &chains[i];
            if chain.len() < 2 {
                let label = e.label.filter(|_| e.from < n).map(|_| {
                    let (cx, cy) = (x[e.from], y(e.from));
                    (cx + widths[e.from] / 2.0 + LOOP_ROOM, cy)
                });
                return Route {
                    points: Vec::new(),
                    label,
                };
            }
            let mut points: Vec<(f64, f64)> = Vec::with_capacity(4 * chain.len());
            for pair in chain.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                let (xt, yt) = (end_x(a, a, b), y(a) + nodes[a].height / 2.0);
                let (xb, yb) = (end_x(b, a, b), y(b) - nodes[b].height / 2.0);
                points.push((xt, yt));
                let channel = nodes[a].layer;
                if let Some(t) = track_y[channel][hyper_of[&(a, b)]] {
                    let ty = channel_top[channel] + t;
                    points.push((xt, ty));
                    points.push((xb, ty));
                }
                points.push((xb, yb));
            }
            if reversed[i] {
                points.reverse();
            }
            let label = label_node[i].map(|w| (x[w] - nodes[w].left + 1.0, y(w)));
            Route {
                points: simplify(points),
                label,
            }
        })
        .collect();
    Layout {
        states: (0..n).map(|v| (x[v], y(v))).collect(),
        widths,
        routes,
    }
}

fn waypoint(nodes: &mut Vec<Node>, layer: usize, label: Option<(f64, f64)>) -> usize {
    // A label sits on its edge, centred.
    let (half, height) = label.map_or((0.0, 0.0), |(w, h)| (w / 2.0 + 1.0, h));
    nodes.push(Node {
        layer,
        left: half,
        right: half,
        height,
        state: false,
    });
    nodes.len() - 1
}

/// Drops repeated points and points in the middle of a straight run.
fn simplify(points: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(points.len());
    for p in points {
        if let Some(&last) = out.last()
            && (last.0 - p.0).abs() < 0.05
            && (last.1 - p.1).abs() < 0.05
        {
            continue;
        }
        if out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let vertical = (a.0 - b.0).abs() < 0.05 && (b.0 - p.0).abs() < 0.05;
            let horizontal = (a.1 - b.1).abs() < 0.05 && (b.1 - p.1).abs() < 0.05;
            if vertical || horizontal {
                out.pop();
            }
        }
        out.push(p);
    }
    out
}

/// The track of each segment of a channel (none for a straight one), and
/// the number of tracks. Segments whose spans overlap get different
/// tracks, the one above the other that way that crosses less.
fn tracks(list: &[Hyper]) -> (Vec<Option<usize>>, usize) {
    let m = list.len();
    let bent: Vec<bool> = list.iter().map(|h| h.hi - h.lo > 0.5).collect();
    let overlap = |a: &Hyper, b: &Hyper| a.lo < b.hi + TRACK_SEP && b.lo < a.hi + TRACK_SEP;
    let within = |x: f64, h: &Hyper| x > h.lo - 0.5 && x < h.hi + 0.5;
    // The crossings when `a` runs above `b`; a vertical of `a` running
    // down along one of `b` going up counts as many.
    let cost = |a: &Hyper, b: &Hyper| {
        let mut c = 0;
        for &x in &a.bottoms {
            c += usize::from(within(x, b));
            c += 100 * b.tops.iter().filter(|&&t| (t - x).abs() < 0.5).count();
        }
        c + b.tops.iter().filter(|&&x| within(x, a)).count()
    };
    let mut above: Vec<Vec<usize>> = vec![Vec::new(); m];
    let mut conflicts: Vec<Vec<usize>> = vec![Vec::new(); m];
    for a in 0..m {
        for b in a + 1..m {
            if !bent[a] || !bent[b] || !overlap(&list[a], &list[b]) {
                continue;
            }
            conflicts[a].push(b);
            conflicts[b].push(a);
            let (ab, ba) = (cost(&list[a], &list[b]), cost(&list[b], &list[a]));
            if ab < ba {
                above[b].push(a);
            } else if ba < ab {
                above[a].push(b);
            }
        }
    }
    // Topological order of "above", dropping constraints to break cycles.
    let mut done = vec![false; m];
    let mut order = Vec::with_capacity(m);
    let mut remaining: Vec<usize> = (0..m).filter(|&v| bent[v]).collect();
    remaining.sort_by(|&a, &b| list[a].lo.total_cmp(&list[b].lo));
    while !remaining.is_empty() {
        let pick = remaining
            .iter()
            .position(|&v| above[v].iter().all(|&u| done[u]))
            .unwrap_or_else(|| {
                // A cycle: take the one with the fewest pending constraints.
                (0..remaining.len())
                    .min_by_key(|&k| above[remaining[k]].iter().filter(|&&u| !done[u]).count())
                    .unwrap_or(0)
            });
        let v = remaining.remove(pick);
        done[v] = true;
        order.push(v);
    }
    let mut track: Vec<Option<usize>> = vec![None; m];
    let mut count = 0;
    for &v in &order {
        let mut t = above[v]
            .iter()
            .filter_map(|&u| track[u])
            .map(|t| t + 1)
            .max()
            .unwrap_or(0);
        while conflicts[v].iter().any(|&w| track[w] == Some(t)) {
            t += 1;
        }
        track[v] = Some(t);
        count = count.max(t + 1);
    }
    (track, count)
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

/// Moves the long trunks that many edges join to the side of the diagram
/// they lean to, where they run straight instead of weaving between the
/// branches.
fn to_the_side(
    layers: &mut [Vec<usize>],
    trunks: &BTreeMap<(Net, usize), usize>,
    edges: &[Edge],
    reversed: &[bool],
) {
    let mut joining: BTreeMap<Net, usize> = BTreeMap::new();
    for (i, e) in edges.iter().enumerate() {
        if e.from != e.to {
            *joining.entry((e.to, reversed[i])).or_default() += 1;
        }
    }
    let mut runs: BTreeMap<Net, Vec<(usize, usize)>> = BTreeMap::new();
    for (&(net, layer), &v) in trunks {
        runs.entry(net).or_default().push((layer, v));
    }
    let mut runs: Vec<Vec<(usize, usize)>> = runs
        .into_iter()
        .filter(|(net, run)| run.len() >= MIN_SIDE_TRUNK && joining[net] >= MIN_SIDE_EDGES)
        .map(|(_, run)| run)
        .collect();
    // Shorter trunks go inside the longer ones.
    runs.sort_by_key(|run| run.len());
    for run in runs {
        let lean = run
            .iter()
            .map(|&(l, v)| {
                let layer = &layers[l];
                let i = layer.iter().position(|&w| w == v).unwrap_or(0);
                (i as f64 + 0.5) / layer.len() as f64
            })
            .sum::<f64>()
            / run.len() as f64;
        for (l, v) in run {
            let layer = &mut layers[l];
            layer.retain(|&w| w != v);
            if lean < 0.5 {
                layer.insert(0, v);
            } else {
                layer.push(v);
            }
        }
    }
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

/// `v` within `lo..=hi`, or `hi` when they are out of order (rounding).
fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    v.max(lo).min(hi)
}

/// The least distance between the centres of `a` and `b`, `a` left of
/// `b` in the same layer.
fn gap(nodes: &[Node], a: usize, b: usize) -> f64 {
    let spacing = if nodes[a].state || nodes[b].state {
        STATE_GAP
    } else {
        WAYPOINT_GAP
    };
    nodes[a].right + spacing + nodes[b].left
}

/// Lines up the waypoints of each trunk, and the waypoint of each label
/// with its source, wherever the spacing of their layers allows, so that
/// edges run straight instead of stepping sideways at every layer.
fn straighten(
    nodes: &[Node],
    layers: &[Vec<usize>],
    x: &mut [f64],
    down: &[Vec<(usize, f64)>],
    up: &[Vec<(usize, f64)>],
    trunks: &BTreeMap<(Net, usize), usize>,
    widths: &[f64],
) {
    let mut runs: BTreeMap<Net, Vec<usize>> = BTreeMap::new();
    for (&(net, _), &v) in trunks {
        runs.entry(net).or_default().push(v);
    }
    let mut runs: Vec<Vec<usize>> = runs.into_values().collect();
    let in_trunk: std::collections::BTreeSet<usize> = trunks.values().copied().collect();
    runs.extend(
        (0..nodes.len())
            .filter(|&v| !nodes[v].state && !in_trunk.contains(&v))
            .map(|v| vec![v]),
    );
    runs.sort_by_key(|run| std::cmp::Reverse(run.len()));
    let mut neighbours: Vec<(Option<usize>, Option<usize>)> = vec![(None, None); nodes.len()];
    for layer in layers {
        for (i, &v) in layer.iter().enumerate() {
            neighbours[v] = (
                i.checked_sub(1).map(|j| layer[j]),
                layer.get(i + 1).copied(),
            );
        }
    }
    // The single state an end of a run is linked to.
    let end_state = |list: &[(usize, f64)]| match list {
        [(s, _)] if nodes[*s].state => Some(*s),
        _ => None,
    };
    for _ in 0..4 {
        for run in &runs {
            let feasible = |v: usize, x: &[f64]| {
                let (left, right) = neighbours[v];
                (
                    left.map_or(f64::MIN, |u| x[u] + gap(nodes, u, v)),
                    right.map_or(f64::MAX, |w| x[w] - gap(nodes, v, w)),
                )
            };
            let mut start = 0;
            let mut previous = None;
            while start < run.len() {
                let (mut lo, mut hi) = feasible(run[start], x);
                let mut end = start + 1;
                while end < run.len() {
                    let (l, h) = feasible(run[end], x);
                    if lo.max(l) > hi.min(h) {
                        break;
                    }
                    (lo, hi) = (lo.max(l), hi.min(h));
                    end += 1;
                }
                let part = &run[start..end];
                // Where the run cannot go straight, it steps as little as
                // it can.
                let mean = part.iter().map(|&v| x[v]).sum::<f64>() / part.len() as f64;
                let mut target = clamp(previous.unwrap_or(mean), lo, hi);
                // Straight into the state at the end, then out of the one
                // at the start.
                let ends = [
                    (end == run.len()).then(|| end_state(&down[run[end - 1]])),
                    (start == 0).then(|| end_state(&up[run[0]])),
                ];
                for s in ends.into_iter().flatten().flatten() {
                    let half = widths[s] / 2.0 - CORNER;
                    let (l, h) = (lo.max(x[s] - half), hi.min(x[s] + half));
                    if l <= h {
                        target = clamp(target, l, h);
                        (lo, hi) = (l, h);
                    }
                }
                for &v in part {
                    x[v] = target;
                }
                previous = Some(target);
                start = end;
            }
        }
    }
}

/// Moves each node, within the room its layer leaves it, to where most of
/// its links run straight down: to the x of a waypoint, or anywhere a port
/// of a state can sit.
fn snap(
    nodes: &[Node],
    layers: &[Vec<usize>],
    x: &mut [f64],
    down: &[Vec<(usize, f64)>],
    up: &[Vec<(usize, f64)>],
    widths: &[f64],
) {
    let half = |v: usize| {
        if nodes[v].state {
            (widths[v] / 2.0 - CORNER).max(0.0)
        } else {
            0.0
        }
    };
    for _ in 0..3 {
        for layer in layers {
            for (i, &v) in layer.iter().enumerate() {
                let lo = i
                    .checked_sub(1)
                    .map_or(f64::MIN, |j| x[layer[j]] + gap(nodes, layer[j], v));
                let hi = layer
                    .get(i + 1)
                    .map_or(f64::MAX, |&w| x[w] - gap(nodes, v, w));
                if lo > hi {
                    continue;
                }
                let links = || down[v].iter().chain(&up[v]);
                // The score of `p`: the weight of the links straight there.
                let score = |p: f64, x: &[f64]| -> f64 {
                    links()
                        .filter(|&&(n, _)| (p - x[n]).abs() <= half(v) + half(n) + 0.01)
                        .map(|&(_, w)| w)
                        .sum()
                };
                let current = x[v];
                let mut best = (score(current, x), 0.0, current);
                for &(n, _) in links() {
                    let reach = half(v) + half(n);
                    let p = clamp(clamp(current, x[n] - reach, x[n] + reach), lo, hi);
                    let candidate = (score(p, x), -(p - current).abs(), p);
                    if candidate.0 > best.0 || (candidate.0 == best.0 && candidate.1 > best.1) {
                        best = candidate;
                    }
                }
                x[v] = best.2;
            }
        }
    }
}

/// The x of every node.
fn place(
    nodes: &[Node],
    layers: &[Vec<usize>],
    down: &[Vec<(usize, f64)>],
    up: &[Vec<(usize, f64)>],
) -> Vec<f64> {
    let gap = |a: usize, b: usize| gap(nodes, a, b);
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

    #[test]
    fn edges_into_a_state_merge_and_run_orthogonally() {
        // IDLE -> A -> B -> C, and every state back to IDLE and on to ERR.
        let sizes = vec![(60.0, 30.0); 5];
        let mut edges = vec![edge(0, 1), edge(1, 2), edge(2, 3)];
        for v in 1..4 {
            edges.push(edge(v, 0));
            edges.push(Edge {
                from: v,
                to: 4,
                label: Some((30.0, 14.0)),
            });
        }
        let l = layout(&sizes, &edges, Some(0));
        for (e, r) in edges.iter().zip(&l.routes) {
            for p in r.points.windows(2) {
                let (a, b) = (p[0], p[1]);
                assert!(
                    (a.0 - b.0).abs() < 1e-6 || (a.1 - b.1).abs() < 1e-6,
                    "{e:?} has a slanted segment"
                );
            }
        }
        // One arrowhead per state and side.
        let ends = |to: usize| {
            let mut ends: Vec<(i64, i64)> = edges
                .iter()
                .zip(&l.routes)
                .filter(|(e, _)| e.to == to && e.from != to)
                .map(|(_, r)| {
                    let p = r.points.last().unwrap();
                    (p.0.round() as i64, p.1.round() as i64)
                })
                .collect();
            ends.dedup();
            ends
        };
        assert_eq!(ends(0).len(), 1);
        assert_eq!(ends(4).len(), 1);
        assert!(l.routes.iter().all(|r| r.points.len() >= 2));
        assert_eq!(l.routes.iter().filter(|r| r.label.is_some()).count(), 3);
    }
}
