//! `vfl doc` and `vfl schema`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use vfl_config::{DocConfig, DocFormat};
use vfl_doc::{Design, hierarchy, svg, text};
use vfl_project::{Project, find_root};
use vfl_syntax::identifier_key;

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SchemaKind {
    /// The block symbol of an entity, with its generics and ports.
    Symbol,
    /// The instantiation tree.
    Hierarchy,
    /// The instances of an entity's architecture, wired by their signals.
    Block,
    /// One instance of an entity's architecture, with the actual of each port.
    PortMap,
    /// A state machine of an entity's architecture.
    Fsm,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SchemaFormat {
    Svg,
    Mermaid,
    Dot,
}

pub struct SchemaOptions {
    pub kind: SchemaKind,
    pub entity: Option<String>,
    pub signal: Option<String>,
    pub architecture: Option<String>,
    pub instance: Option<String>,
    pub format: SchemaFormat,
    pub output: Option<PathBuf>,
    pub project: PathBuf,
}

fn load(path: &Path) -> Result<(Project, Design)> {
    if !path.exists() {
        bail!("{}: no such file or directory", path.display());
    }
    let root = find_root(&std::path::absolute(path)?);
    let project = Project::load(root)?;
    let design =
        vfl_doc::load(&project, &project.config.doc).context("invalid pattern in doc.exclude")?;
    Ok((project, design))
}

pub fn doc(
    path: &Path,
    out: Option<&Path>,
    format: Option<DocFormat>,
    title: Option<String>,
) -> Result<()> {
    let (project, mut design) = load(path)?;
    let mut config: DocConfig = project.config.doc.clone();
    if let Some(format) = format {
        config.format = format;
    }
    if let Some(title) = title {
        design.title = title;
    }
    let dir = match out {
        Some(out) => out.to_owned(),
        None => project.root.dir.join(&config.out_dir),
    };
    let files = vfl_doc::write(&design, &config, &dir)
        .with_context(|| format!("writing {}", dir.display()))?;
    let index = match config.format {
        DocFormat::Html => "index.html",
        DocFormat::Markdown => "index.md",
        DocFormat::Asciidoc => "index.adoc",
    };
    eprintln!(
        "{} entities, {} packages: {} files written to {}",
        design.entities.len(),
        design.packages.len(),
        files.len(),
        dir.display()
    );
    println!("{}", dir.join(index).display());
    Ok(())
}

/// The entity `name` (`name` or `library.name`) names.
fn find_entity(design: &Design, name: &str) -> Result<usize> {
    let (library, name) = match name.split_once('.') {
        Some((l, n)) => (Some(identifier_key(l)), identifier_key(n)),
        None => (None, identifier_key(name)),
    };
    let matches: Vec<usize> = design
        .entities
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e.key() == name
                && library
                    .as_ref()
                    .is_none_or(|l| identifier_key(&e.library) == *l)
        })
        .map(|(i, _)| i)
        .collect();
    match matches.as_slice() {
        [one] => Ok(*one),
        [] => bail!("no entity named `{name}` in the project"),
        _ => {
            let names: Vec<String> = matches
                .iter()
                .map(|&i| format!("{}.{}", design.entities[i].library, design.entities[i].name))
                .collect();
            bail!(
                "several entities are named `{name}`: {}; name one as `library.name`",
                names.join(", ")
            )
        }
    }
}

/// The architecture named `name`, else the last one.
fn architecture(e: &vfl_doc::model::Entity, name: Option<&str>) -> Result<usize> {
    match name {
        Some(name) => e
            .architectures
            .iter()
            .position(|a| identifier_key(&a.name) == identifier_key(name))
            .with_context(|| format!("`{}` has no architecture `{name}`", e.name)),
        None => e
            .architectures
            .len()
            .checked_sub(1)
            .with_context(|| format!("`{}` has no architecture", e.name)),
    }
}

pub fn schema(options: &SchemaOptions) -> Result<()> {
    let (project, design) = load(&options.project)?;
    let config = &project.config.doc;
    let entity = options
        .entity
        .as_deref()
        .map(|name| find_entity(&design, name))
        .transpose()?;
    let output = match options.kind {
        SchemaKind::Symbol => {
            let Some(entity) = entity else {
                bail!("`vfl schema symbol` needs an entity name");
            };
            if options.format != SchemaFormat::Svg {
                bail!("symbols are drawn as SVG only");
            }
            svg::symbol(
                &design.entities[entity],
                svg::SymbolOptions {
                    port_types: config.diagrams.port_types,
                    generics: config.diagrams.generics,
                },
            )
        }
        SchemaKind::Hierarchy => {
            let mut roots = match entity {
                Some(entity) => vec![hierarchy::subtree(
                    &design,
                    entity,
                    None,
                    None,
                    &mut hierarchy::Expand::once(),
                )],
                None => hierarchy::build(&design),
            };
            hierarchy::collapse_repeats(&mut roots);
            match options.format {
                SchemaFormat::Svg => svg::hierarchy(&roots, None),
                SchemaFormat::Mermaid => text::hierarchy_mermaid(&roots),
                SchemaFormat::Dot => text::hierarchy_dot(&roots),
            }
        }
        SchemaKind::Block => {
            let Some(entity) = entity else {
                bail!("`vfl schema block` needs an entity name");
            };
            if options.format != SchemaFormat::Svg {
                bail!("block diagrams are drawn as SVG only");
            }
            let e = &design.entities[entity];
            let arch = architecture(e, options.architecture.as_deref())?;
            match vfl_doc::block::block(&design, entity, arch, None) {
                Some(svg) => svg,
                None if e.architectures[arch].instances.is_empty() => bail!(
                    "`{}({})` instantiates nothing",
                    e.name,
                    e.architectures[arch].name
                ),
                None => bail!(
                    "`{}({})` has more than {} instances",
                    e.name,
                    e.architectures[arch].name,
                    vfl_doc::block::MAX_BLOCKS
                ),
            }
        }
        SchemaKind::PortMap => {
            let Some(entity) = entity else {
                bail!("`vfl schema port-map` needs an entity name");
            };
            if options.format != SchemaFormat::Svg {
                bail!("port maps are drawn as SVG only");
            }
            let e = &design.entities[entity];
            let arch = architecture(e, options.architecture.as_deref())?;
            let instances = &e.architectures[arch].instances;
            let index = match &options.instance {
                Some(label) => instances
                    .iter()
                    .position(|i| {
                        identifier_key(&i.path()) == identifier_key(label)
                            || identifier_key(&i.label) == identifier_key(label)
                    })
                    .with_context(|| {
                        format!(
                            "`{}({})` has no instance `{label}`",
                            e.name, e.architectures[arch].name
                        )
                    })?,
                None if instances.len() == 1 => 0,
                None => {
                    let labels: Vec<String> = instances.iter().map(|i| i.path()).collect();
                    bail!(
                        "name the instance with --instance: {}",
                        if labels.is_empty() {
                            "there is none".to_owned()
                        } else {
                            labels.join(", ")
                        }
                    )
                }
            };
            vfl_doc::block::port_map(&design, entity, arch, index)
                .context("the instance could not be drawn")?
        }
        SchemaKind::Fsm => {
            let Some(entity) = entity else {
                bail!("`vfl schema fsm` needs an entity name");
            };
            let entity = &design.entities[entity];
            let machines: Vec<_> = entity
                .architectures
                .iter()
                .flat_map(|a| a.fsms.iter())
                .collect();
            let machine = match &options.signal {
                Some(signal) => machines
                    .iter()
                    .find(|m| identifier_key(&m.signal) == identifier_key(signal))
                    .copied(),
                None if machines.len() == 1 => Some(machines[0]),
                None if machines.is_empty() => None,
                None => {
                    let names: Vec<&str> = machines.iter().map(|m| m.signal.as_str()).collect();
                    bail!(
                        "`{}` has several state machines ({}); pick one with --signal",
                        entity.name,
                        names.join(", ")
                    );
                }
            };
            let Some(machine) = machine else {
                bail!("no state machine found in `{}`", entity.name);
            };
            match options.format {
                SchemaFormat::Svg => svg::fsm(machine),
                SchemaFormat::Mermaid => text::fsm_mermaid(machine),
                SchemaFormat::Dot => text::fsm_dot(machine),
            }
        }
    };
    match &options.output {
        Some(path) => {
            std::fs::write(path, output).with_context(|| format!("writing {}", path.display()))?
        }
        None => print!("{output}"),
    }
    Ok(())
}
