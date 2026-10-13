//! Documentation and diagram generation for VHDL designs.
//!
//! [`load`] reads a documentation model ([`Design`]) off the syntax trees of
//! a project: entities with their generics, ports and architectures,
//! packages, processes, instances and state machines, each with its doc
//! comments. [`generate`] turns it into Markdown or HTML pages with SVG
//! diagrams: a block symbol per entity, the instantiation tree and a state
//! diagram per state machine. The diagrams are also available alone, as
//! SVG, Mermaid or DOT ([`svg`], [`text`]).

pub mod block;
pub mod comments;
pub mod fsm;
pub mod hierarchy;
pub mod model;
mod page;
mod pages;
pub mod svg;
pub mod text;

use std::path::Path;

use globset::{GlobBuilder, GlobSetBuilder};
use vfl_config::{DocConfig, DocFormat, MarkdownDiagrams};
use vfl_project::Project;

pub use fsm::{Fsm, Transition};
pub use hierarchy::Node;
pub use model::{Design, Entity, Instance, Interface, Package};
pub use page::OutputFile;

/// Builds the documentation model of a loaded project, leaving out the
/// files `config.exclude` matches.
pub fn load(project: &Project, config: &DocConfig) -> Result<Design, globset::Error> {
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
    Ok(model::build(&inputs, config, title(project, config)))
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
    let mut files = Vec::new();
    match config.format {
        DocFormat::Markdown => {
            let mermaid = config.diagrams.markdown == MarkdownDiagrams::Mermaid;
            for page in &pages {
                let mut extra = Vec::new();
                let contents = page::markdown(page, mermaid, &mut extra);
                files.push(OutputFile {
                    path: format!("{}.md", page.id),
                    contents,
                });
                files.extend(extra);
            }
        }
        DocFormat::Html => {
            let nav = pages::nav(design);
            for page in &pages {
                files.push(OutputFile {
                    path: format!("{}.html", page.id),
                    contents: page::html(page, &design.title, &nav),
                });
            }
        }
    }
    files
}

/// Writes the documentation of `design` under `dir`. Returns the files
/// written.
pub fn write(design: &Design, config: &DocConfig, dir: &Path) -> std::io::Result<Vec<OutputFile>> {
    let files = generate(design, config);
    std::fs::create_dir_all(dir)?;
    for file in &files {
        std::fs::write(dir.join(&file.path), &file.contents)?;
    }
    Ok(files)
}
