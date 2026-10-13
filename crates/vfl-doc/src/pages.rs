//! The pages of the documentation: an index, one page per entity and one
//! per package.

use vfl_config::DocConfig;
use vfl_syntax::identifier_key;

use crate::comments::brief;
use crate::hierarchy::{self, Node};
use crate::model::{Architecture, Design, Entity, InstanceKind, Interface, Object, Package};
use crate::page::{Block, Cell, Inline, NavItem, Page, code, text};
use crate::svg::{self, SymbolOptions};
use crate::text as diagram_text;

/// A file name from a unit name: lower case, and only safe characters.
fn file_safe(text: &str) -> String {
    let safe: String = identifier_key(text)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // A file name has at most 255 bytes and is made of up to four of
    // these: past the limit, keep the start and a hash of the whole.
    if safe.len() > MAX_NAME_PART {
        format!("{}-{:016x}", &safe[..MAX_NAME_PART - 17], fnv(&safe))
    } else {
        safe
    }
}

/// The longest part of a file name taken from one identifier.
const MAX_NAME_PART: usize = 48;

fn extension(config: &DocConfig) -> &'static str {
    match config.format {
        vfl_config::DocFormat::Html => "html",
        vfl_config::DocFormat::Markdown => "md",
        vfl_config::DocFormat::Asciidoc => "adoc",
    }
}

pub(crate) fn page_id(library: &str, name: &str) -> String {
    format!("{}.{}", file_safe(library), file_safe(name))
}

fn entity_page(design: &Design, index: usize) -> String {
    design.entities[index].page.clone()
}

fn link(label: &str, page: String, anchor: Option<String>) -> Inline {
    Inline::Link {
        text: label.to_owned(),
        code: true,
        page,
        anchor,
    }
}

fn heading(level: u8, content: Vec<Inline>, id: Option<String>) -> Block {
    Block::Heading { level, content, id }
}

fn doc(text: &str, blocks: &mut Vec<Block>, config: &DocConfig) {
    if text.is_empty() {
        return;
    }
    // ```wavedrom blocks become timing diagrams.
    let mut rest = text;
    while config.diagrams.waveforms
        && let Some(start) = rest.find("```wavedrom")
    {
        let body_start = start + "```wavedrom".len();
        let Some(len) = rest[body_start..].find("```") else {
            break;
        };
        let source = &rest[body_start..body_start + len];
        let before = rest[..start].trim();
        if !before.is_empty() {
            blocks.push(Block::Doc(before.to_owned()));
        }
        match crate::wave::draw(source) {
            Ok(svg) => blocks.push(Block::Diagram {
                file: format!("wave-{:016x}", fnv(source)),
                alt: "Timing diagram".into(),
                svg,
                mermaid: None,
            }),
            // Shown as written, to be fixed.
            Err(_) => blocks.push(Block::Doc(rest[start..body_start + len + 3].to_owned())),
        }
        rest = &rest[body_start + len + 3..];
    }
    let rest = rest.trim();
    if !rest.is_empty() {
        blocks.push(Block::Doc(rest.to_owned()));
    }
}

/// A stable hash, for file names derived from content.
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn opt_code(value: &Option<String>) -> Cell {
    value.iter().map(|v| code(v.clone())).collect()
}

fn generics_table(generics: &[Interface]) -> Block {
    Block::Table {
        head: vec!["Name", "Type", "Default", "Description"],
        rows: generics
            .iter()
            .map(|g| {
                vec![
                    vec![code(g.name.clone())],
                    vec![code(g.ty.clone())],
                    opt_code(&g.default),
                    vec![text(g.description.clone())],
                ]
            })
            .collect(),
    }
}

fn ports_table(ports: &[Interface]) -> Block {
    Block::Table {
        head: vec!["Name", "Direction", "Type", "Description"],
        rows: ports
            .iter()
            .map(|p| {
                let mode = if p.mode.is_empty() {
                    "in"
                } else {
                    p.mode.as_str()
                };
                let mut ty = vec![code(p.ty.clone())];
                if let Some(default) = &p.default {
                    ty.push(text(" := "));
                    ty.push(code(default.clone()));
                }
                vec![
                    vec![code(p.name.clone())],
                    vec![text(mode)],
                    ty,
                    vec![text(p.description.clone())],
                ]
            })
            .collect(),
    }
}

fn objects_table(objects: &[Object], value: &'static str) -> Block {
    Block::Table {
        head: vec!["Name", "Type", value, "Description"],
        rows: objects
            .iter()
            .map(|o| {
                vec![
                    vec![code(o.name.clone())],
                    vec![code(o.ty.clone())],
                    opt_code(&o.value),
                    vec![text(o.description.clone())],
                ]
            })
            .collect(),
    }
}

fn types_table(types: &[crate::model::TypeInfo]) -> Block {
    Block::Table {
        head: vec!["Name", "Kind", "Definition", "Description"],
        rows: types
            .iter()
            .map(|t| {
                vec![
                    vec![code(t.name.clone())],
                    vec![text(t.kind)],
                    vec![code(t.definition.clone())],
                    vec![text(t.description.clone())],
                ]
            })
            .collect(),
    }
}

fn subprograms_table(subprograms: &[crate::model::Subprogram]) -> Block {
    Block::Table {
        head: vec!["Name", "Signature", "Description"],
        rows: subprograms
            .iter()
            .map(|s| {
                vec![
                    vec![code(s.name.clone())],
                    vec![code(s.signature.clone())],
                    vec![text(s.description.clone())],
                ]
            })
            .collect(),
    }
}

fn location(file: &str, line: u32) -> Inline {
    code(format!("{file}:{line}"))
}

pub fn pages(design: &Design, roots: &[Node], config: &DocConfig) -> Vec<Page> {
    let mut pages = vec![index(design, roots, config)];
    // Who instantiates each entity: (parent entity, architecture, path).
    let mut users: Vec<Vec<(usize, String, String)>> = vec![Vec::new(); design.entities.len()];
    for (i, entity) in design.entities.iter().enumerate() {
        for arch in &entity.architectures {
            for instance in &arch.instances {
                if let Some(target) = hierarchy::resolve(design, instance, &entity.library) {
                    users[target].push((i, arch.name.clone(), instance.path()));
                }
            }
        }
    }
    let packages = crate::fields::Scope::packages(design);
    for (i, users) in users.iter().enumerate() {
        pages.push(entity(design, i, users, &packages, config));
    }
    for package in &design.packages {
        pages.push(package_page(design, package, &packages, config));
    }
    pages
}

/// The most units listed in the side bar of every HTML page. Past it, the
/// pages would mostly be side bar, and the index lists the units instead.
const MAX_NAV: usize = 500;

pub fn nav(design: &Design) -> Vec<NavItem> {
    let mut items = Vec::new();
    if design.entities.len() + design.packages.len() > MAX_NAV {
        return items;
    }
    for e in &design.entities {
        items.push(NavItem {
            section: "Entities",
            page: e.page.clone(),
            label: e.name.clone(),
        });
    }
    for p in &design.packages {
        items.push(NavItem {
            section: "Packages",
            page: p.page.clone(),
            label: p.name.clone(),
        });
    }
    items
}

/// A diagram of each instance of an architecture with its actuals.
fn port_maps(
    design: &Design,
    index: usize,
    entity: &Entity,
    arch: &Architecture,
    page: &str,
    arch_key: &str,
    blocks: &mut Vec<Block>,
) {
    let Some(a) = entity
        .architectures
        .iter()
        .position(|a| std::ptr::eq(a, arch))
    else {
        return;
    };
    let mut first = true;
    for (i, instance) in arch.instances.iter().enumerate() {
        let Some(svg) = crate::block::port_map(design, index, a, i) else {
            continue;
        };
        if first {
            blocks.push(heading(3, vec![text("Port maps")], None));
            first = false;
        }
        let path = instance.path();
        blocks.push(heading(4, vec![code(path.clone())], None));
        blocks.push(Block::Diagram {
            file: format!("{page}.{arch_key}.map-{}", file_safe(&path)),
            alt: format!("Port map of {path}"),
            svg,
            mermaid: None,
        });
    }
}

/// The most top-level entities the index draws.
const MAX_TOPS: usize = 4;

/// The top-level entities: the roots of the hierarchy, or for a testbench
/// the documented entities it instantiates, each with its architecture.
fn tops(design: &Design, roots: &[Node]) -> Vec<(usize, Option<String>)> {
    let mut tops: Vec<(usize, Option<String>)> = Vec::new();
    for root in roots {
        let Some(e) = root.entity else { continue };
        let candidates: Vec<(usize, Option<String>)> = if design.entities[e].is_testbench() {
            root.children
                .iter()
                .filter_map(|c| Some((c.entity?, c.architecture.clone())))
                .filter(|(c, _)| !design.entities[*c].is_testbench())
                .collect()
        } else {
            vec![(e, root.architecture.clone())]
        };
        for top in candidates {
            if !tops.iter().any(|(t, _)| *t == top.0) {
                tops.push(top);
            }
        }
    }
    tops.truncate(MAX_TOPS);
    tops
}

/// The block diagram of each top-level entity, or its symbol when it has
/// no instances.
fn top_level(design: &Design, roots: &[Node], config: &DocConfig, blocks: &mut Vec<Block>) {
    let page_link = |i: usize| format!("{}.{}", entity_page(design, i), extension(config));
    let mut diagrams = Vec::new();
    for (top, architecture) in tops(design, roots) {
        let entity = &design.entities[top];
        let arch = match &architecture {
            Some(name) => entity
                .architectures
                .iter()
                .position(|a| identifier_key(&a.name) == identifier_key(name)),
            None => entity.architectures.len().checked_sub(1),
        };
        let wired = if config.diagrams.block {
            arch.and_then(|a| crate::block::block(design, top, a, Some(&page_link)))
        } else {
            None
        };
        let page = entity_page(design, top);
        let (svg, what) = match wired {
            Some(svg) => (svg, "block"),
            None if config.diagrams.symbol => (
                svg::symbol(
                    entity,
                    svg::SymbolOptions {
                        port_types: config.diagrams.port_types,
                        generics: config.diagrams.generics,
                    },
                ),
                "symbol",
            ),
            None => continue,
        };
        let mut title = vec![link(&entity.name, page.clone(), None)];
        if let Some(a) = arch {
            title.push(text(format!(" ({})", entity.architectures[a].name)));
        }
        diagrams.push((
            title,
            Block::Diagram {
                file: format!("top.{page}.{what}"),
                alt: format!("Top level {}", entity.name),
                svg,
                mermaid: None,
            },
        ));
    }
    if diagrams.is_empty() {
        return;
    }
    blocks.push(heading(2, vec![text("Top level")], Some("top".into())));
    for (title, diagram) in diagrams {
        blocks.push(heading(3, title, None));
        blocks.push(diagram);
    }
}

/// The reset tree of each top-level entity.
fn reset_trees(design: &Design, roots: &[Node], config: &DocConfig, blocks: &mut Vec<Block>) {
    let page_link = |i: usize| format!("{}.{}", entity_page(design, i), extension(config));
    let mut trees = Vec::new();
    for (top, architecture) in tops(design, roots) {
        let entity = &design.entities[top];
        let arch = match &architecture {
            Some(name) => entity
                .architectures
                .iter()
                .position(|a| identifier_key(&a.name) == identifier_key(name)),
            None => entity.architectures.len().checked_sub(1),
        };
        let Some(arch) = arch else { continue };
        if let Some(svg) = crate::relations::resets(design, top, arch, Some(&page_link)) {
            trees.push((top, svg));
        }
    }
    if trees.is_empty() {
        return;
    }
    blocks.push(heading(2, vec![text("Reset tree")], Some("resets".into())));
    let several = trees.len() > 1;
    for (top, svg) in trees {
        let entity = &design.entities[top];
        if several {
            blocks.push(heading(
                3,
                vec![link(&entity.name, entity.page.clone(), None)],
                None,
            ));
        }
        blocks.push(Block::Diagram {
            file: format!("resets.{}", entity.page),
            alt: format!("Reset tree of {}", entity.name),
            svg,
            mermaid: None,
        });
    }
}

/// The most bit-field diagrams of one architecture or package.
const MAX_FIELD_DIAGRAMS: usize = 24;

/// The layouts of record types and of signals with aliased slices.
fn field_diagrams(
    scope: &crate::fields::Scope,
    types: &[crate::model::TypeInfo],
    signals: &[Object],
    aliases: &[crate::model::Alias],
    prefix: &str,
    level: u8,
    blocks: &mut Vec<Block>,
) {
    let mut diagrams: Vec<(String, String)> = types
        .iter()
        .filter_map(|t| Some((t.name.clone(), crate::fields::record(scope, t)?)))
        .collect();
    diagrams.extend(crate::fields::aliases(scope, signals, aliases));
    if diagrams.is_empty() {
        return;
    }
    blocks.push(heading(level, vec![text("Bit fields")], None));
    for (name, svg) in diagrams.into_iter().take(MAX_FIELD_DIAGRAMS) {
        blocks.push(Block::Diagram {
            file: format!("{prefix}.fields-{}", file_safe(&name)),
            alt: format!("Fields of {name}"),
            svg,
            mermaid: None,
        });
    }
}

/// "Uses pkg, ieee.std_logic_1164": links to documented packages.
fn uses_para(design: &Design, uses: &[crate::model::Use]) -> Option<Block> {
    if uses.is_empty() {
        return None;
    }
    let mut content = vec![text("Uses ")];
    for (i, u) in uses.iter().enumerate() {
        if i > 0 {
            content.push(text(", "));
        }
        match crate::relations::resolve_use(design, u) {
            Some(p) => content.push(link(
                &design.packages[p].name,
                design.packages[p].page.clone(),
                None,
            )),
            None => content.push(code(format!("{}.{}", u.library, u.unit))),
        }
    }
    content.push(text("."));
    Some(Block::Para(content))
}

fn index(design: &Design, roots: &[Node], config: &DocConfig) -> Page {
    let mut blocks = vec![heading(1, vec![text(design.title.clone())], None)];
    let libraries = {
        let mut l: Vec<&str> = design
            .entities
            .iter()
            .map(|e| e.library.as_str())
            .chain(design.packages.iter().map(|p| p.library.as_str()))
            .collect();
        l.sort_unstable();
        l.dedup();
        l
    };
    blocks.push(Block::Para(vec![text(format!(
        "{} entities and {} packages in {} {}.",
        design.entities.len(),
        design.packages.len(),
        libraries.len(),
        if libraries.len() == 1 {
            "library"
        } else {
            "libraries"
        }
    ))]));
    if config.diagrams.hierarchy && !roots.is_empty() {
        blocks.push(heading(
            2,
            vec![text("Design hierarchy")],
            Some("hierarchy".into()),
        ));
        let link = |i: usize| format!("{}.{}", entity_page(design, i), extension(config));
        blocks.push(Block::Diagram {
            file: "hierarchy".into(),
            alt: "Design hierarchy".into(),
            svg: svg::hierarchy(roots, Some(&link)),
            mermaid: Some(diagram_text::hierarchy_mermaid(roots)),
        });
    }
    if config.diagrams.top {
        top_level(design, roots, config, &mut blocks);
    }
    if config.diagrams.resets {
        reset_trees(design, roots, config, &mut blocks);
    }
    if config.diagrams.dependencies {
        let link = |u: crate::relations::Unit| {
            let page = match u {
                crate::relations::Unit::Entity(i) => design.entities[i].page.clone(),
                crate::relations::Unit::Package(i) => design.packages[i].page.clone(),
            };
            format!("{page}.{}", extension(config))
        };
        if let Some(svg) = crate::relations::dependencies(design, Some(&link)) {
            blocks.push(heading(
                2,
                vec![text("Package dependencies")],
                Some("dependencies".into()),
            ));
            blocks.push(Block::Diagram {
                file: "dependencies".into(),
                alt: "Package dependencies".into(),
                svg,
                mermaid: None,
            });
        }
    }
    if !design.entities.is_empty() {
        blocks.push(heading(2, vec![text("Entities")], Some("entities".into())));
        blocks.push(Block::Table {
            head: vec!["Entity", "Library", "Description"],
            rows: design
                .entities
                .iter()
                .map(|e| {
                    let mut description = brief(&e.description);
                    if e.is_testbench() && description.is_empty() {
                        description = "Testbench.".into();
                    }
                    vec![
                        vec![link(&e.name, e.page.clone(), None)],
                        vec![code(e.library.clone())],
                        vec![text(description)],
                    ]
                })
                .collect(),
        });
    }
    if !design.packages.is_empty() {
        blocks.push(heading(2, vec![text("Packages")], Some("packages".into())));
        blocks.push(Block::Table {
            head: vec!["Package", "Library", "Description"],
            rows: design
                .packages
                .iter()
                .map(|p| {
                    vec![
                        vec![link(&p.name, p.page.clone(), None)],
                        vec![code(p.library.clone())],
                        vec![text(brief(&p.description))],
                    ]
                })
                .collect(),
        });
    }
    Page {
        id: "index".into(),
        title: design.title.clone(),
        blocks,
    }
}

fn symbol_options(config: &DocConfig) -> SymbolOptions {
    SymbolOptions {
        port_types: config.diagrams.port_types,
        generics: config.diagrams.generics,
    }
}

fn entity(
    design: &Design,
    index: usize,
    users: &[(usize, String, String)],
    packages: &crate::fields::Scope,
    config: &DocConfig,
) -> Page {
    let e: &Entity = &design.entities[index];
    let id = e.page.clone();
    let mut blocks = vec![heading(
        1,
        vec![text("Entity "), code(e.name.clone())],
        None,
    )];
    let mut meta = vec![
        text("Library "),
        code(e.library.clone()),
        text(" · "),
        location(&e.file, e.line),
    ];
    if e.is_testbench() {
        meta.push(text(" · testbench"));
    }
    blocks.push(Block::Para(meta));
    blocks.extend(uses_para(design, &e.uses));
    doc(&e.description, &mut blocks, config);
    if config.diagrams.symbol && !e.is_testbench() {
        blocks.push(Block::Diagram {
            file: format!("{id}.symbol"),
            alt: format!("Symbol of {}", e.name),
            svg: svg::symbol(e, symbol_options(config)),
            mermaid: None,
        });
    }
    if !e.generics.is_empty() {
        blocks.push(heading(2, vec![text("Generics")], Some("generics".into())));
        blocks.push(generics_table(&e.generics));
    }
    if !e.ports.is_empty() {
        blocks.push(heading(2, vec![text("Ports")], Some("ports".into())));
        blocks.push(ports_table(&e.ports));
    }
    if !users.is_empty() {
        blocks.push(heading(
            2,
            vec![text("Instantiated by")],
            Some("users".into()),
        ));
        blocks.push(Block::List(
            users
                .iter()
                .map(|(parent, arch, path)| {
                    let p = &design.entities[*parent];
                    vec![
                        link(
                            &format!("{}({arch})", p.name),
                            entity_page(design, *parent),
                            Some(format!("arch-{}", identifier_key(arch))),
                        ),
                        text(" as "),
                        code(path.clone()),
                    ]
                })
                .collect(),
        ));
    }
    for arch in &e.architectures {
        architecture(design, index, arch, &id, packages, config, &mut blocks);
    }
    Page {
        id,
        title: e.name.clone(),
        blocks,
    }
}

fn architecture(
    design: &Design,
    index: usize,
    arch: &Architecture,
    page: &str,
    packages: &crate::fields::Scope,
    config: &DocConfig,
    blocks: &mut Vec<Block>,
) {
    let entity = &design.entities[index];
    let arch_key = identifier_key(&arch.name);
    blocks.push(heading(
        2,
        vec![text("Architecture "), code(arch.name.clone())],
        Some(format!("arch-{arch_key}")),
    ));
    if arch.file != entity.file {
        blocks.push(Block::Para(vec![location(&arch.file, arch.line)]));
    }
    doc(&arch.description, blocks, config);
    if !arch.instances.is_empty() {
        blocks.push(heading(3, vec![text("Instances")], None));
        let page_link = |i: usize| format!("{}.{}", entity_page(design, i), extension(config));
        let wired = config
            .diagrams
            .block
            .then(|| {
                let a = entity
                    .architectures
                    .iter()
                    .position(|a| std::ptr::eq(a, arch))?;
                crate::block::block(design, index, a, Some(&page_link))
            })
            .flatten();
        if let Some(svg) = wired {
            blocks.push(Block::Diagram {
                file: format!("{page}.{arch_key}.block"),
                alt: format!("Block diagram of {}({})", entity.name, arch.name),
                svg,
                mermaid: None,
            });
        } else if config.diagrams.hierarchy {
            let roots: Vec<Node> = Some(index)
                .map(|i| {
                    let mut node = hierarchy::subtree(
                        design,
                        i,
                        None,
                        Some(&arch.name),
                        &mut hierarchy::Expand::children(),
                    );
                    node.architecture = Some(arch.name.clone());
                    vec![node]
                })
                .unwrap_or_default();
            if roots.iter().any(|r| !r.children.is_empty()) {
                blocks.push(Block::Diagram {
                    file: format!("{page}.{arch_key}.hierarchy"),
                    alt: format!("Instances of {}({})", entity.name, arch.name),
                    svg: svg::hierarchy(&roots, Some(&page_link)),
                    mermaid: Some(diagram_text::hierarchy_mermaid(&roots)),
                });
            }
        }
        blocks.push(Block::Table {
            head: vec!["Instance", "Unit", "Kind", "Description"],
            rows: arch
                .instances
                .iter()
                .map(|instance| {
                    let unit = match hierarchy::resolve(design, instance, &entity.library) {
                        Some(target) => {
                            vec![link(&instance.unit, entity_page(design, target), None)]
                        }
                        None => vec![code(instance.unit.clone())],
                    };
                    let mut unit = unit;
                    if let Some(a) = &instance.architecture {
                        unit.push(text(format!(" ({a})")));
                    }
                    let kind = match instance.kind {
                        InstanceKind::Component => "component",
                        InstanceKind::Entity => "entity",
                        InstanceKind::Configuration => "configuration",
                    };
                    vec![
                        vec![code(instance.path())],
                        unit,
                        vec![text(kind)],
                        vec![text(instance.description.clone())],
                    ]
                })
                .collect(),
        });
        if config.diagrams.port_map && arch.instances.len() <= crate::block::MAX_BLOCKS {
            port_maps(design, index, entity, arch, page, &arch_key, blocks);
        }
    }
    if config.diagrams.dataflow
        && let Some(a) = entity
            .architectures
            .iter()
            .position(|a| std::ptr::eq(a, arch))
        && let Some(svg) = crate::block::dataflow(
            design,
            index,
            a,
            Some(&|i: usize| format!("{}.{}", entity_page(design, i), extension(config))),
        )
    {
        blocks.push(heading(3, vec![text("Data flow")], None));
        blocks.push(Block::Diagram {
            file: format!("{page}.{arch_key}.dataflow"),
            alt: format!("Data flow of {}({})", entity.name, arch.name),
            svg,
            mermaid: None,
        });
    }
    if config.diagrams.clocks
        && let Some(a) = entity
            .architectures
            .iter()
            .position(|a| std::ptr::eq(a, arch))
        && let Some((svg, domains)) = crate::clocks::diagram(design, index, a)
    {
        blocks.push(heading(3, vec![text("Clock domains")], None));
        blocks.push(Block::Diagram {
            file: format!("{page}.{arch_key}.clocks"),
            alt: format!("Clock domains of {}({})", entity.name, arch.name),
            svg,
            mermaid: None,
        });
        if !domains.crossings.is_empty() {
            blocks.push(Block::Table {
                head: vec!["Signal", "From", "To", "Read by"],
                rows: domains
                    .crossings
                    .iter()
                    .map(|c| {
                        vec![
                            vec![code(c.signal.clone())],
                            vec![code(c.from.clone())],
                            vec![code(c.to.clone())],
                            vec![code(c.reader.clone())],
                        ]
                    })
                    .collect(),
            });
        }
    }
    if !arch.processes.is_empty() {
        blocks.push(heading(3, vec![text("Processes")], None));
        blocks.push(Block::Table {
            head: vec!["Process", "Sensitivity", "Clock", "Drives", "Description"],
            rows: arch
                .processes
                .iter()
                .map(|p| {
                    let name = p
                        .label
                        .clone()
                        .unwrap_or_else(|| format!("(line {})", p.line));
                    let list = |items: &[String]| -> Cell {
                        let mut cell = Vec::new();
                        for (i, item) in items.iter().enumerate() {
                            if i > 0 {
                                cell.push(text(", "));
                            }
                            cell.push(code(item.clone()));
                        }
                        cell
                    };
                    vec![
                        if p.label.is_some() {
                            vec![code(name)]
                        } else {
                            vec![text(name)]
                        },
                        list(&p.sensitivity),
                        opt_code(&p.clock),
                        list(&p.drives),
                        vec![text(p.description.clone())],
                    ]
                })
                .collect(),
        });
    }
    for fsm in &arch.fsms {
        let signal_key = identifier_key(&fsm.signal);
        blocks.push(heading(
            3,
            vec![text("State machine "), code(fsm.signal.clone())],
            Some(format!("fsm-{arch_key}-{signal_key}")),
        ));
        let mut meta = vec![text("Type "), code(fsm.type_name.clone())];
        if let Some(next) = &fsm.next {
            meta.push(text(" · next state "));
            meta.push(code(next.clone()));
        }
        if let Some(process) = &fsm.process {
            meta.push(text(" · process "));
            meta.push(code(process.clone()));
        }
        if let Some(initial) = &fsm.initial {
            meta.push(text(" · initial state "));
            meta.push(code(initial.clone()));
        }
        blocks.push(Block::Para(meta));
        doc(&fsm.description, blocks, config);
        if config.diagrams.fsm {
            blocks.push(Block::Diagram {
                file: format!("{page}.{arch_key}.fsm-{}", file_safe(&fsm.signal)),
                alt: format!("State machine {}", fsm.signal),
                svg: svg::fsm(fsm),
                mermaid: Some(diagram_text::fsm_mermaid(fsm)),
            });
        }
        blocks.push(Block::Table {
            head: vec!["From", "To", "Condition"],
            rows: fsm
                .transitions
                .iter()
                .map(|t| {
                    vec![
                        vec![code(t.from.clone())],
                        vec![code(t.to.clone())],
                        opt_code(&t.condition),
                    ]
                })
                .collect(),
        });
    }
    if !arch.signals.is_empty() {
        blocks.push(heading(3, vec![text("Signals")], None));
        blocks.push(objects_table(&arch.signals, "Default"));
    }
    if !arch.constants.is_empty() {
        blocks.push(heading(3, vec![text("Constants")], None));
        blocks.push(objects_table(&arch.constants, "Value"));
    }
    if !arch.types.is_empty() {
        blocks.push(heading(3, vec![text("Types")], None));
        blocks.push(types_table(&arch.types));
    }
    if config.diagrams.fields {
        let scope =
            crate::fields::Scope::new(packages, design, Some(index), &arch.constants, &arch.types);
        field_diagrams(
            &scope,
            &arch.types,
            &arch.signals,
            &arch.aliases,
            &format!("{page}.{arch_key}"),
            3,
            blocks,
        );
    }
    if !arch.subprograms.is_empty() {
        blocks.push(heading(3, vec![text("Functions and procedures")], None));
        blocks.push(subprograms_table(&arch.subprograms));
    }
}

fn package_page(
    design: &Design,
    p: &Package,
    packages: &crate::fields::Scope,
    config: &DocConfig,
) -> Page {
    let id = p.page.clone();
    let mut blocks = vec![
        heading(1, vec![text("Package "), code(p.name.clone())], None),
        Block::Para(vec![
            text("Library "),
            code(p.library.clone()),
            text(" · "),
            location(&p.file, p.line),
        ]),
    ];
    blocks.extend(uses_para(design, &p.uses));
    doc(&p.description, &mut blocks, config);
    if config.constants && !p.constants.is_empty() {
        blocks.push(heading(
            2,
            vec![text("Constants")],
            Some("constants".into()),
        ));
        blocks.push(objects_table(&p.constants, "Value"));
    }
    if config.signals && !p.signals.is_empty() {
        blocks.push(heading(2, vec![text("Signals")], Some("signals".into())));
        blocks.push(objects_table(&p.signals, "Default"));
    }
    if config.types && !p.types.is_empty() {
        blocks.push(heading(2, vec![text("Types")], Some("types".into())));
        blocks.push(types_table(&p.types));
    }
    if config.diagrams.fields {
        let scope = crate::fields::Scope::new(packages, design, None, &p.constants, &p.types);
        field_diagrams(
            &scope,
            &p.types,
            &p.signals,
            &p.aliases,
            &id,
            2,
            &mut blocks,
        );
    }
    if config.subprograms && !p.subprograms.is_empty() {
        blocks.push(heading(
            2,
            vec![text("Functions and procedures")],
            Some("subprograms".into()),
        ));
        blocks.push(subprograms_table(&p.subprograms));
    }
    if !p.components.is_empty() {
        blocks.push(heading(
            2,
            vec![text("Components")],
            Some("components".into()),
        ));
        for c in &p.components {
            blocks.push(heading(
                3,
                vec![code(c.name.clone())],
                Some(format!("component-{}", identifier_key(&c.name))),
            ));
            doc(&c.description, &mut blocks, config);
            if config.diagrams.symbol {
                blocks.push(Block::Diagram {
                    file: format!("{id}.{}.symbol", file_safe(&c.name)),
                    alt: format!("Symbol of {}", c.name),
                    svg: svg::symbol_of(&c.name, &c.generics, &c.ports, symbol_options(config)),
                    mermaid: None,
                });
            }
            if !c.generics.is_empty() {
                blocks.push(generics_table(&c.generics));
            }
            if !c.ports.is_empty() {
                blocks.push(ports_table(&c.ports));
            }
        }
    }
    Page {
        id,
        title: p.name.clone(),
        blocks,
    }
}
