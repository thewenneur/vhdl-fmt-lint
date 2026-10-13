//! Block diagrams: the instances of an architecture as blocks, wired by
//! the signals between their ports, with the entity's ports as terminals.
//!
//! Blocks are placed in columns from left to right, each after the blocks
//! that drive its inputs (feedback is ignored for the placement), and
//! ordered in their column to shorten the wires. A signal from one column
//! to the next is drawn as a wire in the channel between them; every other
//! connection (to a farther column, backwards, or without a known driver)
//! ends in a short stub labelled with the signal's name, as net labels do
//! in a schematic.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use vfl_syntax::identifier_key;

use crate::hierarchy::resolve;
use crate::model::{Design, Direction, Interface};
use crate::svg::{Canvas, cut, escape, width};

/// Past this many instances a block diagram is too large to read.
pub const MAX_BLOCKS: usize = 80;

const ROW: f64 = 18.0;
const HEAD: f64 = 36.0;
const PIN: f64 = 10.0;
const GAP: f64 = 28.0;
const TRACK: f64 = 9.0;
const STUB: f64 = 14.0;
const NAME_MAX: usize = 24;

/// What a pin connects to.
#[derive(Debug, Clone, PartialEq)]
enum Actual {
    /// A signal or port, by key, with its spelling.
    Net(String, String),
    /// An expression or literal.
    Value(String),
    Open,
}

#[derive(Debug, Clone)]
struct Pin {
    name: String,
    /// Drawn on the right side.
    right: bool,
    /// Drives its net (an output, or an entity input as a terminal).
    drives: bool,
    actual: Actual,
    y: f64,
}

#[derive(Debug, Clone)]
struct Node {
    /// `None` for a terminal (an entity port).
    title: Option<(String, String)>,
    entity: Option<usize>,
    pins: Vec<Pin>,
    layer: usize,
    w: f64,
    h: f64,
    x: f64,
    y: f64,
}

impl Node {
    fn is_terminal(&self) -> bool {
        self.title.is_none()
    }

    fn pin_x(&self, pin: &Pin) -> f64 {
        if pin.right {
            self.x + self.w + if self.is_terminal() { 0.0 } else { PIN }
        } else {
            self.x - if self.is_terminal() { 0.0 } else { PIN }
        }
    }
}

/// The leading identifier of a name, as key and spelling: `data` for
/// `data(3)` or `data.field`. `None` for literals and expressions.
pub(crate) fn base(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    let end = text
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    let ident = &text[..end];
    let first = ident.chars().next()?;
    if !first.is_alphabetic() {
        return None;
    }
    let rest = text[end..].trim_start();
    if !(rest.is_empty() || rest.starts_with('(') || rest.starts_with('.')) {
        return None;
    }
    Some((identifier_key(ident), ident.to_owned()))
}

/// The ports of what an instance binds to: the entity, else a component
/// declaration in the architecture or in a package.
pub(crate) fn ports_of<'a>(
    design: &'a Design,
    entity: usize,
    arch: usize,
    instance: &crate::model::Instance,
) -> (Option<usize>, Option<&'a [Interface]>) {
    let library = &design.entities[entity].library;
    if let Some(target) = resolve(design, instance, library) {
        return (Some(target), Some(&design.entities[target].ports));
    }
    let key = identifier_key(&instance.unit);
    let component = design.entities[entity].architectures[arch]
        .components
        .iter()
        .chain(design.packages.iter().flat_map(|p| p.components.iter()))
        .find(|c| identifier_key(&c.name) == key);
    (None, component.map(|c| c.ports.as_slice()))
}

fn actual_of(text: &str) -> Actual {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("open") {
        return Actual::Open;
    }
    match base(trimmed) {
        Some((key, spelling)) => Actual::Net(key, spelling),
        None => Actual::Value(trimmed.to_owned()),
    }
}

/// The block diagram of one architecture of an entity; `None` when it has
/// no instances or too many. `link` gives the page of a documented entity.
pub fn block(
    design: &Design,
    entity: usize,
    arch: usize,
    link: Option<&dyn Fn(usize) -> String>,
) -> Option<String> {
    let e = &design.entities[entity];
    let a = e.architectures.get(arch)?;
    if a.instances.is_empty() || a.instances.len() > MAX_BLOCKS {
        return None;
    }
    let (nodes, terminals) = base_nodes(design, entity, arch);
    Some(draw(
        nodes,
        terminals,
        link,
        &format!("Block diagram of {}({})", e.name, a.name),
    ))
}

/// The data flow of one architecture: its processes, concurrent
/// assignments and instances as blocks, wired by the signals they read and
/// drive. `None` when it has no process or assignment, or too many blocks.
/// Clocks and resets are written under the process name rather than wired.
pub fn dataflow(
    design: &Design,
    entity: usize,
    arch: usize,
    link: Option<&dyn Fn(usize) -> String>,
) -> Option<String> {
    let e = &design.entities[entity];
    let a = e.architectures.get(arch)?;
    let blocks = a.instances.len() + a.processes.len() + a.assignments.len();
    if (a.processes.is_empty() && a.assignments.is_empty()) || blocks > MAX_BLOCKS {
        return None;
    }
    let (mut nodes, terminals) = base_nodes(design, entity, arch);
    let known: HashSet<String> = e
        .ports
        .iter()
        .map(|p| identifier_key(&p.name))
        .chain(a.signals.iter().map(|s| identifier_key(&s.name)))
        .collect();
    for p in &a.processes {
        let mut skip: Vec<String> = Vec::new();
        let mut detail = Vec::new();
        if let Some(clock) = &p.clock {
            skip.push(identifier_key(clock));
            detail.push(format!("{clock} {}", if p.falling { "↓" } else { "↑" }));
        }
        if let Some(reset) = &p.reset {
            skip.push(identifier_key(&reset.signal));
            detail.push(format!(
                "{} {}",
                reset.signal,
                if reset.asynchronous { "async" } else { "sync" }
            ));
        }
        if detail.is_empty() {
            detail.push("combinational".into());
        }
        let title = p
            .label
            .clone()
            .unwrap_or_else(|| format!("process @{}", p.line));
        nodes.push(logic_node(
            title,
            detail.join(", "),
            &a.resolve_aliases(&p.reads),
            &p.drives,
            &known,
            &skip,
        ));
    }
    for s in &a.assignments {
        let title = s
            .label
            .clone()
            .unwrap_or_else(|| format!("{} <=", s.target));
        nodes.push(logic_node(
            title,
            "assignment".into(),
            &a.resolve_aliases(&s.reads),
            std::slice::from_ref(&s.target),
            &known,
            &[],
        ));
    }
    // Ports only clocks and resets use stay out.
    let used: HashSet<String> = nodes[terminals..]
        .iter()
        .flat_map(|n| n.pins.iter())
        .filter_map(|p| match &p.actual {
            Actual::Net(key, _) => Some(key.clone()),
            _ => None,
        })
        .collect();
    let mut kept = Vec::with_capacity(nodes.len());
    let mut kept_terminals = 0;
    for (i, node) in nodes.into_iter().enumerate() {
        if i < terminals {
            if !matches!(&node.pins[0].actual, Actual::Net(key, _) if used.contains(key)) {
                continue;
            }
            kept_terminals += 1;
        }
        kept.push(node);
    }
    Some(draw(
        kept,
        kept_terminals,
        link,
        &format!("Data flow of {}({})", e.name, a.name),
    ))
}

/// A process or assignment block: the signals it reads on the left and
/// those it drives on the right, unnamed (the wires carry the names).
fn logic_node(
    title: String,
    detail: String,
    reads: &[String],
    drives: &[String],
    known: &HashSet<String>,
    skip: &[String],
) -> Node {
    let mut pins = Vec::new();
    let driven: Vec<String> = drives.iter().map(|d| identifier_key(d)).collect();
    for name in reads {
        let key = identifier_key(name);
        if known.contains(&key) && !skip.contains(&key) && !driven.contains(&key) {
            pins.push(Pin {
                name: String::new(),
                right: false,
                drives: false,
                actual: Actual::Net(key, name.clone()),
                y: 0.0,
            });
        }
    }
    for (name, key) in drives.iter().zip(driven) {
        if known.contains(&key) {
            pins.push(Pin {
                name: String::new(),
                right: true,
                drives: true,
                actual: Actual::Net(key, name.clone()),
                y: 0.0,
            });
        }
    }
    let left = pins.iter().filter(|p| !p.right).count();
    let right = pins.len() - left;
    let w = (width(&cut(&title, 40)).max(width(&cut(&detail, 40))) + 16.0).max(60.0);
    let h = HEAD + left.max(right).max(1) as f64 * ROW + 6.0;
    Node {
        title: Some((title, detail)),
        entity: None,
        pins,
        layer: 1,
        w,
        h,
        x: 0.0,
        y: 0.0,
    }
}

/// The entity's ports as terminals, then a block per instance; with the
/// number of terminals.
fn base_nodes(design: &Design, entity: usize, arch: usize) -> (Vec<Node>, usize) {
    let e = &design.entities[entity];
    let a = &e.architectures[arch];
    let mut nodes: Vec<Node> = Vec::new();

    // The entity's ports, as terminals: inputs drive their net from the
    // left, outputs take theirs on the right.
    let port_keys: Vec<String> = e.ports.iter().map(|p| identifier_key(&p.name)).collect();
    for port in &e.ports {
        let input = port.direction() == Direction::In;
        nodes.push(Node {
            title: None,
            entity: None,
            pins: vec![Pin {
                name: port.name.clone(),
                right: input,
                drives: input,
                actual: Actual::Net(identifier_key(&port.name), port.name.clone()),
                y: 0.0,
            }],
            layer: 0,
            w: width(&cut(&port.name, NAME_MAX)) + 16.0,
            h: 20.0,
            x: 0.0,
            y: 0.0,
        });
    }
    let terminals = nodes.len();

    for instance in &a.instances {
        let (target, ports) = ports_of(design, entity, arch, instance);
        let mut pins: Vec<Pin> = ports
            .unwrap_or_default()
            .iter()
            .map(|p| Pin {
                name: p.name.clone(),
                right: p.direction() != Direction::In,
                drives: p.direction() == Direction::Out,
                actual: Actual::Open,
                y: 0.0,
            })
            .collect();
        let known = !pins.is_empty();
        for (k, (formal, actual)) in instance.port_map.iter().enumerate() {
            let actual = actual_of(actual);
            let index = if formal.is_empty() {
                (k < pins.len()).then_some(k)
            } else {
                let key = base(formal).map(|(k, _)| k);
                pins.iter()
                    .position(|p| Some(identifier_key(&p.name)) == key)
            };
            match index {
                Some(i) => pins[i].actual = actual,
                None => {
                    // An unknown port: guess its side from the entity port
                    // it connects to, if any.
                    let output = matches!(&actual, Actual::Net(key, _)
                        if port_keys.iter().position(|p| p == key)
                            .is_some_and(|i| e.ports[i].direction() != Direction::In));
                    pins.push(Pin {
                        name: if formal.is_empty() {
                            format!("#{}", k + 1)
                        } else {
                            formal.clone()
                        },
                        right: output && !known,
                        drives: false,
                        actual,
                        y: 0.0,
                    });
                }
            }
        }
        let left = pins.iter().filter(|p| !p.right).count();
        let right = pins.len() - left;
        let name_w = |right: bool| {
            pins.iter()
                .filter(|p| p.right == right)
                .map(|p| width(&cut(&p.name, NAME_MAX)))
                .fold(0.0, f64::max)
        };
        let title = (instance.path(), instance.unit.clone());
        let w = (name_w(false) + name_w(true) + 28.0)
            .max(width(&cut(&title.0, 40)) + 16.0)
            .max(width(&cut(&title.1, 40)) + 16.0)
            .max(80.0);
        let h = HEAD + left.max(right).max(1) as f64 * ROW + 6.0;
        nodes.push(Node {
            title: Some(title),
            entity: target,
            pins,
            layer: 1,
            w,
            h,
            x: 0.0,
            y: 0.0,
        });
    }
    (nodes, terminals)
}

/// Lays out and draws blocks and terminals (the first `terminals` nodes),
/// wired by their nets.
fn draw(
    mut nodes: Vec<Node>,
    terminals: usize,
    link: Option<&dyn Fn(usize) -> String>,
    title: &str,
) -> String {
    // Nets: key -> (node, pin) of every pin on it.
    let mut nets: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    let mut spelling: HashMap<String, String> = HashMap::new();
    for (n, node) in nodes.iter().enumerate() {
        for (p, pin) in node.pins.iter().enumerate() {
            if let Actual::Net(key, name) = &pin.actual {
                nets.entry(key.clone()).or_default().push((n, p));
                spelling.entry(key.clone()).or_insert_with(|| name.clone());
            }
        }
    }
    let driver = |members: &[(usize, usize)], nodes: &[Node]| -> Option<(usize, usize)> {
        members
            .iter()
            .copied()
            .find(|&(n, p)| nodes[n].pins[p].drives)
    };

    // Columns: each block after the blocks driving it, feedback ignored.
    let blocks = nodes.len() - terminals;
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for members in nets.values() {
        if let Some((d, _)) = driver(members, &nodes) {
            for &(n, p) in members {
                if n != d && n >= terminals && !nodes[n].pins[p].drives && d >= terminals {
                    successors[d].push(n);
                }
            }
        }
    }
    // Depth-first order, dropping the edges that close a cycle.
    let mut state = vec![0u8; nodes.len()];
    let mut order = Vec::with_capacity(blocks);
    let mut forward: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for start in terminals..nodes.len() {
        if state[start] != 0 {
            continue;
        }
        let mut stack = vec![(start, 0usize)];
        state[start] = 1;
        while let Some((n, i)) = stack.pop() {
            if let Some(&next) = successors[n].get(i) {
                stack.push((n, i + 1));
                match state[next] {
                    0 => {
                        forward[n].push(next);
                        state[next] = 1;
                        stack.push((next, 0));
                    }
                    2 => forward[n].push(next),
                    _ => {}
                }
            } else {
                state[n] = 2;
                order.push(n);
            }
        }
    }
    for &n in order.iter().rev() {
        let layer = nodes[n].layer;
        for &next in &forward[n] {
            nodes[next].layer = nodes[next].layer.max(layer + 1);
        }
    }
    let last_block_layer = nodes[terminals..]
        .iter()
        .map(|n| n.layer)
        .max()
        .unwrap_or(1);
    // Output terminals go right of the block driving them.
    for t in 0..terminals {
        if nodes[t].pins[0].drives {
            continue;
        }
        let key = match &nodes[t].pins[0].actual {
            Actual::Net(key, _) => key.clone(),
            _ => continue,
        };
        let layer = nets
            .get(&key)
            .and_then(|m| driver(m, &nodes))
            .filter(|&(d, _)| d >= terminals)
            .map(|(d, _)| nodes[d].layer + 1)
            .unwrap_or(last_block_layer + 1);
        nodes[t].layer = layer;
    }
    let columns = nodes.iter().map(|n| n.layer).max().unwrap_or(0) + 1;

    // Order each column by the mean position of its neighbours.
    let mut column: Vec<Vec<usize>> = vec![Vec::new(); columns];
    for (n, node) in nodes.iter().enumerate() {
        column[node.layer].push(n);
    }
    let neighbours: Vec<Vec<usize>> = {
        let mut list = vec![Vec::new(); nodes.len()];
        for members in nets.values() {
            for &(a, _) in members {
                for &(b, _) in members {
                    if a != b && !list[a].contains(&b) {
                        list[a].push(b);
                    }
                }
            }
        }
        list
    };
    for sweep in 0..4 {
        let mut rank = vec![0.0; nodes.len()];
        for col in &column {
            for (i, &n) in col.iter().enumerate() {
                rank[n] = i as f64 / col.len().max(1) as f64;
            }
        }
        let range: Vec<usize> = if sweep % 2 == 0 {
            (1..columns).collect()
        } else {
            (0..columns.saturating_sub(1)).rev().collect()
        };
        for c in range {
            let adjacent = |n: usize| -> Option<f64> {
                let near: Vec<f64> = neighbours[n]
                    .iter()
                    .filter(|&&m| {
                        let l = nodes[m].layer;
                        if sweep % 2 == 0 {
                            l + 1 == c
                        } else {
                            l == c + 1
                        }
                    })
                    .map(|&m| rank[m])
                    .collect();
                (!near.is_empty()).then(|| near.iter().sum::<f64>() / near.len() as f64)
            };
            let mut keyed: Vec<(f64, usize, usize)> = column[c]
                .iter()
                .enumerate()
                .map(|(i, &n)| (adjacent(n).unwrap_or(rank[n]), i, n))
                .collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            column[c] = keyed.into_iter().map(|(_, _, n)| n).collect();
            for (i, &n) in column[c].iter().enumerate() {
                rank[n] = i as f64 / column[c].len().max(1) as f64;
            }
        }
    }

    // Vertical positions, with each column centred on the tallest.
    let heights: Vec<f64> = column
        .iter()
        .map(|col| col.iter().map(|&n| nodes[n].h + GAP).sum::<f64>())
        .collect();
    let tallest = heights.iter().copied().fold(0.0, f64::max);
    for (c, col) in column.iter().enumerate() {
        let mut y = (tallest - heights[c]) / 2.0;
        for &n in col {
            nodes[n].y = y;
            y += nodes[n].h + GAP;
        }
    }
    for node in &mut nodes {
        let y0 = node.y;
        if node.is_terminal() {
            node.pins[0].y = y0 + node.h / 2.0;
            continue;
        }
        let mut left = 0;
        let mut right = 0;
        for pin in &mut node.pins {
            let row = if pin.right { &mut right } else { &mut left };
            pin.y = y0 + HEAD + *row as f64 * ROW + ROW / 2.0;
            *row += 1;
        }
    }

    // The nets routed in the channel right of each column: those whose
    // driver is in the column and which have a sink in the next one.
    let mut routed: Vec<Vec<String>> = vec![Vec::new(); columns];
    let mut keys: Vec<&String> = nets.keys().collect();
    keys.sort();
    for key in keys {
        let members = &nets[key];
        let Some((d, _)) = driver(members, &nodes) else {
            continue;
        };
        let layer = nodes[d].layer;
        if members.iter().any(|&(n, p)| {
            nodes[n].layer == layer + 1 && !nodes[n].pins[p].drives && !nodes[n].pins[p].right
        }) {
            routed[layer].push(key.clone());
        }
    }
    for list in &mut routed {
        list.sort_by(|a, b| {
            let y = |k: &String| {
                let (d, p) = driver(&nets[k], &nodes).unwrap_or((0, 0));
                nodes[d].pins[p].y
            };
            y(a).total_cmp(&y(b))
        });
    }

    // Horizontal positions: column widths, then channels wide enough for
    // the wire labels and tracks.
    let mut x = 0.0;
    let mut track_x: HashMap<String, f64> = HashMap::new();
    for c in 0..columns {
        let col_w = column[c].iter().map(|&n| nodes[n].w).fold(0.0, f64::max);
        for &n in &column[c] {
            // Terminals hug the channel they connect through.
            nodes[n].x = if nodes[n].is_terminal() && nodes[n].pins[0].right {
                x + col_w - nodes[n].w
            } else {
                x
            };
        }
        x += col_w;
        let labels = routed[c]
            .iter()
            .map(|k| width(&cut(&spelling[k], NAME_MAX)))
            .fold(0.0, f64::max);
        let stubs = |right: bool, col: &[usize]| {
            col.iter()
                .flat_map(|&n| nodes[n].pins.iter())
                .filter(|p| p.right == right)
                .map(|p| match &p.actual {
                    Actual::Net(_, s) => width(&cut(s, NAME_MAX)),
                    Actual::Value(v) => width(&cut(v, NAME_MAX)),
                    Actual::Open => 0.0,
                })
                .fold(0.0, f64::max)
        };
        let next = column
            .get(c + 1)
            .map(|col| stubs(false, col))
            .unwrap_or(0.0);
        let start = x + PIN + (labels + 8.0).max(STUB + 3.0 + stubs(true, &column[c])) + 12.0;
        for (i, key) in routed[c].iter().enumerate() {
            track_x.insert(key.clone(), start + i as f64 * TRACK);
        }
        let tracks_end = start + routed[c].len() as f64 * TRACK;
        x = tracks_end.max(x + PIN + STUB + 8.0 + stubs(true, &column[c]))
            + next
            + STUB
            + PIN
            + 16.0;
    }

    // Drawing.
    let mut c = Canvas::new();
    for node in &nodes {
        match &node.title {
            None => {
                let pin = &node.pins[0];
                c.rect(node.x, node.y, node.w, node.h, "vfl-term");
                c.text(
                    node.x + node.w / 2.0,
                    node.y + 14.0,
                    &cut(&pin.name, NAME_MAX),
                    "middle",
                    "",
                );
            }
            Some((label, unit)) => {
                let href = node.entity.and_then(|e| link.map(|l| l(e)));
                c.rect(node.x, node.y, node.w, node.h, "vfl-box");
                c.rect(node.x, node.y, node.w, HEAD, "vfl-head");
                c.text(
                    node.x + node.w / 2.0,
                    node.y + 15.0,
                    &cut(label, 40),
                    "middle",
                    "vfl-title",
                );
                if let Some(href) = &href {
                    c.raw(&format!(r#"<a href="{}">"#, escape(href)));
                }
                c.text(
                    node.x + node.w / 2.0,
                    node.y + 30.0,
                    &cut(unit, 40),
                    "middle",
                    "vfl-muted",
                );
                if href.is_some() {
                    c.raw("</a>");
                }
                for pin in &node.pins {
                    let (x0, x1, tx, anchor) = if pin.right {
                        (
                            node.x + node.w,
                            node.x + node.w + PIN,
                            node.x + node.w - 5.0,
                            "end",
                        )
                    } else {
                        (node.x - PIN, node.x, node.x + 5.0, "start")
                    };
                    let class = if pin.actual == Actual::Open {
                        "vfl-muted"
                    } else {
                        ""
                    };
                    c.line(x0, pin.y, x1, pin.y, "vfl-pin");
                    c.text(tx, pin.y + 4.0, &cut(&pin.name, NAME_MAX), anchor, class);
                }
            }
        }
    }

    let mut wired: Vec<Vec<bool>> = nodes.iter().map(|n| vec![false; n.pins.len()]).collect();
    for (col, list) in routed.iter().enumerate() {
        for key in list {
            let members = &nets[key];
            let Some((d, dp)) = driver(members, &nodes) else {
                continue;
            };
            let tx = track_x[key];
            let from = &nodes[d];
            let (fx, fy) = (from.pin_x(&from.pins[dp]), from.pins[dp].y);
            let sinks: Vec<(f64, f64, usize, usize)> = members
                .iter()
                .filter(|&&(n, p)| {
                    nodes[n].layer == col + 1 && !nodes[n].pins[p].drives && !nodes[n].pins[p].right
                })
                .map(|&(n, p)| (nodes[n].pin_x(&nodes[n].pins[p]), nodes[n].pins[p].y, n, p))
                .collect();
            let mut path = format!("M{fx:.1},{fy:.1}H{tx:.1}");
            let low = sinks.iter().map(|s| s.1).fold(fy, f64::min);
            let high = sinks.iter().map(|s| s.1).fold(fy, f64::max);
            if high > low {
                let _ = write!(path, "M{tx:.1},{low:.1}V{high:.1}");
            }
            for &(sx, sy, n, p) in &sinks {
                let _ = write!(path, "M{tx:.1},{sy:.1}H{sx:.1}");
                wired[n][p] = true;
            }
            wired[d][dp] = true;
            c.extend(fx, low, tx, high);
            c.raw(&format!(
                r#"<g><title>{}</title><path class="vfl-wire" d="{path}"/>"#,
                escape(&spelling[key])
            ));
            // Junctions where the wire branches.
            let ends: Vec<f64> = sinks.iter().map(|s| s.1).chain([fy]).collect();
            for &y in &ends {
                if y > low && y < high {
                    c.raw(&format!(
                        r#"<circle class="vfl-dot" cx="{tx:.1}" cy="{y:.1}" r="2.5"/>"#
                    ));
                }
            }
            c.raw("</g>");
            if !from.is_terminal() {
                c.text(
                    fx + 4.0,
                    fy - 4.0,
                    &cut(&spelling[key], NAME_MAX),
                    "start",
                    "vfl-muted vfl-small",
                );
            }
        }
    }

    // Stubs with net labels for every other connection.
    for (n, node) in nodes.iter().enumerate() {
        for (p, pin) in node.pins.iter().enumerate() {
            if wired[n][p] {
                continue;
            }
            let label = match &pin.actual {
                // A terminal's own name is its label.
                Actual::Net(..) if node.is_terminal() => continue,
                Actual::Net(_, s) => s.clone(),
                Actual::Value(v) => v.clone(),
                Actual::Open => continue,
            };
            let class = if matches!(pin.actual, Actual::Value(_)) {
                "vfl-muted"
            } else {
                ""
            };
            let x0 = node.pin_x(pin);
            let (x1, tx, anchor) = if pin.right {
                (x0 + STUB, x0 + STUB + 3.0, "start")
            } else {
                (x0 - STUB, x0 - STUB - 3.0, "end")
            };
            c.raw(&format!(
                r#"<path class="vfl-wire" d="M{x0:.1},{:.1}H{x1:.1}"/>"#,
                pin.y
            ));
            c.text(tx, pin.y + 4.0, &cut(&label, NAME_MAX), anchor, class);
        }
    }
    c.finish(title, 10.0)
}

/// The port map of one instance of an architecture: the block of the bound
/// entity or component, with the actual of each port at the end of its pin
/// and the generic map under the title. Ports of an unknown unit are drawn
/// from the association list, as inputs.
pub fn port_map(design: &Design, entity: usize, arch: usize, instance: usize) -> Option<String> {
    let instance = design.entities[entity]
        .architectures
        .get(arch)?
        .instances
        .get(instance)?;
    let (target, ports) = ports_of(design, entity, arch, instance);
    let mut ports: Vec<Interface> = ports.map(<[Interface]>::to_vec).unwrap_or_default();
    let known = !ports.is_empty();
    let mut actuals: Vec<Option<String>> = vec![None; ports.len()];
    for (k, (formal, actual)) in instance.port_map.iter().enumerate() {
        let index = if formal.is_empty() {
            (k < ports.len()).then_some(k)
        } else {
            let key = base(formal).map(|(k, _)| k);
            ports
                .iter()
                .position(|p| Some(identifier_key(&p.name)) == key)
        };
        let actual = compact_actual(actual);
        match index {
            // `slice(0) => a, slice(1) => b`: one pin with every actual.
            Some(i)
                if known
                    && !formal.is_empty()
                    && base(formal).is_some_and(|(_, s)| s != formal.trim()) =>
            {
                let part = format!("{} => {actual}", formal.trim());
                actuals[i] = Some(match actuals[i].take() {
                    Some(before) => format!("{before}, {part}"),
                    None => part,
                });
            }
            Some(i) => actuals[i] = Some(actual),
            None => {
                ports.push(Interface {
                    name: if formal.is_empty() {
                        format!("#{}", k + 1)
                    } else {
                        formal.trim().to_owned()
                    },
                    mode: String::new(),
                    ty: String::new(),
                    default: None,
                    description: String::new(),
                });
                actuals.push(Some(actual));
            }
        }
    }
    // Generics: the mapped ones as associated, the others with their
    // default, muted.
    let generics: &[Interface] = match target {
        Some(t) => &design.entities[t].generics,
        None => &[],
    };
    let mut generic_lines: Vec<(String, bool)> = Vec::new();
    let mut mapped = vec![false; generics.len()];
    for (k, (formal, actual)) in instance.generic_map.iter().enumerate() {
        let index = if formal.is_empty() {
            (k < generics.len()).then_some(k)
        } else {
            generics
                .iter()
                .position(|g| identifier_key(&g.name) == identifier_key(formal.trim()))
        };
        let name = match index {
            Some(i) => {
                mapped[i] = true;
                generics[i].name.clone()
            }
            None if formal.is_empty() => format!("#{}", k + 1),
            None => formal.trim().to_owned(),
        };
        generic_lines.push((format!("{name} => {}", compact_actual(actual)), false));
    }
    for (g, _) in generics.iter().zip(&mapped).filter(|(_, m)| !**m) {
        if let Some(default) = &g.default {
            generic_lines.push((format!("{} := {default}", g.name), true));
        }
    }
    let actual_of_port = |port: &Interface| {
        let i = ports.iter().position(|p| std::ptr::eq(p, port))?;
        Some(match &actuals[i] {
            Some(a) if !a.eq_ignore_ascii_case("open") => (a.clone(), false),
            Some(_) => ("open".to_owned(), true),
            // An unassociated input takes its default, an output is open.
            None => match &port.default {
                Some(d) if port.direction() == Direction::In => (format!("default {d}"), true),
                _ => ("open".to_owned(), true),
            },
        })
    };
    let title = instance.path();
    let subtitle = match &instance.architecture {
        Some(a) => format!("{}({a})", instance.unit),
        None => instance.unit.clone(),
    };
    let canvas = crate::svg::draw_symbol(
        &title,
        Some(&subtitle),
        &generic_lines,
        &ports,
        &actual_of_port,
    );
    Some(canvas.finish(&format!("{title} : {subtitle}"), 8.0))
}

/// An actual on one line, with runs of white space collapsed.
fn compact_actual(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
