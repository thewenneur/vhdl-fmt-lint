//! The `vfl` command-line tool.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use vfl_config::Config;
use vfl_fmt::FormatError;
use vfl_project::{collect_sources, find_root};

#[derive(Parser)]
#[command(name = "vfl", version, about = "VHDL formatter and linter")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Format VHDL files in place.
    Fmt {
        /// Files or directories (default: the project around the current
        /// directory). `-` formats standard input to standard output.
        paths: Vec<PathBuf>,
        /// Report files that would change instead of rewriting them.
        #[arg(long)]
        check: bool,
    },
    /// Lint VHDL files.
    Lint { paths: Vec<PathBuf> },
    /// Parse VHDL files and report syntax errors.
    Check {
        /// Files or directories (default: the project around the current directory).
        paths: Vec<PathBuf>,
        /// Print the syntax tree of each file.
        #[arg(long, hide = true)]
        tree: bool,
    },
    /// Show the project root and sources the tool discovers.
    Project {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Inspect configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Run the language server over stdio.
    Lsp,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Print the effective configuration for a file or directory.
    Show {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Fmt { paths, check } => fmt(&paths, check),
        Command::Lint { .. } => bail!("`vfl lint` is not implemented yet"),
        Command::Lsp => bail!("`vfl lsp` is not implemented yet"),
        Command::Check { paths, tree } => check(&paths, tree),
        Command::Project { path } => show_project(&path),
        Command::Config {
            command: ConfigCommand::Show { path },
        } => show_config(&path),
    }
}

fn config_dir(path: &Path) -> &Path {
    if path.is_file() {
        path.parent().unwrap_or(Path::new("."))
    } else {
        path
    }
}

/// The source files under `paths` (the current directory if empty).
fn source_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let default = [PathBuf::from(".")];
    let paths = if paths.is_empty() {
        &default[..]
    } else {
        paths
    };
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            let root = find_root(path);
            let config = vfl_config::load_for_dir(&root.dir)?;
            files.extend(
                collect_sources(path, &config.project)
                    .with_context(|| format!("scanning {}", path.display()))?,
            );
        } else {
            files.push(path.clone());
        }
    }
    Ok(files)
}

/// Loads each directory's effective config once.
#[derive(Default)]
struct ConfigCache(HashMap<PathBuf, Config>);

impl ConfigCache {
    fn for_file(&mut self, file: &Path) -> Result<&Config> {
        let dir = file.parent().unwrap_or(Path::new(".")).to_owned();
        if !self.0.contains_key(&dir) {
            let config = vfl_config::load_for_dir(&dir)?;
            self.0.insert(dir.clone(), config);
        }
        Ok(&self.0[&dir])
    }
}

/// Parses every file under `paths` and prints its syntax errors. Fails if any.
fn check(paths: &[PathBuf], tree: bool) -> Result<()> {
    let mut error_count = 0usize;
    let mut file_count = 0usize;
    let mut configs = ConfigCache::default();
    for file in source_files(paths)? {
        let config = configs.for_file(&file)?;
        file_count += 1;
        let bytes = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
        let (text, _) = decode(bytes);
        let parsed = vfl_syntax::parse(&text, config.standard());
        if tree {
            print!("{}", parsed.debug_tree());
        }
        let index = vfl_syntax::LineIndex::new(&text);
        for error in parsed.errors() {
            let at = index.line_col(&text, error.range.start());
            println!(
                "{}:{}:{}: error: {}",
                file.display(),
                at.line + 1,
                at.col + 1,
                error.message
            );
        }
        error_count += parsed.errors().len();
    }
    if error_count > 0 {
        bail!("{error_count} syntax errors in {file_count} files");
    }
    eprintln!("{file_count} files checked, no syntax errors");
    Ok(())
}

/// How a source file was encoded.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Latin1,
}

/// Decodes a source file: UTF-8 if valid, else Latin-1 (common in VHDL), byte for byte.
fn decode(bytes: Vec<u8>) -> (String, Encoding) {
    match String::from_utf8(bytes) {
        Ok(text) => (text, Encoding::Utf8),
        Err(err) => (
            err.into_bytes().iter().map(|&b| char::from(b)).collect(),
            Encoding::Latin1,
        ),
    }
}

fn encode(text: &str, encoding: Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        // Decoded byte for byte, and formatting only adds ASCII.
        Encoding::Latin1 => text.chars().map(|c| c as u8).collect(),
    }
}

/// Formats files in place, or reports which would change with `check`.
fn fmt(paths: &[PathBuf], check: bool) -> Result<()> {
    if paths.len() == 1 && paths[0] == Path::new("-") {
        return fmt_stdin();
    }
    let mut configs = ConfigCache::default();
    let mut changed = 0usize;
    let mut unchanged = 0usize;
    let mut failed = 0usize;
    for file in source_files(paths)? {
        let config = configs.for_file(&file)?;
        let bytes = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
        let (text, encoding) = decode(bytes);
        match vfl_fmt::format(&text, &config.format, config.standard()) {
            Ok(out) if out == text => unchanged += 1,
            Ok(out) => {
                changed += 1;
                if check {
                    println!("{}: would reformat", file.display());
                } else {
                    std::fs::write(&file, encode(&out, encoding))
                        .with_context(|| format!("writing {}", file.display()))?;
                }
            }
            Err(err) => {
                failed += 1;
                eprintln!("{}: {err}", file.display());
                if let FormatError::Syntax(errors) = &err {
                    let index = vfl_syntax::LineIndex::new(&text);
                    if let Some(first) = errors.first() {
                        let at = index.line_col(&text, first.range.start());
                        eprintln!(
                            "{}:{}:{}: error: {}",
                            file.display(),
                            at.line + 1,
                            at.col + 1,
                            first.message
                        );
                    }
                }
            }
        }
    }
    let verb = if check {
        "would be reformatted"
    } else {
        "reformatted"
    };
    eprintln!("{changed} files {verb}, {unchanged} unchanged, {failed} not formatted");
    if failed > 0 {
        bail!("{failed} files could not be formatted");
    }
    if check && changed > 0 {
        bail!("{changed} files are not formatted");
    }
    Ok(())
}

fn fmt_stdin() -> Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes)?;
    let (text, encoding) = decode(bytes);
    let config = vfl_config::load_for_dir(Path::new("."))?;
    let out = vfl_fmt::format(&text, &config.format, config.standard())?;
    std::io::stdout().write_all(&encode(&out, encoding))?;
    Ok(())
}

fn show_project(path: &Path) -> Result<()> {
    let root = find_root(path);
    let config = vfl_config::load_for_dir(&root.dir)?;
    println!("root: {} ({:?})", root.dir.display(), root.kind);
    println!("standard: {}", config.standard());
    let sources = collect_sources(&root.dir, &config.project)
        .with_context(|| format!("scanning {}", root.dir.display()))?;
    println!("sources ({}):", sources.len());
    for source in sources {
        let shown = source.strip_prefix(&root.dir).unwrap_or(&source);
        println!("  {}", shown.display());
    }
    Ok(())
}

fn show_config(path: &Path) -> Result<()> {
    let dir = config_dir(path);
    let chain = vfl_config::config_chain(dir)?;
    if chain.is_empty() {
        println!("# no config file found; showing defaults");
    }
    for file in &chain {
        println!("# from {}", file.display());
    }
    let config = vfl_config::load_layered(&chain)?;
    print!("{}", toml::to_string_pretty(&config)?);
    Ok(())
}
