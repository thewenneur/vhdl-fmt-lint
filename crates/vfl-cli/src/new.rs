//! `vfl new`: code generated from an entity of the project.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use vfl_gen::{Found, Source};
use vfl_project::{Encoding, Project, encode, find_root};

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Kind {
    /// A testbench, written to a new file.
    #[value(alias = "testbench")]
    Tb,
    /// An entity instantiation (`u_e : entity work.e port map (...)`).
    #[value(alias = "inst")]
    Instance,
    /// A component declaration.
    #[value(alias = "comp")]
    Component,
    /// A component instantiation (`u_e : e port map (...)`).
    ComponentInstance,
}

pub struct Options {
    pub kind: Kind,
    pub entity: String,
    pub project: PathBuf,
    pub output: Option<PathBuf>,
    pub force: bool,
}

pub fn run(options: Options) -> Result<()> {
    let root = find_root(&std::path::absolute(&options.project)?);
    let project = Project::load(root)?;
    let sources: Vec<Source> = project
        .files
        .iter()
        .zip(&project.libraries)
        .map(|(file, libraries)| Source {
            path: &file.path,
            root: file.parse.syntax(),
            libraries: libraries.iter().map(|(l, _)| l.clone()).collect(),
        })
        .collect();
    let mut found = vfl_gen::find_entities(&sources, Some(&options.entity));
    let found: Found = match found.len() {
        0 => bail!(
            "no entity `{}` in the project at {}",
            options.entity,
            project.root.dir.display()
        ),
        1 => found.pop().unwrap(),
        _ => {
            let places: Vec<String> = found
                .iter()
                .map(|f| {
                    format!(
                        "{}.{} ({})",
                        f.entity.library,
                        f.entity.name,
                        f.path.display()
                    )
                })
                .collect();
            bail!(
                "several entities are named `{}`; name one as `library.entity`:\n  {}",
                options.entity,
                places.join("\n  ")
            );
        }
    };
    // The settings of the directory the code is for.
    let dir = found.path.parent().unwrap_or(Path::new("."));
    let config = vfl_config::load_for_dir(dir)?;
    let entity = &found.entity;
    let text = match options.kind {
        Kind::Tb => vfl_gen::testbench(entity, &config)?,
        Kind::Instance => vfl_gen::instance(entity, &config)?,
        Kind::Component => vfl_gen::component(entity, &config)?,
        Kind::ComponentInstance => vfl_gen::component_instance(entity, &config)?,
    };
    // A testbench goes to a new file next to the entity; the rest to stdout.
    let output = match (&options.output, options.kind) {
        (Some(path), _) if path == Path::new("-") => None,
        (Some(path), _) => Some(path.clone()),
        (None, Kind::Tb) => {
            let extension = found
                .path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("vhd");
            Some(dir.join(vfl_gen::testbench_file_name(
                entity,
                &config.generate,
                extension,
            )))
        }
        (None, _) => None,
    };
    match output {
        None => print!("{text}"),
        Some(path) => {
            if path.exists() && !options.force {
                bail!("{} exists; pass --force to overwrite it", path.display());
            }
            let encoding = project
                .files
                .iter()
                .find(|f| f.path == found.path)
                .map(|f| f.encoding)
                .unwrap_or(Encoding::Utf8);
            std::fs::write(&path, encode(&text, encoding))
                .with_context(|| format!("writing {}", path.display()))?;
            eprintln!("wrote {}", path.display());
        }
    }
    Ok(())
}
