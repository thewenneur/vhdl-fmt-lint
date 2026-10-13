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
