//! Clock domains of an architecture, and the signals that cross between
//! them.
//!
//! A clocked process belongs to the domain of its clock; an instance to
//! the domains of the signals connected to its clock inputs (the ports its
//! entity uses as clocks, inside instances too). Combinational processes
//! and assignments pass on the domains of what they read. A signal crosses
//! when an element of one domain reads it while it comes from another.

use std::collections::{BTreeSet, HashMap, HashSet};

use vfl_syntax::identifier_key;

use crate::block::{base, ports_of};
use crate::model::{Design, Direction};
use crate::svg::{Canvas, cut, width};

/// A crossing: a signal from one domain read in another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crossing {
    pub signal: String,
    pub from: String,
    pub to: String,
    /// The process or instance that reads it.
    pub reader: String,
}

/// The clock domains of an architecture.
#[derive(Debug, Clone, Default)]
pub struct Domains {
    /// Each clock (as spelt) with its processes and instances.
    pub domains: Vec<(String, Vec<String>)>,
    /// Instances on several clocks.
    pub bridges: Vec<(String, Vec<String>)>,
    pub crossings: Vec<Crossing>,
}

/// The ports an entity uses as clocks, by key: clocks of its processes,
/// and ports connected to clock ports of its instances.
fn clock_ports(
    design: &Design,
    entity: usize,
    memo: &mut HashMap<usize, HashSet<String>>,
    visiting: &mut HashSet<usize>,
) -> HashSet<String> {
    if let Some(known) = memo.get(&entity) {
        return known.clone();
    }
    if !visiting.insert(entity) {
        return HashSet::new();
    }
    let e = &design.entities[entity];
    let ports: HashSet<String> = e.ports.iter().map(|p| identifier_key(&p.name)).collect();
    let mut clocks = HashSet::new();
    for (a, arch) in e.architectures.iter().enumerate() {
        for p in &arch.processes {
            if let Some(clock) = &p.clock {
                let key = identifier_key(clock);
                if ports.contains(&key) {
                    clocks.insert(key);
                }
            }
        }
        for instance in &arch.instances {
            for (_, actual) in instance_clocks(design, entity, a, instance, memo, visiting) {
                if ports.contains(&actual) {
                    clocks.insert(actual);
                }
            }
        }
    }
    visiting.remove(&entity);
    memo.insert(entity, clocks.clone());
    clocks
}

/// The clock ports of an instance's entity with the signal connected to
/// each, by key: (formal, actual).
fn instance_clocks(
    design: &Design,
    entity: usize,
    arch: usize,
    instance: &crate::model::Instance,
    memo: &mut HashMap<usize, HashSet<String>>,
    visiting: &mut HashSet<usize>,
) -> Vec<(String, String)> {
    let (target, ports) = ports_of(design, entity, arch, instance);
    let (Some(target), Some(ports)) = (target, ports) else {
        return Vec::new();
    };
    let clocks = clock_ports(design, target, memo, visiting);
    let mut out = Vec::new();
    for (k, (formal, actual)) in instance.port_map.iter().enumerate() {
        let formal_key = if formal.is_empty() {
            ports.get(k).map(|p| identifier_key(&p.name))
        } else {
            base(formal).map(|(k, _)| k)
        };
        let (Some(formal_key), Some((actual_key, _))) = (formal_key, base(actual)) else {
            continue;
        };
        if clocks.contains(&formal_key) {
            out.push((formal_key, actual_key));
        }
    }
    out
}

/// An element that reads and drives signals.
struct Element {
    title: String,
    /// Clock keys; empty for combinational logic.
    clocks: Vec<String>,
    reads: Vec<String>,
    drives: Vec<String>,
}

/// The clock domains of an architecture; `None` with fewer than two.
pub fn domains(design: &Design, entity: usize, arch: usize) -> Option<Domains> {
    let e = &design.entities[entity];
    let a = e.architectures.get(arch)?;
    let mut memo = HashMap::new();
    let mut visiting = HashSet::new();
    let mut spelling: HashMap<String, String> = HashMap::new();
    let mut elements = Vec::new();
    for p in &a.processes {
        let clocks: Vec<String> = p.clock.iter().map(|c| identifier_key(c)).collect();
        if let Some(c) = &p.clock {
            spelling
                .entry(identifier_key(c))
                .or_insert_with(|| c.clone());
        }
        let mut skip: Vec<String> = clocks.clone();
        if let Some(r) = &p.reset {
            skip.push(identifier_key(&r.signal));
        }
        elements.push(Element {
            title: p
                .label
                .clone()
                .unwrap_or_else(|| format!("process @{}", p.line)),
            clocks,
            reads: a
                .resolve_aliases(&p.reads)
                .iter()
                .map(|r| identifier_key(r))
                .filter(|r| !skip.contains(r))
                .collect(),
            drives: p.drives.iter().map(|d| identifier_key(d)).collect(),
        });
    }
    for s in &a.assignments {
        elements.push(Element {
            title: s
                .label
                .clone()
                .unwrap_or_else(|| format!("{} <=", s.target)),
            clocks: Vec::new(),
            reads: a
                .resolve_aliases(&s.reads)
                .iter()
                .map(|r| identifier_key(r))
                .collect(),
            drives: vec![identifier_key(&s.target)],
        });
    }
    for instance in &a.instances {
        let clocks = instance_clocks(design, entity, arch, instance, &mut memo, &mut visiting);
        let (_, ports) = ports_of(design, entity, arch, instance);
        let ports = ports.unwrap_or_default();
        let clock_keys: Vec<String> = {
            let mut list: Vec<String> = clocks.iter().map(|(_, a)| a.clone()).collect();
            list.sort();
            list.dedup();
            list
        };
        let mut reads = Vec::new();
        let mut drives = Vec::new();
        for (k, (formal, actual)) in instance.port_map.iter().enumerate() {
            let Some((key, name)) = base(actual) else {
                continue;
            };
            if clock_keys.contains(&key) {
                spelling.entry(key).or_insert(name);
                continue;
            }
            let port = if formal.is_empty() {
                ports.get(k)
            } else {
                let fkey = base(formal).map(|(k, _)| k);
                ports.iter().find(|p| Some(identifier_key(&p.name)) == fkey)
            };
            match port.map(|p| p.direction()) {
                Some(Direction::Out) => drives.push(key),
                Some(Direction::InOut) => {
                    drives.push(key.clone());
                    reads.push(key);
                }
                _ => reads.push(key),
            }
        }
        elements.push(Element {
            title: instance.path(),
            clocks: clock_keys,
            reads,
            drives,
        });
    }
    let all_clocks: BTreeSet<String> = elements
        .iter()
        .flat_map(|el| el.clocks.iter().cloned())
        .collect();
    if all_clocks.len() < 2 {
        return None;
    }

    // The domains each signal comes from.
    let mut net: HashMap<String, BTreeSet<String>> = HashMap::new();
    for el in &elements {
        if el.clocks.len() == 1 {
            for d in &el.drives {
                net.entry(d.clone())
                    .or_default()
                    .insert(el.clocks[0].clone());
            }
        }
    }
    for _ in 0..8 {
        let mut changed = false;
        for el in elements.iter().filter(|el| el.clocks.is_empty()) {
            let from: BTreeSet<String> = el
                .reads
                .iter()
                .filter_map(|r| net.get(r))
                .flatten()
                .cloned()
                .collect();
            for d in &el.drives {
                let entry = net.entry(d.clone()).or_default();
                let before = entry.len();
                entry.extend(from.iter().cloned());
                changed |= entry.len() != before;
            }
        }
        if !changed {
            break;
        }
    }
    let signal_name = |key: &str| -> String {
        a.signals
            .iter()
            .map(|s| &s.name)
            .chain(e.ports.iter().map(|p| &p.name))
            .find(|n| identifier_key(n) == key)
            .cloned()
            .unwrap_or_else(|| key.to_owned())
    };
    let clock_name =
        |key: &str| -> String { spelling.get(key).cloned().unwrap_or_else(|| key.to_owned()) };
    let mut crossings = Vec::new();
    for el in elements.iter().filter(|el| el.clocks.len() == 1) {
        let to = &el.clocks[0];
        for r in &el.reads {
            for from in net.get(r).into_iter().flatten() {
                if from != to {
                    crossings.push(Crossing {
                        signal: signal_name(r),
                        from: clock_name(from),
                        to: clock_name(to),
                        reader: el.title.clone(),
                    });
                }
            }
        }
    }
    let domains = all_clocks
        .iter()
        .map(|clock| {
            (
                clock_name(clock),
                elements
                    .iter()
                    .filter(|el| el.clocks.len() == 1 && &el.clocks[0] == clock)
                    .map(|el| el.title.clone())
                    .collect(),
            )
        })
        .collect();
    let bridges = elements
        .iter()
        .filter(|el| el.clocks.len() > 1)
        .map(|el| {
            (
                el.title.clone(),
                el.clocks.iter().map(|c| clock_name(c)).collect(),
            )
        })
        .collect();
    Some(Domains {
        domains,
        bridges,
        crossings,
    })
}

const MAX_LINES: usize = 12;

/// The domains as boxes in a row, the instances on several clocks between
/// them, and an arc under the boxes for each pair of domains a signal
/// crosses between, labelled with the signals.
pub fn draw(domains: &Domains, title: &str) -> String {
    const ROW: f64 = 18.0;
    const HEAD: f64 = 26.0;
    const GAP: f64 = 48.0;
    let mut c = Canvas::new();
    let mut boxes: Vec<(String, f64, f64)> = Vec::new(); // clock, center x, bottom
    let mut x = 0.0;
    let draw_box =
        |c: &mut Canvas, x: &mut f64, head: &str, lines: &[String], class: &str| -> (f64, f64) {
            let mut shown: Vec<String> = lines.iter().take(MAX_LINES).map(|l| cut(l, 36)).collect();
            if shown.is_empty() {
                shown.push("—".into());
            }
            if lines.len() > MAX_LINES {
                shown.push(format!("… {} more", lines.len() - MAX_LINES));
            }
            let w = shown
                .iter()
                .map(|l| width(l))
                .fold(width(&cut(head, 36)), f64::max)
                + 20.0;
            let h = HEAD + shown.len().max(1) as f64 * ROW + 8.0;
            c.rect(*x, 0.0, w, h, class);
            c.rect(*x, 0.0, w, HEAD, "vfl-head");
            c.text(*x + w / 2.0, 17.0, &cut(head, 36), "middle", "vfl-title");
            for (i, line) in shown.iter().enumerate() {
                c.text(*x + 10.0, HEAD + 14.0 + i as f64 * ROW, line, "start", "");
            }
            let center = *x + w / 2.0;
            *x += w + GAP;
            (center, h)
        };
    let (bridged, alone): (Vec<_>, Vec<_>) = domains.domains.iter().partition(|(clock, _)| {
        domains
            .bridges
            .iter()
            .any(|(_, clocks)| clocks.contains(clock))
    });
    // Domains joined by a bridge sit on each side of it.
    let mut order: Vec<&(String, Vec<String>)> = Vec::new();
    for d in bridged.iter().chain(alone.iter()) {
        order.push(d);
    }
    let mut placed_bridges = false;
    for (i, (clock, items)) in order.iter().map(|d| (&d.0, &d.1)).enumerate() {
        let (center, h) = draw_box(&mut c, &mut x, &format!("{clock} domain"), items, "vfl-box");
        boxes.push((clock.clone(), center, h));
        if i == 0 && !domains.bridges.is_empty() && !placed_bridges {
            let lines: Vec<String> = domains
                .bridges
                .iter()
                .map(|(name, clocks)| format!("{name} ({})", clocks.join(", ")))
                .collect();
            draw_box(&mut c, &mut x, "several clocks", &lines, "vfl-ext");
            placed_bridges = true;
        }
    }
    let bottom = boxes.iter().map(|b| b.2).fold(0.0, f64::max);
    // One arc per (from, to) pair, with the signals that cross.
    let mut pairs: Vec<((String, String), Vec<String>)> = Vec::new();
    for crossing in &domains.crossings {
        let key = (crossing.from.clone(), crossing.to.clone());
        match pairs.iter_mut().find(|(k, _)| *k == key) {
            Some((_, signals)) => {
                if !signals.contains(&crossing.signal) {
                    signals.push(crossing.signal.clone());
                }
            }
            None => pairs.push((key, vec![crossing.signal.clone()])),
        }
    }
    for (i, ((from, to), signals)) in pairs.iter().enumerate() {
        let find = |clock: &str| boxes.iter().find(|b| b.0 == clock).map(|b| b.1);
        let (Some(x0), Some(x1)) = (find(from), find(to)) else {
            continue;
        };
        // Leave and enter the boxes a little apart, so that arcs in both
        // directions do not overlap.
        let (x0, x1) = if x0 < x1 {
            (x0 + 10.0, x1 - 10.0)
        } else {
            (x0 - 10.0, x1 + 10.0)
        };
        let depth = bottom + 40.0 + i as f64 * 34.0;
        let path = format!(
            "M{x0:.1},{bottom:.1}C{x0:.1},{depth:.1} {x1:.1},{depth:.1} {x1:.1},{:.1}",
            bottom + 2.0
        );
        c.extend(x0.min(x1), bottom, x0.max(x1), depth);
        c.raw(&format!(
            r#"<path class="vfl-cross" d="{path}" marker-end="url(#vfl-arrow)"/>"#
        ));
        let label = cut(&signals.join(", "), 60);
        let mid = (x0 + x1) / 2.0;
        let ly = bottom + (depth - bottom) * 0.75 + 4.0;
        let w = width(&label) + 8.0;
        c.rect(mid - w / 2.0, ly - 13.0, w, 17.0, "vfl-label-bg");
        c.text(mid, ly, &label, "middle", "vfl-cross-text");
    }
    c.finish(title, 10.0)
}

/// The clock domains of an architecture, drawn; `None` with fewer than
/// two domains.
pub fn diagram(design: &Design, entity: usize, arch: usize) -> Option<(String, Domains)> {
    let d = domains(design, entity, arch)?;
    let e = &design.entities[entity];
    let svg = draw(
        &d,
        &format!(
            "Clock domains of {}({})",
            e.name, e.architectures[arch].name
        ),
    );
    Some((svg, d))
}
