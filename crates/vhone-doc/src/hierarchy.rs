//! The design hierarchy: which entity each instance binds to, and the
//! instantiation tree from the top-level entities down.

use std::collections::HashSet;

use vhone_syntax::identifier_key;

use crate::model::{Design, Instance, InstanceKind};

/// A node of the instantiation tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// The instance path relative to the parent (`gen.u_fifo`); `None` for
    /// a top-level entity.
    pub label: Option<String>,
    /// The entity's (or unbound component's) name as written.
    pub unit: String,
    /// The bound entity, as an index into [`Design::entities`]; `None` when
    /// it is not part of the documented design (a vendor primitive, say).
    pub entity: Option<usize>,
    pub architecture: Option<String>,
    pub children: Vec<Node>,
    /// The entity instantiates itself further up: its subtree is not repeated.
    pub recursive: bool,
    /// The entity's subtree is drawn earlier in the same diagram, so it is
    /// left out here (see [`collapse_repeats`]).
    pub repeated: bool,
}

/// The entity an instance in an entity of `library` binds to.
pub fn resolve(design: &Design, instance: &Instance, library: &str) -> Option<usize> {
    let library = match instance.library.as_deref().map(identifier_key) {
        Some(l) if l != "work" => l,
        _ => library.to_owned(),
    };
    let mut entity = identifier_key(&instance.unit);
    if instance.kind == InstanceKind::Configuration {
        entity = design
            .configurations
            .iter()
            .filter(|(_, name, _)| *name == entity)
            .min_by_key(|(l, _, _)| *l != library)
            .map(|(_, _, e)| e.clone())?;
    }
    design
        .by_name
        .get(&entity)?
        .iter()
        .copied()
        .min_by_key(|&i| design.entities[i].library != library)
}

/// The architecture an instance of `entity` uses: the one it names, else
/// the last one (VHDL binds the most recently analyzed).
fn architecture_index(design: &Design, entity: usize, name: Option<&str>) -> Option<usize> {
    let architectures = &design.entities[entity].architectures;
    match name {
        Some(name) => architectures
            .iter()
            .position(|a| identifier_key(&a.name) == identifier_key(name)),
        None => architectures.len().checked_sub(1),
    }
}

/// The instantiation trees of the design, one per entity no other entity
/// instantiates.
pub fn build(design: &Design) -> Vec<Node> {
    let mut instantiated = vec![false; design.entities.len()];
    for entity in &design.entities {
        for arch in &entity.architectures {
            for instance in &arch.instances {
                if let Some(target) = resolve(design, instance, &entity.library)
                    && (identifier_key(&design.entities[target].name) != entity.key()
                        || design.entities[target].library != entity.library)
                {
                    instantiated[target] = true;
                }
            }
        }
    }
    (0..design.entities.len())
        .filter(|&i| !instantiated[i])
        .map(|i| subtree(design, i, None, None, &mut Expand::once()))
        .collect()
}

/// How far [`subtree`] expands.
#[derive(Debug, Clone)]
pub struct Expand {
    /// Levels of instances below the entity; `1` for its own instances only.
    pub depth: usize,
    /// Entities whose subtree is already drawn: they are marked `repeated`
    /// and not expanded again. `None` expands every occurrence.
    pub seen: Option<HashSet<usize>>,
}

impl Expand {
    /// Everything, every subtree once.
    pub fn once() -> Self {
        Expand {
            depth: usize::MAX,
            seen: Some(HashSet::new()),
        }
    }

    /// The entity's own instances.
    pub fn children() -> Self {
        Expand {
            depth: 1,
            seen: None,
        }
    }
}

/// The tree under one entity.
pub fn subtree(
    design: &Design,
    entity: usize,
    label: Option<String>,
    architecture: Option<&str>,
    expand: &mut Expand,
) -> Node {
    walk(design, entity, label, architecture, expand, &mut Vec::new())
}

fn walk(
    design: &Design,
    entity: usize,
    label: Option<String>,
    architecture: Option<&str>,
    expand: &mut Expand,
    stack: &mut Vec<usize>,
) -> Node {
    let arch = architecture_index(design, entity, architecture);
    let mut node = Node {
        label,
        unit: design.entities[entity].name.clone(),
        entity: Some(entity),
        architecture: arch.map(|a| design.entities[entity].architectures[a].name.clone()),
        children: Vec::new(),
        recursive: stack.contains(&entity),
        repeated: false,
    };
    let Some(arch) = arch else {
        return node;
    };
    let instances = &design.entities[entity].architectures[arch].instances;
    if node.recursive || stack.len() >= expand.depth || instances.is_empty() {
        return node;
    }
    if let Some(seen) = &mut expand.seen
        && !seen.insert(entity)
    {
        node.repeated = true;
        return node;
    }
    stack.push(entity);
    let library = &design.entities[entity].library;
    for instance in instances {
        let child = match resolve(design, instance, library) {
            Some(target) => walk(
                design,
                target,
                Some(instance.path()),
                instance.architecture.as_deref(),
                expand,
                stack,
            ),
            None => Node {
                label: Some(instance.path()),
                unit: instance.unit.clone(),
                entity: None,
                architecture: instance.architecture.clone(),
                children: Vec::new(),
                recursive: false,
                repeated: false,
            },
        };
        node.children.push(child);
    }
    stack.pop();
    node
}

/// Leaves out the subtree of every entity whose subtree is already shown
/// earlier (in depth-first order), so that a design instantiated by several
/// testbenches, or a block instantiated many times, is drawn once.
pub fn collapse_repeats(roots: &mut [Node]) {
    fn walk(node: &mut Node, seen: &mut Vec<usize>) {
        if let Some(entity) = node.entity
            && !node.children.is_empty()
        {
            if seen.contains(&entity) {
                node.children.clear();
                node.repeated = true;
                return;
            }
            seen.push(entity);
        }
        for child in &mut node.children {
            walk(child, seen);
        }
    }
    let mut seen = Vec::new();
    for root in roots {
        walk(root, &mut seen);
    }
}
