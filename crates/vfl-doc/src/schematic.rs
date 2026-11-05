//! Combinational logic diagrams: the logic in front of each signal of an
//! architecture drawn as gates, from its inputs on the left to the signal
//! on the right (or to the register it feeds).
//!
//! Gates are rectangles with their IEC 60617 symbol (`&`, `≥1`, `=1`, an
//! inversion circle for `not`), multiplexers trapezoids with the choice
//! of each input and the select coming in from below, and other operators
//! and functions boxes with their name. Each signal is a tree: an input
//! read twice is written twice, so no wire crosses another.

use std::collections::HashSet;

use vfl_syntax::identifier_key;

use crate::block::base;
use crate::logic::{Cone, Expr, MAX_NODES, is_conversion};
use crate::model::Design;
use crate::svg::{Canvas, cut, width};

const ROW: f64 = 20.0;
const WIRE: f64 = 18.0;
const PAD: f64 = 9.0;
const GATE: f64 = 30.0;
const GAP: f64 = 24.0;
const LABEL: usize = 32;

/// What the tree's nodes are drawn as.
enum Shape {
    /// An input: a signal, or a constant.
    Input(String, bool),
    /// A gate: its symbol, inverted output.
    Gate(&'static str, bool),
    Mux(Vec<String>),
    Box(String),
}

struct Tree {
    shape: Shape,
    inputs: Vec<Tree>,
    /// The select of a multiplexer.
    select: Option<Box<Tree>>,
}

fn leaf(text: &str, signal: bool) -> Tree {
    Tree {
        shape: Shape::Input(text.to_owned(), signal),
        inputs: Vec::new(),
        select: None,
    }
}

fn tree(e: &Expr, known: &HashSet<String>) -> Tree {
    let is_known = |text: &str| base(text).is_some_and(|(k, _)| known.contains(&k));
    let node = |shape, inputs| Tree {
        shape,
        inputs,
        select: None,
    };
    match e {
        Expr::Name(n) => leaf(n, is_known(n)),
        Expr::Value(v) => leaf(v, false),
        Expr::Op(op, args) => {
            let inputs = args.iter().map(|a| tree(a, known)).collect();
            let shape = match op.as_str() {
                "and" => Shape::Gate("&", false),
                "nand" => Shape::Gate("&", true),
                "or" => Shape::Gate("≥1", false),
                "nor" => Shape::Gate("≥1", true),
                "xor" => Shape::Gate("=1", false),
                "xnor" => Shape::Gate("=1", true),
                "not" => Shape::Gate("1", true),
                _ => Shape::Box(op.clone()),
            };
            node(shape, inputs)
        }
        Expr::Call(name, args, text) => {
            let index_known = args.iter().any(|a| mentions(a, known));
            if is_known(name) && !index_known {
                leaf(text, true)
            } else if is_known(name) {
                let mut inputs = vec![leaf(name, true)];
                inputs.extend(args.iter().map(|a| tree(a, known)));
                node(Shape::Box("( )".into()), inputs)
            } else if is_conversion(name) && !args.is_empty() {
                tree(&args[0], known)
            } else {
                node(
                    Shape::Box(name.clone()),
                    args.iter().map(|a| tree(a, known)).collect(),
                )
            }
        }
        Expr::Mux(select, arms) => Tree {
            shape: Shape::Mux(arms.iter().map(|(c, _)| cut(c, LABEL)).collect()),
            inputs: arms.iter().map(|(_, e)| tree(e, known)).collect(),
            select: Some(Box::new(tree(select, known))),
        },
    }
}

/// Whether `e` reads a signal.
fn mentions(e: &Expr, known: &HashSet<String>) -> bool {
    match e {
        Expr::Name(n) => base(n).is_some_and(|(k, _)| known.contains(&k)),
        Expr::Value(_) => false,
        Expr::Op(_, args) => args.iter().any(|a| mentions(a, known)),
        Expr::Call(n, args, _) => {
            base(n).is_some_and(|(k, _)| known.contains(&k))
                || args.iter().any(|a| mentions(a, known))
        }
        Expr::Mux(s, arms) => mentions(s, known) || arms.iter().any(|(_, e)| mentions(e, known)),
    }
}

/// Whether `e` is more than a wire: it has a gate, operator or function.
fn has_logic(e: &Expr, known: &HashSet<String>) -> bool {
    match e {
        Expr::Name(_) | Expr::Value(_) => false,
        Expr::Call(name, args, _) => {
            let signal = base(name).is_some_and(|(k, _)| known.contains(&k));
            if signal {
                args.iter().any(|a| mentions(a, known))
            } else if is_conversion(name) {
                args.first().is_some_and(|a| has_logic(a, known))
            } else {
                true
            }
        }
        _ => true,
    }
}

impl Tree {
    fn width(&self) -> f64 {
        match &self.shape {
            Shape::Input(text, _) => width(&cut(text, LABEL)) + 4.0,
            Shape::Gate(..) => GATE,
            Shape::Mux(labels) => (labels
                .iter()
                .map(|l| width(l) * 10.0 / 12.0)
                .fold(0.0, f64::max)
                + 18.0)
                .max(26.0),
            Shape::Box(label) => (width(&cut(label, LABEL)) + 14.0).max(GATE),
        }
    }

    /// Draws the tree with its output at `(right, y)` where `y` is
    /// returned, its top at `top`; returns the output y and the bottom.
    fn draw(&self, right: f64, top: f64, c: &mut Canvas) -> (f64, f64) {
        let w = self.width();
        if let Shape::Input(text, signal) = &self.shape {
            let y = top + ROW / 2.0;
            let class = if *signal { "" } else { "vfl-actual" };
            c.text(right - 3.0, y + 4.0, &cut(text, LABEL), "end", class);
            return (y, top + ROW);
        }
        let left = right - w;
        let inverted = matches!(self.shape, Shape::Gate(_, true));
        let body_right = if inverted { right - 6.0 } else { right };
        let body_left = if inverted { left - 6.0 } else { left };
        // The inputs, stacked, their outputs a wire away.
        let mut pins = Vec::new();
        let mut y = top;
        for input in &self.inputs {
            let (out, bottom) = input.draw(body_left - WIRE, y, c);
            pins.push(out);
            y = bottom + 4.0;
        }
        let (first, last) = match (pins.first(), pins.last()) {
            (Some(&a), Some(&b)) => (a, b),
            _ => (top + ROW / 2.0, top + ROW / 2.0),
        };
        let body_top = (first - PAD).min((first + last) / 2.0 - 13.0);
        let body_bottom = (last + PAD).max((first + last) / 2.0 + 13.0);
        let out = (body_top + body_bottom) / 2.0;
        for &pin in &pins {
            c.raw(&format!(
                r#"<path class="vfl-wire" d="M{:.1},{pin:.1}H{body_left:.1}"/>"#,
                body_left - WIRE
            ));
        }
        let mut bottom = y.max(body_bottom + 4.0);
        match &self.shape {
            Shape::Gate(..) | Shape::Box(..) => {
                let label = match &self.shape {
                    Shape::Gate(s, _) => (*s).to_owned(),
                    Shape::Box(l) => cut(l, LABEL),
                    _ => String::new(),
                };
                let bw = body_right - body_left;
                c.rect(body_left, body_top, bw, body_bottom - body_top, "vfl-box");
                c.text(body_left + bw / 2.0, out + 4.0, &label, "middle", "");
                if inverted {
                    c.raw(&format!(
                        r#"<circle class="vfl-box" cx="{:.1}" cy="{out:.1}" r="3"/>"#,
                        body_right + 3.0
                    ));
                }
            }
            Shape::Mux(labels) => {
                let inset = 8.0;
                c.extend(body_left, body_top, body_right, body_bottom);
                c.raw(&format!(
                    r#"<path class="vfl-box" d="M{body_left:.1},{body_top:.1}L{body_right:.1},{:.1}L{body_right:.1},{:.1}L{body_left:.1},{body_bottom:.1}Z"/>"#,
                    body_top + inset,
                    body_bottom - inset
                ));
                for (label, &pin) in labels.iter().zip(&pins) {
                    c.text(
                        body_left + 3.0,
                        pin + 3.5,
                        label,
                        "start",
                        "vfl-muted vfl-small",
                    );
                }
                if let Some(select) = &self.select {
                    let mid = (body_left + body_right) / 2.0;
                    let (sy, sb) = select.draw(body_left - WIRE, bottom, c);
                    c.raw(&format!(
                        r#"<path class="vfl-wire" d="M{:.1},{sy:.1}H{mid:.1}V{:.1}"/>"#,
                        body_left - WIRE,
                        body_bottom - inset / 2.0
                    ));
                    bottom = sb + 4.0;
                }
            }
            Shape::Input(..) => {}
        }
        (out, bottom.max(top + ROW))
    }
}

/// The combinational logic of one architecture of an entity: for each
/// signal it drives through more than a wire (state machine states
/// aside), the gates in front of it. `None` when there is none.
pub fn combinational(design: &Design, entity: usize, arch: usize) -> Option<String> {
    let e = &design.entities[entity];
    let a = e.architectures.get(arch)?;
    let known: HashSet<String> = e
        .ports
        .iter()
        .map(|p| identifier_key(&p.name))
        .chain(a.signals.iter().map(|s| identifier_key(&s.name)))
        .collect();
    let states: HashSet<String> = a
        .fsms
        .iter()
        .flat_map(|f| std::iter::once(&f.signal).chain(&f.next))
        .filter_map(|s| base(s).map(|(k, _)| k))
        .collect();
    let cones: Vec<&Cone> = a
        .cones
        .iter()
        .filter(|c| {
            !states.contains(&identifier_key(&c.target))
                && c.expr.size() <= MAX_NODES
                && has_logic(&c.expr, &known)
        })
        .collect();
    if cones.is_empty() {
        return None;
    }
    let mut c = Canvas::new();
    let mut top = 0.0;
    for cone in cones {
        let t = tree(&cone.expr, &known);
        let right = -GAP - if cone.clock.is_some() { 44.0 } else { 0.0 };
        let (out, bottom) = t.draw(right, top, &mut c);
        let name = cut(&cone.target, LABEL);
        match &cone.clock {
            Some(clock) => {
                // A D flip-flop, then the register's name.
                let (x, h) = (-GAP - 36.0, 36.0);
                let y0 = out - 12.0;
                c.raw(&format!(
                    r#"<path class="vfl-wire" d="M{right:.1},{out:.1}H{x:.1}"/>"#
                ));
                c.rect(x, y0, 36.0, h, "vfl-box");
                c.text(x + 4.0, out + 4.0, "D", "start", "vfl-small");
                c.text(x + 32.0, out + 4.0, "Q", "end", "vfl-small");
                c.raw(&format!(
                    r#"<path class="vfl-box" d="M{x:.1},{:.1}L{:.1},{:.1}L{x:.1},{:.1}"/>"#,
                    y0 + h - 10.0,
                    x + 7.0,
                    y0 + h - 5.0,
                    y0 + h
                ));
                let edge = if cone.falling { "↓" } else { "↑" };
                c.text(
                    x + 18.0,
                    y0 + h + 12.0,
                    &format!("{} {edge}", cut(clock, LABEL)),
                    "middle",
                    "vfl-muted vfl-small",
                );
                c.raw(&format!(
                    r#"<path class="vfl-wire" d="M{:.1},{out:.1}H{:.1}"/>"#,
                    x + 36.0,
                    -4.0
                ));
                term(&mut c, out, &name);
                top = bottom.max(y0 + h + 16.0) + GAP;
            }
            None => {
                c.raw(&format!(
                    r#"<path class="vfl-wire" d="M{right:.1},{out:.1}H-4"/>"#
                ));
                term(&mut c, out, &name);
                top = bottom + GAP;
            }
        }
    }
    Some(c.finish(
        &format!("Combinational logic of {}({})", e.name, a.name),
        10.0,
    ))
}

/// The signal a tree drives, as a terminal on the right.
fn term(c: &mut Canvas, y: f64, name: &str) {
    let w = width(name) + 16.0;
    c.rect(-4.0, y - 10.0, w, 20.0, "vfl-term");
    c.text(-4.0 + w / 2.0, y + 4.0, name, "middle", "");
}
