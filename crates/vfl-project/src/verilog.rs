//! The Verilog and SystemVerilog files of a project: parsed with the
//! `[verilog]` settings and summarized, so that VHDL can instantiate their
//! modules.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use vfl_config::{Config, VerilogStandard};
use vfl_sema::{ForeignModule, ForeignObject, Mode};
use vfl_verilog_syntax::summary::{self, Direction, Module};

use crate::load::{Encoding, decode};
use crate::summary::{FileSummary, UnitKind, UnitRef, UnitSummary};

/// A parsed Verilog or SystemVerilog file.
#[derive(Debug, Clone)]
pub struct VerilogFile {
    pub path: PathBuf,
    pub text: Arc<str>,
    pub encoding: Encoding,
    pub parse: vfl_verilog_syntax::Parse,
    /// Its modules; shared, so that an analysis can tell an unchanged file
    /// by pointer.
    pub modules: Arc<[Module]>,
    /// Its modules as design units, and the modules it instantiates, for
    /// library inference.
    pub summary: FileSummary,
    /// Its modules as the analysis of VHDL sees them; shared, so that an
    /// analysis can tell an unchanged file by pointer.
    pub foreign: Arc<[ForeignModule]>,
}

impl VerilogFile {
    /// Parses `text` with the settings of `config` and summarizes it.
    /// Included files are looked for in the file's directory, then in the
    /// configured include directories (relative to `root`).
    pub fn new(
        path: PathBuf,
        text: String,
        encoding: Encoding,
        config: &Config,
        root: &Path,
        standard: VerilogStandard,
    ) -> Self {
        let parse = parse_verilog(&path, &text, config, root, standard);
        let modules: Arc<[Module]> = summary::modules(&parse.syntax()).into();
        let summary = file_summary(&modules);
        let foreign = modules.iter().map(foreign_module).collect();
        VerilogFile {
            path,
            text: text.into(),
            encoding,
            parse,
            modules,
            summary,
            foreign,
        }
    }

    /// Reads, decodes and parses a file.
    pub fn read(
        path: PathBuf,
        config: &Config,
        root: &Path,
        standard: VerilogStandard,
    ) -> std::io::Result<Self> {
        let (text, encoding) = decode(std::fs::read(&path)?);
        Ok(Self::new(path, text, encoding, config, root, standard))
    }
}

/// The Verilog standard of `path` by its extension, if it is a Verilog or
/// SystemVerilog source as `config` sees it.
pub fn verilog_standard(config: &Config, path: &Path) -> Option<VerilogStandard> {
    path.extension()
        .and_then(|e| e.to_str())
        .and_then(|e| config.verilog.standard_for_extension(e))
}

/// Parses a Verilog or SystemVerilog file, reading the files it includes
/// (for the macros they define) from its directory and the configured
/// include directories, relative to `root`.
pub fn parse_verilog(
    path: &Path,
    text: &str,
    config: &Config,
    root: &Path,
    standard: VerilogStandard,
) -> vfl_verilog_syntax::Parse {
    let standard = standard
        .as_str()
        .parse()
        .expect("config standards are parser standards");
    let dir = path.parent().unwrap_or(Path::new(".")).to_owned();
    let search: Vec<PathBuf> = std::iter::once(dir)
        .chain(config.verilog.include_dirs.iter().map(|d| root.join(d)))
        .collect();
    let read = |name: &str| -> Option<String> {
        search
            .iter()
            .map(|d| d.join(name))
            .find(|p| p.is_file())
            .and_then(|p| std::fs::read(p).ok())
            .map(|bytes| decode(bytes).0)
    };
    vfl_verilog_syntax::parse_with(
        text,
        &vfl_verilog_syntax::Options {
            standard,
            defines: &config.verilog.defines,
            include: Some(&read),
        },
    )
}

/// The modules as entities, and the modules they instantiate as implied
/// `work` references: a module goes in the libraries of the units that
/// instantiate it, like a VHDL entity.
fn file_summary(modules: &[Module]) -> FileSummary {
    let units = modules
        .iter()
        .map(|m| UnitSummary {
            kind: UnitKind::Entity,
            name: module_key(&m.name),
            primary: None,
            name_range: m.name_range,
        })
        .collect();
    let mut refs: Vec<UnitRef> = modules
        .iter()
        .flat_map(|m| &m.instances)
        .map(|i| UnitRef {
            library: "work".into(),
            unit: module_key(&i.module),
            implicit: true,
        })
        .collect();
    refs.sort_by(|a, b| a.unit.cmp(&b.unit));
    refs.dedup();
    FileSummary { units, refs }
}

/// The key a module is known by in VHDL: its name in lower case, as VHDL
/// identifiers are not case sensitive. An escaped identifier keeps its
/// text without the backslash.
pub fn module_key(name: &str) -> String {
    name.strip_prefix('\\')
        .unwrap_or(name)
        .trim_end()
        .to_ascii_lowercase()
}

fn foreign_module(module: &Module) -> ForeignModule {
    ForeignModule {
        name: module.name.clone(),
        name_range: module.name_range,
        range: module.header,
        generics: module
            .parameters
            .iter()
            .map(|p| ForeignObject {
                name: p.name.clone(),
                name_range: p.name_range,
                range: p.range,
                mode: None,
                has_default: p.default.is_some(),
                width: None,
                value: p.value,
                declaration: p.declaration(),
            })
            .collect(),
        ports: module
            .ports
            .iter()
            .map(|p| ForeignObject {
                name: p.name.clone(),
                name_range: p.name_range,
                range: p.range,
                mode: p.direction.map(|d| match d {
                    Direction::Input => Mode::In,
                    Direction::Output => Mode::Out,
                    Direction::Inout | Direction::Ref => Mode::InOut,
                }),
                has_default: p.default.is_some(),
                width: p.width,
                value: None,
                declaration: p.declaration(),
            })
            .collect(),
    }
}
