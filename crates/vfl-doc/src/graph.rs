//! Layered graphs drawn left to right: package dependencies and reset
//! trees.
//!
//! Each node goes one column right of the farthest node with an edge to it
//! (edges that close a cycle are ignored for the placement), and columns
//! are ordered to bring connected nodes level. Edges are curves from the
//! right side of a node to the left side of the next.

use crate::svg::{Canvas, cut, escape, width};

const ROW_GAP: f64 = 12.0;
const COLUMN_GAP: f64 = 64.0;
const MAX_LABEL: usize = 40;

/// A node: a title, an optional muted second line, the box style and a
/// link.
#[derive(Debug, Clone, Default)]
pub struct GraphNode {
    pub title: String,
    pub detail: Option<String>,
    /// `vfl-box`, `vfl-head` or `vfl-term`.
    pub class: &'static str,
    pub link: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphEdge {
    pub from: usize,
    pub to: usize,
}

pub fn layered(nodes: &[GraphNode], edges: &[GraphEdge], title: &str) -> String {
    let n = nodes.len();
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in edges {
        if e.from != e.to && e.from < n && e.to < n && !successors[e.from].contains(&e.to) {
            successors[e.from].push(e.to);
            predecessors[e.to].push(e.from);
        }
    }
    // Depth-first order without the edges that close cycles, then the
    // longest path from a source for each node.
    let mut state = vec![0u8; n];
    let mut order = Vec::with_capacity(n);
    let mut forward: Vec<Vec<usize>> = vec![Vec::new(); n];
    for start in 0..n {
        if state[start] != 0 {
            continue;
        }
        let mut stack = vec![(start, 0usize)];
        state[start] = 1;
        while let Some((v, i)) = stack.pop() {
            if let Some(&next) = successors[v].get(i) {
                stack.push((v, i + 1));
                match state[next] {
                    0 => {
                        forward[v].push(next);
                        state[next] = 1;
                        stack.push((next, 0));
                    }
                    2 => forward[v].push(next),
                    _ => {}
                }
            } else {
                state[v] = 2;
                order.push(v);
            }
        }
    }
    let mut layer = vec![0usize; n];
    for &v in order.iter().rev() {
        for &next in &forward[v] {
            layer[next] = layer[next].max(layer[v] + 1);
        }
    }
    let columns = layer.iter().copied().max().map_or(0, |m| m + 1);
    let mut column: Vec<Vec<usize>> = vec![Vec::new(); columns];
    for v in 0..n {
        column[layer[v]].push(v);
    }
    // Order columns by the mean rank of the neighbours in the column
    // before (then after) them.
    let mut rank = vec![0.0f64; n];
    let set_rank = |column: &[Vec<usize>], rank: &mut [f64]| {
        for col in column {
            for (i, &v) in col.iter().enumerate() {
                rank[v] = i as f64;
            }
        }
    };
    set_rank(&column, &mut rank);
    for sweep in 0..6 {
        let range: Vec<usize> = if sweep % 2 == 0 {
            (1..columns).collect()
        } else {
            (0..columns.saturating_sub(1)).rev().collect()
        };
        for c in range {
            let near = |v: usize| -> Option<f64> {
                let list = if sweep % 2 == 0 {
                    &predecessors[v]
                } else {
                    &successors[v]
                };
                let ranks: Vec<f64> = list
                    .iter()
                    .filter(|&&m| {
                        if sweep % 2 == 0 {
                            layer[m] < c
                        } else {
                            layer[m] > c
                        }
                    })
                    .map(|&m| rank[m])
                    .collect();
                (!ranks.is_empty()).then(|| ranks.iter().sum::<f64>() / ranks.len() as f64)
            };
            let mut keyed: Vec<(f64, usize, usize)> = column[c]
                .iter()
                .enumerate()
                .map(|(i, &v)| (near(v).unwrap_or(rank[v]), i, v))
                .collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            column[c] = keyed.into_iter().map(|(_, _, v)| v).collect();
            set_rank(&column, &mut rank);
        }
    }

    // Sizes and positions.
    let size = |v: usize| -> (f64, f64) {
        let node = &nodes[v];
        let w = width(&cut(&node.title, MAX_LABEL)).max(
            node.detail
                .as_ref()
                .map_or(0.0, |d| width(&cut(d, MAX_LABEL))),
        ) + 16.0;
        let h = if node.detail.is_some() { 36.0 } else { 22.0 };
        (w.max(40.0), h)
    };
    let sizes: Vec<(f64, f64)> = (0..n).map(size).collect();
    let heights: Vec<f64> = column
        .iter()
        .map(|col| col.iter().map(|&v| sizes[v].1 + ROW_GAP).sum::<f64>() - ROW_GAP)
        .collect();
    let tallest = heights.iter().copied().fold(0.0, f64::max);
    let mut pos = vec![(0.0f64, 0.0f64); n];
    let mut x = 0.0;
    for (c, col) in column.iter().enumerate() {
        let col_w = col.iter().map(|&v| sizes[v].0).fold(0.0, f64::max);
        let mut y = (tallest - heights[c]) / 2.0;
        for &v in col {
            pos[v] = (x, y);
            y += sizes[v].1 + ROW_GAP;
        }
        x += col_w + COLUMN_GAP;
    }

    let mut c = Canvas::new();
    for e in edges {
        if e.from >= n || e.to >= n || e.from == e.to {
            continue;
        }
        let (fx, fy) = pos[e.from];
        let (tx, ty) = pos[e.to];
        let (fw, fh) = sizes[e.from];
        let (_, th) = sizes[e.to];
        let (x0, y0) = (fx + fw, fy + fh / 2.0);
        let (x1, y1) = (tx - 2.0, ty + th / 2.0);
        let path = if x1 > x0 {
            let mid = (x1 - x0) / 2.0;
            format!(
                "M{x0:.1},{y0:.1}C{:.1},{y0:.1} {:.1},{y1:.1} {x1:.1},{y1:.1}",
                x0 + mid,
                x1 - mid
            )
        } else {
            // Backwards: loop around below.
            let low = y0.max(y1) + 30.0;
            format!(
                "M{x0:.1},{y0:.1}C{:.1},{low:.1} {:.1},{low:.1} {x1:.1},{y1:.1}",
                x0 + 40.0,
                x1 - 40.0
            )
        };
        c.extend(x0, y0.min(y1), x1, y0.max(y1));
        c.raw(&format!(
            r#"<path class="vfl-edge" d="{path}" marker-end="url(#vfl-arrow)"/>"#
        ));
    }
    for v in 0..n {
        let node = &nodes[v];
        let (x, y) = pos[v];
        let (w, h) = sizes[v];
        if let Some(link) = &node.link {
            c.raw(&format!(r#"<a href="{}">"#, escape(link)));
        }
        c.rect(
            x,
            y,
            w,
            h,
            if node.class.is_empty() {
                "vfl-box"
            } else {
                node.class
            },
        );
        let title_class = if node.class == "vfl-term" {
            ""
        } else {
            "vfl-title"
        };
        c.text(
            x + w / 2.0,
            y + 15.0,
            &cut(&node.title, MAX_LABEL),
            "middle",
            title_class,
        );
        if let Some(detail) = &node.detail {
            c.text(
                x + w / 2.0,
                y + 30.0,
                &cut(detail, MAX_LABEL),
                "middle",
                "vfl-muted",
            );
        }
        if node.link.is_some() {
            c.raw("</a>");
        }
    }
    c.finish(title, 10.0)
}
