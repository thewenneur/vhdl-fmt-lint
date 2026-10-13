//! The `vfl` command-line tool.

mod lint;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use vfl_config::Config;
use vfl_fmt::FormatError;
use vfl_lint::report::Format;
use vfl_project::{Project, collect_sources, decode, encode, find_root};

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
    ///
    /// The whole project around each path is analyzed, so names are
    /// resolved across files, but only the given files are reported.
    Lint {
        /// Files or directories (default: the project around the current directory).
        paths: Vec<PathBuf>,
        /// Apply safe fixes.
        #[arg(long)]
        fix: bool,
        /// Also apply fixes that may change behavior (with `--fix`).
        #[arg(long)]
        unsafe_fixes: bool,
        /// How to print diagnostics.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        output_format: OutputFormat,
        /// Fail on warnings too.
        #[arg(long)]
        deny_warnings: bool,
        /// Colorize output (default: when printing to a terminal).
        #[arg(long, value_name = "WHEN", default_value = "auto")]
        color: ColorChoice,
    },
    /// List the lint rules and their levels in each preset.
    Rules,
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
        /// Print the libraries as a rust_hdl `vhdl_ls.toml` instead.
        #[arg(long)]
        vhdl_ls: bool,
    },
    /// Inspect configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Run the language server over stdio.
    Lsp,
}

#[derive(Clone, Copy, ValueEnum)]
enum OutputFormat {
    Human,
    Concise,
    Json,
    Sarif,
    Github,
}

#[derive(Clone, Copy, ValueEnum)]
enum ColorChoice {
    Auto,
    Always,
    Never,
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
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// Runs a command; `Ok(false)` means it found problems.
fn run(cli: Cli) -> Result<bool> {
    match cli.command {
        Command::Fmt { paths, check } => fmt(&paths, check).map(|()| true),
        Command::Lint {
            paths,
            fix,
            unsafe_fixes,
            output_format,
            deny_warnings,
            color,
        } => lint::run(lint::Options {
            paths,
            fix,
            unsafe_fixes,
            format: match output_format {
                OutputFormat::Human => Format::Human,
                OutputFormat::Concise => Format::Concise,
                OutputFormat::Json => Format::Json,
                OutputFormat::Sarif => Format::Sarif,
                OutputFormat::Github => Format::Github,
            },
            deny_warnings,
            color: match color {
                ColorChoice::Auto => None,
                ColorChoice::Always => Some(true),
                ColorChoice::Never => Some(false),
            },
        }),
        Command::Rules => {
            show_rules();
            Ok(true)
        }
        Command::Lsp => vfl_lsp::run_stdio().map(|()| true),
        Command::Check { paths, tree } => check(&paths, tree).map(|()| true),
        Command::Project { path, vhdl_ls } => show_project(&path, vhdl_ls).map(|()| true),
        Command::Config {
            command: ConfigCommand::Show { path },
        } => show_config(&path).map(|()| true),
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

fn show_project(path: &Path, vhdl_ls: bool) -> Result<()> {
    let root = find_root(&std::path::absolute(path)?);
    if vhdl_ls {
        let project = Project::load(root)?;
        let mut libraries: Vec<(String, Vec<String>)> = Vec::new();
        for (file, libs) in project.files.iter().zip(&project.libraries) {
            let shown = file
                .path
                .strip_prefix(&project.root.dir)
                .unwrap_or(&file.path);
            let shown = shown.to_string_lossy().replace('\\', "/");
            for (library, _) in libs {
                // rust_hdl rejects a library named `work`.
                let library = if library == "work" {
                    "defaultlib"
                } else {
                    library.as_str()
                };
                match libraries.iter_mut().find(|(l, _)| l == library) {
                    Some((_, files)) => files.push(shown.clone()),
                    None => libraries.push((library.to_owned(), vec![shown.clone()])),
                }
            }
        }
        let mut table = toml::Table::new();
        for (library, files) in libraries {
            let mut entry = toml::Table::new();
            entry.insert("files".into(), files.into());
            table.insert(library, entry.into());
        }
        let mut out = toml::Table::new();
        out.insert("libraries".into(), table.into());
        print!("{}", toml::to_string(&out)?);
        return Ok(());
    }
    println!("root: {} ({:?})", root.dir.display(), root.kind);
    let project = Project::load(root)?;
    println!("standard: {}", project.config.standard());
    println!("sources ({}):", project.files.len());
    let dir = project.root.dir.clone();
    for (file, libraries) in project.files.iter().zip(&project.libraries) {
        let shown = file.path.strip_prefix(&dir).unwrap_or(&file.path);
        let names: Vec<&str> = libraries.iter().map(|(l, _)| l.as_str()).collect();
        println!("  {}  [{}]", shown.display(), names.join(", "));
        for (library, reason) in libraries {
            println!("      {library}: {reason}");
        }
    }
    for (path, error) in &project.unreadable {
        println!("unreadable: {}: {error}", path.display());
    }
    for note in &project.notes {
        println!("note: {note}");
    }
    Ok(())
}

fn show_rules() {
    let name = |level: vfl_config::Level| match level {
        vfl_config::Level::Off => "off",
        vfl_config::Level::Info => "info",
        vfl_config::Level::Warn => "warn",
        vfl_config::Level::Error => "error",
    };
    println!(
        "{:<30} {:<12} {:<7} {:<4} summary",
        "rule", "recommended", "strict", "fix"
    );
    for rule in vfl_lint::RULES {
        println!(
            "{:<30} {:<12} {:<7} {:<4} {}",
            rule.id,
            name(rule.recommended),
            name(rule.strict),
            if rule.fixable { "yes" } else { "" },
            rule.summary
        );
    }
    for (kind, what) in vfl_lint::NAMING_KINDS {
        println!(
            "{:<30} {:<12} {:<7} {:<4} Naming convention for {what} (on when configured).",
            format!("naming/{kind}"),
            "off",
            "off",
            ""
        );
    }
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
