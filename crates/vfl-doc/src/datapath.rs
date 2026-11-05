//! Data path diagrams: what feeds what in an architecture, drawn top to
//! bottom with the layered layout of the state diagrams.
//!
//! Ports are terminals, signals boxes (registers with a clock mark),
//! instances blocks with a header, and operators and functions rounded
//! boxes with their name (`+ 1` when the other operand is a constant).
//! Only data is drawn: constants, conditions and selects are left out, a
//! multiplexer only appears where two or more signals meet, and type
//! conversions are wires.

use std::collections::{BTreeSet, HashMap, HashSet};

use vfl_syntax::identifier_key;

use crate::block::{base, ports_of};
use crate::layered;
use crate::logic::{Expr, MAX_NODES, is_conversion};
use crate::model::{Design, Direction};
use crate::svg::{Canvas, cut, escape, rounded, width};

/// Past this many nodes a data path is too large to read.
pub const MAX_DATAPATH: usize = 200;

const HEIGHT: f64 = 28.0;
const OP_HEIGHT: f64 = 24.0;
const LABEL: usize = 32;

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Input,
    Output,
    Signal,
    /// A register and its clock.
    Register(String, bool),
    /// An instance: label, unit and the entity it binds to.
    Instance(String, Option<usize>),
    Op,
}

struct Graph<'a> {
    known: &'a HashSet<String>,
    nodes: Vec<(Kind, String)>,
    signals: HashMap<String, usize>,
    /// From, to, slot.
    edges: BTreeSet<(usize, usize, usize)>,
}

impl Graph<'_> {
    fn signal(&mut self, key: &str, name: &str) -> usize {
        if let Some(&i) = self.signals.get(key) {
            return i;
        }
        self.nodes.push((Kind::Signal, name.to_owned()));
        self.signals.insert(key.to_owned(), self.nodes.len() - 1);
        self.nodes.len() - 1
    }

    fn op(&mut self, label: String, inputs: &[usize]) -> usize {
        self.nodes.push((Kind::Op, label));
        let id = self.nodes.len() - 1;
        for (slot, &i) in inputs.iter().enumerate() {
            self.edges.insert((i, id, slot));
        }
        id
    }

    fn known(&self, text: &str) -> Option<(String, String)> {
        base(text).filter(|(k, _)| self.known.contains(k))
    }

    /// The node carrying the value of `e`; `None` for a constant. `target`
    /// is the signal being assigned, whose own value is a hold.
    fn build(&mut self, e: &Expr, target: Option<usize>) -> Option<usize> {
        match e {
            Expr::Name(n) => {
                let (key, name) = self.known(n)?;
                Some(self.signal(&key, &name))
            }
            Expr::Value(_) => None,
            Expr::Op(op, args) => {
                let built: Vec<Option<usize>> =
                    args.iter().map(|a| self.build(a, target)).collect();
                let mut inputs: Vec<usize> = built.iter().flatten().copied().collect();
                if op.ends_with(":=")
                    && let Some(t) = target
                {
                    // A part assigned: its value goes straight in.
                    for &i in inputs.iter().filter(|&&i| i != t) {
                        self.edges.insert((i, t, 0));
                    }
                    return Some(t);
                }
                if op == "( )" || op.ends_with(":=") {
                    inputs.dedup();
                    return match inputs.len() {
                        0 => None,
                        1 => Some(inputs[0]),
                        _ => Some(self.op("( )".into(), &inputs)),
                    };
                }
                if inputs.is_empty() {
                    return None;
                }
                // `a + 1`: the constant goes in the label.
                let constants: Vec<String> = args
                    .iter()
                    .zip(&built)
                    .filter(|(_, b)| b.is_none())
                    .map(|(a, _)| text(a))
                    .collect();
                let label = if inputs.len() == 1
                    && constants.len() == 1
                    && args.len() == 2
                    && constants[0].chars().count() <= 12
                {
                    format!("{op} {}", constants[0])
                } else {
                    op.clone()
                };
                inputs.dedup();
                Some(self.op(label, &inputs))
            }
            Expr::Call(name, args, _) => {
                if let Some((key, spelling)) = self.known(name) {
                    let signal = self.signal(&key, &spelling);
                    let index: Vec<usize> =
                        args.iter().filter_map(|a| self.build(a, target)).collect();
                    if index.is_empty() {
                        return Some(signal);
                    }
                    let mut inputs = vec![signal];
                    inputs.extend(index);
                    return Some(self.op("( )".into(), &inputs));
                }
                if is_conversion(name) {
                    return args.first().and_then(|a| self.build(a, target));
                }
                let inputs: Vec<usize> =
                    args.iter().filter_map(|a| self.build(a, target)).collect();
                if inputs.is_empty() {
                    return None;
                }
                Some(self.op(name.clone(), &inputs))
            }
            Expr::Mux(..) => {
                // Selects are not drawn, so nested multiplexers are one.
                fn values<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) {
                    match e {
                        Expr::Mux(_, arms) => arms.iter().for_each(|(_, v)| values(v, out)),
                        e => out.push(e),
                    }
                }
                let mut arms = Vec::new();
                values(e, &mut arms);
                let mut inputs: Vec<usize> = Vec::new();
                for value in arms {
                    if let Some(i) = self.build(value, target)
                        && Some(i) != target
                        && !inputs.contains(&i)
                    {
                        inputs.push(i);
                    }
                }
                match inputs.len() {
                    0 => None,
                    1 => Some(inputs[0]),
                    _ => Some(self.op("mux".into(), &inputs)),
                }
            }
        }
    }
}

/// An actual without its type conversions: `counter` for
/// `std_logic_vector(counter)`.
fn unconverted(text: &str) -> &str {
    let mut text = text.trim();
    while let Some((_, name)) = base(text)
        && is_conversion(&name)
        && let (Some(open), true) = (text.find('('), text.ends_with(')'))
    {
        text = text[open + 1..text.len() - 1].trim();
    }
    text
}

/// A constant operand as written, for a label.
fn text(e: &Expr) -> String {
    match e {
        Expr::Name(n) | Expr::Value(n) => n.clone(),
        Expr::Call(_, _, t) => t.clone(),
        _ => "…".into(),
    }
}

/// The data path of one architecture of an entity; `None` when no data
/// flows (nothing but constants and conditions), or it is too large.
/// `link` gives the page of a documented entity.
pub fn datapath(
    design: &Design,
    entity: usize,
    arch: usize,
    link: Option<&dyn Fn(usize) -> String>,
) -> Option<String> {
    let e = &design.entities[entity];
    let a = e.architectures.get(arch)?;
    let known: HashSet<String> = e
        .ports
        .iter()
        .map(|p| identifier_key(&p.name))
        .chain(a.signals.iter().map(|s| identifier_key(&s.name)))
        .collect();
    let mut g = Graph {
        known: &known,
        nodes: Vec::new(),
        signals: HashMap::new(),
        edges: BTreeSet::new(),
    };
    for port in &e.ports {
        let kind = match port.direction() {
            Direction::In => Kind::Input,
            _ => Kind::Output,
        };
        g.nodes.push((kind, port.name.clone()));
        g.signals
            .insert(identifier_key(&port.name), g.nodes.len() - 1);
    }
    // Clocks and resets are not data.
    let control = |arch: &crate::model::Architecture| -> Vec<String> {
        arch.processes
            .iter()
            .flat_map(|p| p.clock.iter().chain(p.reset.as_ref().map(|r| &r.signal)))
            .filter_map(|s| base(s).map(|(k, _)| k))
            .collect()
    };
    let mut skip: HashSet<String> = control(a).into_iter().collect();
    for cone in &a.cones {
        if cone.expr.size() > MAX_NODES {
            continue;
        }
        let Some((key, name)) = g.known(&cone.target) else {
            continue;
        };
        let target = g.signal(&key, &name);
        if let Some(clock) = &cone.clock
            && g.nodes[target].0 == Kind::Signal
        {
            g.nodes[target].0 = Kind::Register(clock.clone(), cone.falling);
        }
        if let Some(root) = g.build(&cone.expr, Some(target))
            && root != target
        {
            g.edges.insert((root, target, 0));
        }
    }
    for instance in &a.instances {
        let (bound, ports) = ports_of(design, entity, arch, instance);
        let inner: HashSet<String> = bound
            .map(|b| {
                design.entities[b]
                    .architectures
                    .iter()
                    .flat_map(control)
                    .collect()
            })
            .unwrap_or_default();
        g.nodes.push((
            Kind::Instance(instance.unit.clone(), bound),
            instance.path(),
        ));
        let id = g.nodes.len() - 1;
        for (slot, (formal, actual)) in instance.port_map.iter().enumerate() {
            let Some((key, name)) = g.known(unconverted(actual)) else {
                continue;
            };
            if inner.contains(&identifier_key(formal)) {
                skip.insert(key);
                continue;
            }
            let direction = ports
                .and_then(|ps| {
                    ps.iter()
                        .find(|p| identifier_key(&p.name) == identifier_key(formal))
                })
                .map(|p| p.direction());
            let signal = g.signal(&key, &name);
            match direction {
                Some(Direction::In) => {
                    g.edges.insert((signal, id, slot));
                }
                Some(Direction::Out) | Some(Direction::InOut) => {
                    g.edges.insert((id, signal, 0));
                }
                _ => {}
            }
        }
    }
    if !g.nodes.iter().any(|(k, _)| *k == Kind::Op) && a.instances.is_empty() {
        return None;
    }
    // Only what data flows through is drawn: not the clocks and resets,
    // nor the logic that only feeds them.
    let skipped: HashSet<usize> = skip
        .iter()
        .filter_map(|k| g.signals.get(k).copied())
        .collect();
    g.edges
        .retain(|(f, t, _)| !skipped.contains(f) && !skipped.contains(t));
    loop {
        let sinks: HashSet<usize> = (0..g.nodes.len())
            .filter(|&i| {
                matches!(g.nodes[i].0, Kind::Op | Kind::Signal) && !g.edges.iter().any(|e| e.0 == i)
            })
            .collect();
        let before = g.edges.len();
        g.edges.retain(|e| !sinks.contains(&e.1));
        if g.edges.len() == before {
            break;
        }
    }
    let mut used = vec![false; g.nodes.len()];
    for &(f, t, _) in &g.edges {
        used[f] = true;
        used[t] = true;
    }
    let keep: Vec<usize> = (0..g.nodes.len()).filter(|&i| used[i]).collect();
    if keep.is_empty() || keep.len() > MAX_DATAPATH {
        return None;
    }
    let mut index = vec![usize::MAX; g.nodes.len()];
    for (new, &old) in keep.iter().enumerate() {
        index[old] = new;
    }
    let nodes: Vec<&(Kind, String)> = keep.iter().map(|&i| &g.nodes[i]).collect();
    let sizes: Vec<(f64, f64)> = nodes
        .iter()
        .map(|(kind, name)| match kind {
            Kind::Op => (width(&cut(name, LABEL)) + 20.0, OP_HEIGHT),
            Kind::Instance(unit, _) => (
                width(&cut(name, LABEL)).max(width(&cut(unit, LABEL))) + 20.0,
                38.0,
            ),
            _ => (width(&cut(name, LABEL)) + 24.0, HEIGHT),
        })
        .collect();
    let edges: Vec<layered::Edge> = g
        .edges
        .iter()
        .map(|&(f, t, slot)| layered::Edge {
            from: index[f],
            to: index[t],
            label: None,
            merge: None,
            slot,
        })
        .collect();
    let layout = layered::layout(&sizes, &edges, None);

    let mut c = Canvas::new();
    for route in &layout.routes {
        for &(x, y) in &route.points {
            c.extend(x, y, x, y);
        }
        c.raw(&format!(
            r#"<path class="vfl-edge" marker-end="url(#vfl-arrow)" d="{}"/>"#,
            rounded(&route.points, 6.0)
        ));
    }
    for (i, (kind, name)) in nodes.iter().enumerate() {
        let (x, y) = layout.states[i];
        let (w, h) = (layout.widths[i], sizes[i].1);
        let (left, top) = (x - w / 2.0, y - h / 2.0);
        let name = cut(name, LABEL);
        match kind {
            Kind::Input | Kind::Output => {
                c.rect(left, top, w, h, "vfl-term");
                c.text(x, y + 4.0, &name, "middle", "");
            }
            Kind::Signal | Kind::Register(..) => {
                c.rect(left, top, w, h, "vfl-box");
                c.text(x, y + 4.0, &name, "middle", "");
                if let Kind::Register(clock, falling) = kind {
                    // The clock input of a flip-flop, at the bottom left.
                    let b = top + h;
                    c.raw(&format!(
                        r#"<g><title>{} {}</title><path class="vfl-box" d="M{left:.1},{:.1}L{:.1},{:.1}L{left:.1},{b:.1}"/></g>"#,
                        escape(clock),
                        if *falling { "↓" } else { "↑" },
                        b - 10.0,
                        left + 7.0,
                        b - 5.0
                    ));
                }
            }
            Kind::Instance(unit, bound) => {
                c.rect(left, top, w, h, "vfl-head");
                c.text(x, top + 15.0, &name, "middle", "vfl-title");
                let href = bound.and_then(|b| link.map(|l| l(b)));
                if let Some(href) = &href {
                    c.raw(&format!(r#"<a href="{}">"#, escape(href)));
                }
                c.text(x, top + 31.0, &cut(unit, LABEL), "middle", "vfl-muted");
                if href.is_some() {
                    c.raw("</a>");
                }
            }
            Kind::Op => {
                c.extend(left, top, left + w, top + h);
                c.raw(&format!(
                    r#"<rect class="vfl-box" x="{left:.1}" y="{top:.1}" width="{w:.1}" height="{h:.1}" rx="{:.1}"/>"#,
                    h / 2.0
                ));
                c.text(x, y + 4.0, &name, "middle", "vfl-title");
            }
        }
    }
    Some(c.finish(&format!("Data path of {}({})", e.name, a.name), 10.0))
}
