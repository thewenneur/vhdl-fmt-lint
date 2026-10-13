//! The `vfl` command-line tool.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
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
        Command::Fmt { .. } => bail!("`vfl fmt` is not implemented yet"),
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

/// Parses every file under `paths` and prints its syntax errors. Fails if any.
fn check(paths: &[PathBuf], tree: bool) -> Result<()> {
    let default = [PathBuf::from(".")];
    let paths = if paths.is_empty() {
        &default[..]
    } else {
        paths
    };
    let mut error_count = 0usize;
    let mut file_count = 0usize;
    for path in paths {
        let root = find_root(path);
        let config = vfl_config::load_for_dir(&root.dir)?;
        let files = if path.is_dir() {
            collect_sources(path, &config.project)
                .with_context(|| format!("scanning {}", path.display()))?
        } else {
            vec![path.clone()]
        };
        for file in files {
            file_count += 1;
            let bytes =
                std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            let text = decode(bytes);
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
    }
    if error_count > 0 {
        bail!("{error_count} syntax errors in {file_count} files");
    }
    eprintln!("{file_count} files checked, no syntax errors");
    Ok(())
}

/// Decodes a source file: UTF-8 if valid, else Latin-1 (common in VHDL), byte for byte.
fn decode(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => err.into_bytes().iter().map(|&b| char::from(b)).collect(),
    }
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
