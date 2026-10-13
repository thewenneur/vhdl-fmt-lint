//! Diagrams of how units relate beyond instantiation: which packages they
//! use, and where each reset goes.

use std::collections::HashMap;

use vfl_syntax::identifier_key;

use crate::block::{base, ports_of};
use crate::graph::{GraphEdge, GraphNode, layered};
use crate::model::{Design, Direction, Use};

/// Past this many nodes a graph is left out.
pub const MAX_NODES: usize = 80;

/// A unit of the design: an entity or a package, by index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unit {
    Entity(usize),
    Package(usize),
}

/// The documented package a `use` names.
pub fn resolve_use(design: &Design, u: &Use) -> Option<usize> {
    design
        .packages
        .iter()
        .position(|p| identifier_key(&p.library) == u.library && identifier_key(&p.name) == u.unit)
}

/// The dependency graph: every unit that uses a documented package, and
/// those packages. `link` gives the page of a unit. `None` when no unit
/// uses a documented package, or when the graph is too large.
pub fn dependencies(design: &Design, link: Option<&dyn Fn(Unit) -> String>) -> Option<String> {
    let mut units: Vec<(Unit, Vec<usize>)> = Vec::new();
    for (i, e) in design.entities.iter().enumerate() {
        let used: Vec<usize> = e.uses.iter().filter_map(|u| resolve_use(design, u)).collect();
        units.push((Unit::Entity(i), used));
    }
    for (i, p) in design.packages.iter().enumerate() {
        let used: Vec<usize> = p
            .uses
            .iter()
            .filter_map(|u| resolve_use(design, u))
            .filter(|&q| q != i)
            .collect();
        units.push((Unit::Package(i), used));
    }
    let used_packages: Vec<usize> = {
        let mut list: Vec<usize> = units.iter().flat_map(|(_, u)| u.iter().copied()).collect();
        list.sort_unstable();
        list.dedup();
        list
    };
    if used_packages.is_empty() {
        return None;
    }
    let mut index: HashMap<Unit, usize> = HashMap::new();
    let mut nodes = Vec::new();
    let mut add = |unit: Unit, nodes: &mut Vec<GraphNode>| -> usize {
        *index.entry(unit).or_insert_with(|| {
            let (title, detail, class) = match unit {
                Unit::Entity(i) => (design.entities[i].name.clone(), "entity", "vfl-box"),
                Unit::Package(i) => (design.packages[i].name.clone(), "package", "vfl-head"),
            };
            nodes.push(GraphNode {
                title,
                detail: Some(detail.into()),
                class,
                link: link.map(|l| l(unit)),
            });
            nodes.len() - 1
        })
    };
    let mut edges = Vec::new();
    for (unit, used) in &units {
        if used.is_empty() {
            continue;
        }
        let from = add(*unit, &mut nodes);
        for &p in used {
            let to = add(Unit::Package(p), &mut nodes);
            edges.push(GraphEdge { from, to });
        }
    }
    if nodes.len() > MAX_NODES {
        return None;
    }
    Some(layered(&nodes, &edges, "Package dependencies"))
}

/// A node of a reset tree.
#[derive(Debug, Clone)]
struct ResetNode {
    title: String,
    detail: String,
    entity: Option<usize>,
    children: Vec<ResetNode>,
    /// Resets at least one process, here or below.
    used: bool,
}

const MAX_DEPTH: usize = 24;

/// Where the signal `key` of an architecture goes as a reset: the
/// processes it resets, the instances it enters (followed inside), and the
/// signals assigned from it (followed too).
fn follow(
    design: &Design,
    entity: usize,
    arch: usize,
    key: &str,
    depth: usize,
    budget: &mut usize,
) -> Vec<ResetNode> {
    let mut out = Vec::new();
    let a = &design.entities[entity].architectures[arch];
    if depth > MAX_DEPTH || *budget == 0 {
        return out;
    }
    for p in &a.processes {
        let Some(reset) = &p.reset else { continue };
        if identifier_key(&reset.signal) != key {
            continue;
        }
        *budget = budget.saturating_sub(1);
        let level = match reset.active_high {
            Some(true) => ", active high",
            Some(false) => ", active low",
            None => "",
        };
        out.push(ResetNode {
            title: p
                .label
                .clone()
                .unwrap_or_else(|| format!("process @{}", p.line)),
            detail: format!(
                "{}{level}",
                if reset.asynchronous { "async" } else { "sync" }
            ),
            entity: None,
            children: Vec::new(),
            used: true,
        });
    }
    for s in &a.assignments {
        if identifier_key(&s.target) == key
            || !s.reads.iter().any(|r| identifier_key(r) == key)
        {
            continue;
        }
        let children = follow(design, entity, arch, &identifier_key(&s.target), depth + 1, budget);
        if children.iter().any(|c| c.used) {
            out.push(ResetNode {
                title: s.target.clone(),
                detail: "assignment".into(),
                entity: None,
                used: true,
                children,
            });
        }
    }
    for instance in &a.instances {
        let (target, ports) = ports_of(design, entity, arch, instance);
        let (Some(target), Some(ports)) = (target, ports) else {
            continue;
        };
        for (k, (formal, actual)) in instance.port_map.iter().enumerate() {
            if base(actual).map(|(k, _)| k).as_deref() != Some(key) {
                continue;
            }
            let port = if formal.is_empty() {
                ports.get(k)
            } else {
                let fkey = base(formal).map(|(k, _)| k);
                ports.iter().find(|p| Some(identifier_key(&p.name)) == fkey)
            };
            let Some(port) = port.filter(|p| p.direction() == Direction::In) else {
                continue;
            };
            let t = &design.entities[target];
            let t_arch = match &instance.architecture {
                Some(name) => t
                    .architectures
                    .iter()
                    .position(|a| identifier_key(&a.name) == identifier_key(name)),
                None => t.architectures.len().checked_sub(1),
            };
            let Some(t_arch) = t_arch else { continue };
            let children = follow(
                design,
                target,
                t_arch,
                &identifier_key(&port.name),
                depth + 1,
                budget,
            );
            if children.iter().any(|c| c.used) {
                out.push(ResetNode {
                    title: instance.path(),
                    detail: format!("{} ({})", t.name, port.name),
                    entity: Some(target),
                    used: true,
                    children,
                });
            }
        }
    }
    out
}

/// The reset tree of an entity: each input port that resets a process,
/// in this architecture or below, down to those processes. `link` gives
/// the page of an entity.
pub fn resets(
    design: &Design,
    entity: usize,
    arch: usize,
    link: Option<&dyn Fn(usize) -> String>,
) -> Option<String> {
    let e = &design.entities[entity];
    e.architectures.get(arch)?;
    let mut budget = MAX_NODES;
    let mut roots = Vec::new();
    for port in e.ports.iter().filter(|p| p.direction() == Direction::In) {
        let children = follow(design, entity, arch, &identifier_key(&port.name), 0, &mut budget);
        if children.iter().any(|c| c.used) {
            roots.push(ResetNode {
                title: port.name.clone(),
                detail: String::new(),
                entity: None,
                used: true,
                children,
            });
        }
    }
    if roots.is_empty() {
        return None;
    }
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    fn add(
        node: &ResetNode,
        root: bool,
        nodes: &mut Vec<GraphNode>,
        edges: &mut Vec<GraphEdge>,
        link: Option<&dyn Fn(usize) -> String>,
    ) -> usize {
        let index = nodes.len();
        nodes.push(GraphNode {
            title: node.title.clone(),
            detail: (!node.detail.is_empty()).then(|| node.detail.clone()),
            class: if root {
                "vfl-term"
            } else if node.children.is_empty() {
                "vfl-box"
            } else {
                "vfl-head"
            },
            link: node.entity.and_then(|e| link.map(|l| l(e))),
        });
        for child in &node.children {
            let c = add(child, false, nodes, edges, link);
            edges.push(GraphEdge {
                from: index,
                to: c,
            });
        }
        index
    }
    for root in &roots {
        add(root, true, &mut nodes, &mut edges, link);
    }
    Some(layered(
        &nodes,
        &edges,
        &format!("Reset tree of {}({})", e.name, e.architectures[arch].name),
    ))
}
