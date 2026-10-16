//! Documentation and diagram generation for VHDL designs, with the
//! Verilog and SystemVerilog modules they instantiate.
//!
//! [`load`] reads a documentation model ([`Design`]) off the syntax trees of
//! a project: entities with their generics, ports and architectures,
//! packages, processes, instances and state machines, each with its doc
//! comments. [`generate`] turns it into Markdown or HTML pages with SVG
//! diagrams: a block symbol per entity, the instantiation tree and a state
//! diagram per state machine. The diagrams are also available alone, as
//! SVG, Mermaid or DOT ([`svg`], [`text`]).

pub mod block;
pub mod clocks;
pub mod comments;
pub mod fields;
pub mod fsm;
pub mod graph;
pub mod hierarchy;
mod layered;
pub mod model;
mod page;
mod pages;
pub mod relations;
pub mod spec;
pub mod svg;
pub mod text;
pub mod theme;
mod verilog;
pub mod wave;

use std::path::Path;

use globset::{GlobBuilder, GlobSetBuilder};
use vfl_config::{DocConfig, DocFormat, MarkdownDiagrams};
use vfl_project::Project;

pub use fsm::{Fsm, Transition};
pub use hierarchy::Node;
pub use model::{Design, Entity, Instance, Interface, Package};
pub use page::OutputFile;
pub use theme::Theme;

/// Builds the documentation model of a loaded project, leaving out the
/// files `config.exclude` matches.
pub fn load(project: &Project, config: &DocConfig) -> Result<Design, globset::Error> {
    load_inner(project, config, false).map(|(design, _)| design)
}

/// Builds the documentation model of a loaded project with its spec
/// comments, for [`spec::generate`].
pub fn load_spec(
    project: &Project,
    config: &DocConfig,
) -> Result<(Design, Vec<spec::SpecItem>), globset::Error> {
    load_inner(project, config, true)
}

fn load_inner(
    project: &Project,
    config: &DocConfig,
    with_spec: bool,
) -> Result<(Design, Vec<spec::SpecItem>), globset::Error> {
    let mut excluded = GlobSetBuilder::new();
    for pattern in &config.exclude {
        let pattern = pattern.trim_start_matches('/');
        let pattern = if pattern.contains('/') {
            pattern.to_owned()
        } else {
            format!("**/{pattern}")
        };
        excluded.add(GlobBuilder::new(&pattern).literal_separator(true).build()?);
        // A directory pattern covers what is under it.
        excluded.add(
            GlobBuilder::new(&format!("{}/**", pattern.trim_end_matches('/')))
                .literal_separator(true)
                .build()?,
        );
    }
    let excluded = excluded.build()?;
    let trees: Vec<_> = project.files.iter().map(|f| f.parse.syntax()).collect();
    let mut inputs = Vec::new();
    for ((file, libraries), root) in project.files.iter().zip(&project.libraries).zip(&trees) {
        let path = model::relative_path(&project.root.dir, &file.path);
        if excluded.is_match(&path) {
            continue;
        }
        let library = libraries
            .first()
            .map(|(l, _)| l.clone())
            .unwrap_or_else(|| "work".into());
        inputs.push(model::FileInput {
            root,
            text: &file.text,
            path,
            library,
        });
    }
    let mut verilog = Vec::new();
    for (file, libraries) in project.verilog.iter().zip(&project.verilog_libraries) {
        let path = model::relative_path(&project.root.dir, &file.path);
        if excluded.is_match(&path) {
            continue;
        }
        let library = libraries.first().map_or("work", |(l, _)| l.as_str());
        verilog.extend(verilog::entities(&file.modules, &file.text, &path, library));
    }
    let items = if with_spec {
        spec::collect(&inputs, config)
    } else {
        Vec::new()
    };
    let design = model::build(&inputs, verilog, config, title(project, config));
    Ok((design, items))
}

fn title(project: &Project, config: &DocConfig) -> String {
    config.title.clone().unwrap_or_else(|| {
        project
            .root
            .dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Design".into())
    })
}

/// The files of the documentation of `design`, relative to the output
/// directory.
pub fn generate(design: &Design, config: &DocConfig) -> Vec<OutputFile> {
    let mut roots = hierarchy::build(design);
    hierarchy::collapse_repeats(&mut roots);
    let pages = pages::pages(design, &roots, config);
    let nav = match config.format {
        DocFormat::Html => pages::nav(design),
        _ => Vec::new(),
    };
    render(&pages, &design.title, &nav, config)
}

/// The files of `pages` in `config.format`.
fn render(
    pages: &[page::Page],
    title: &str,
    nav: &[page::NavItem],
    config: &DocConfig,
) -> Vec<OutputFile> {
    let mut files = Vec::new();
    match config.format {
        DocFormat::Markdown => {
            let mermaid = config.diagrams.markdown == MarkdownDiagrams::Mermaid;
            for page in pages {
                let mut extra = Vec::new();
                let contents = page::markdown(page, mermaid, &mut extra);
                files.push(OutputFile {
                    path: format!("{}.md", page.id),
                    contents,
                });
                files.extend(extra);
            }
        }
        DocFormat::Asciidoc => {
            for page in pages {
                let mut extra = Vec::new();
                let contents = page::asciidoc(page, &mut extra);
                files.push(OutputFile {
                    path: format!("{}.adoc", page.id),
                    contents,
                });
                files.extend(extra);
            }
        }
        DocFormat::Html => {
            for page in pages {
                files.push(OutputFile {
                    path: format!("{}.html", page.id),
                    contents: page::html(page, title, nav),
                });
            }
        }
    }
    let theme = theme::Theme::new(&config.theme);
    for file in &mut files {
        if let std::borrow::Cow::Owned(themed) = theme.apply(&file.contents) {
            file.contents = themed;
        }
    }
    files
}

/// Writes the documentation of `design` under `dir`. Returns the files
/// written.
pub fn write(design: &Design, config: &DocConfig, dir: &Path) -> std::io::Result<Vec<OutputFile>> {
    write_files(generate(design, config), dir)
}

/// Writes the specification of `design` under `dir`. Returns the files
/// written.
pub fn write_spec(
    design: &Design,
    items: &[spec::SpecItem],
    config: &DocConfig,
    dir: &Path,
) -> std::io::Result<Vec<OutputFile>> {
    write_files(spec::generate(design, items, config), dir)
}

fn write_files(files: Vec<OutputFile>, dir: &Path) -> std::io::Result<Vec<OutputFile>> {
    std::fs::create_dir_all(dir)?;
    for file in &files {
        std::fs::write(dir.join(&file.path), &file.contents)?;
    }
    Ok(files)
}

/// The id of the page documenting `file`, a file of the project rooted at
/// `root`: its first entity, else its first package.
pub fn page_for<'a>(design: &'a Design, root: &Path, file: &Path) -> Option<&'a str> {
    let mut path = model::relative_path(root, file);
    if Path::new(&path).is_absolute() {
        // Not under the root as written: compare the resolved paths.
        path = model::relative_path(&root.canonicalize().ok()?, &file.canonicalize().ok()?);
    }
    let entity = design
        .entities
        .iter()
        .filter(|e| e.file == path)
        .min_by_key(|e| e.line);
    match entity {
        Some(e) => Some(&e.page),
        None => design
            .packages
            .iter()
            .filter(|p| p.file == path)
            .min_by_key(|p| p.line)
            .map(|p| p.page.as_str()),
    }
}
