//! The `vfl` command-line tool.

mod doc;
mod lint;
mod new;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use vfl_config::Config;
use vfl_fmt::FormatError;
use vfl_lint::report::Format;
use vfl_project::{
    Project, collect_all_sources, collect_sources, decode, encode, find_root, parse_verilog,
};

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
        /// Format only lines FIRST to LAST (1-based, inclusive), and the
        /// statements and aligned lines they are part of. Repeatable; takes
        /// a single file or `-`.
        #[arg(long, value_name = "FIRST:LAST", value_parser = parse_lines)]
        lines: Vec<(usize, usize)>,
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
    /// Parse VHDL, Verilog and SystemVerilog files and report syntax errors.
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
    /// Generate documentation of the design, with diagrams.
    ///
    /// Writes an index, a page per entity (generics, ports, symbol,
    /// architectures, instances, processes, state machines) and a page per
    /// package, as HTML, Markdown or AsciiDoc. Doc comments are the comments
    /// directly above a declaration or at the end of its line (`--!` ones
    /// if the file has any). Settings are under `[doc]` in vfl.toml.
    Doc {
        /// A directory or file of the project (default: the current directory).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Output directory (default: `doc.out_dir`, relative to the project root).
        #[arg(long, short)]
        out: Option<PathBuf>,
        /// Output format (default: `doc.format`).
        #[arg(long, value_enum)]
        format: Option<DocFormatArg>,
        /// Title of the index page (default: `doc.title`, else the project directory's name).
        #[arg(long)]
        title: Option<String>,
        /// Print the page of the first entity (else package) this file
        /// declares instead of the index (the index if it declares none).
        #[arg(long, value_name = "FILE")]
        page_for: Option<PathBuf>,
    },
    /// Draw one diagram: an entity's symbol, the design hierarchy, the block
    /// diagram of an architecture or a state machine.
    Schema {
        #[arg(value_enum)]
        kind: doc::SchemaKind,
        /// The entity, as `name` or `library.name`. For `hierarchy`, the top
        /// (default: every entity no other one instantiates). For `wave`, a
        /// WaveJSON file.
        entity: Option<String>,
        /// For `fsm`, the state signal (default: the entity's only state machine).
        #[arg(long)]
        signal: Option<String>,
        /// For `block`, `port-map`, `dataflow`, `clocks` and `resets`, the architecture (default: the last one).
        #[arg(long)]
        architecture: Option<String>,
        /// For `port-map`, the instance label (default: the only instance).
        #[arg(long)]
        instance: Option<String>,
        /// Output format. Only `hierarchy` and `fsm` have Mermaid and DOT; the others are SVG only.
        #[arg(long, short, value_enum, default_value = "svg")]
        format: doc::SchemaFormat,
        /// Write to this file instead of standard output.
        #[arg(long, short)]
        output: Option<PathBuf>,
        /// A directory or file of the project.
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
    /// Generate code from an entity of the project: a testbench, an
    /// instantiation or a component declaration.
    ///
    /// `vfl new tb counter` writes `counter_tb.vhd` next to the entity;
    /// the other kinds print to standard output. Names, labels and the
    /// clock period come from `[generate]` in `vfl.toml`, the layout from
    /// `[format]`.
    New {
        /// What to generate.
        #[arg(value_enum)]
        kind: new::Kind,
        /// The entity, as `name` or `library.name`.
        entity: String,
        /// Where to write the code (`-` for standard output). Default: a
        /// testbench goes next to the entity, the rest to standard output.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Overwrite the output file if it exists.
        #[arg(long)]
        force: bool,
        /// A directory of the project (default: the current directory).
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
    /// Run the language server over stdio.
    Lsp,
}

#[derive(Clone, Copy, ValueEnum)]
enum DocFormatArg {
    Html,
    Markdown,
    Asciidoc,
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
        Command::Fmt {
            paths,
            check,
            lines,
        } => fmt(&paths, check, &lines).map(|()| true),
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
        Command::Doc {
            path,
            out,
            format,
            title,
            page_for,
        } => doc::doc(
            &path,
            out.as_deref(),
            format.map(|f| match f {
                DocFormatArg::Html => vfl_config::DocFormat::Html,
                DocFormatArg::Markdown => vfl_config::DocFormat::Markdown,
                DocFormatArg::Asciidoc => vfl_config::DocFormat::Asciidoc,
            }),
            title,
            page_for.as_deref(),
        )
        .map(|()| true),
        Command::Schema {
            kind,
            entity,
            signal,
            architecture,
            instance,
            format,
            output,
            project,
        } => doc::schema(&doc::SchemaOptions {
            kind,
            entity,
            signal,
            architecture,
            instance,
            format,
            output,
            project,
        })
        .map(|()| true),
        Command::New {
            kind,
            entity,
            output,
            force,
            project,
        } => new::run(new::Options {
            kind,
            entity,
            project,
            output,
            force,
        })
        .map(|()| true),
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
    for file in all_source_files(paths)? {
        let config = configs.for_file(&file)?;
        file_count += 1;
        let bytes = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
        let (text, _) = decode(bytes);
        let verilog = file
            .extension()
            .and_then(|e| e.to_str())
            .and_then(|e| config.verilog.standard_for_extension(e));
        let (dump, errors) = match verilog {
            Some(standard) => {
                let root = find_root(file.parent().unwrap_or(Path::new("."))).dir;
                let parsed = parse_verilog(&file, &text, config, &root, standard);
                (tree.then(|| parsed.debug_tree()), parsed.errors().to_vec())
            }
            None => {
                let parsed = vfl_syntax::parse(&text, config.standard());
                (tree.then(|| parsed.debug_tree()), parsed.errors().to_vec())
            }
        };
        if let Some(dump) = dump {
            print!("{dump}");
        }
        let index = vfl_syntax::LineIndex::new(&text);
        for error in &errors {
            let at = index.line_col(&text, error.range.start());
            println!(
                "{}:{}:{}: error: {}",
                file.display(),
                at.line + 1,
                at.col + 1,
                error.message
            );
        }
        error_count += errors.len();
    }
    if error_count > 0 {
        bail!("{error_count} syntax errors in {file_count} files");
    }
    eprintln!("{file_count} files checked, no syntax errors");
    Ok(())
}

/// The VHDL, Verilog and SystemVerilog sources under `paths` (the current
/// directory if empty).
fn all_source_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
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
            let (vhdl, verilog) = collect_all_sources(path, &config.project, &config.verilog)
                .with_context(|| format!("scanning {}", path.display()))?;
            files.extend(vhdl);
            files.extend(verilog);
        } else {
            files.push(path.clone());
        }
    }
    Ok(files)
}

fn parse_lines(arg: &str) -> Result<(usize, usize), String> {
    let (first, last) = arg.split_once(':').unwrap_or((arg, arg));
    let number = |s: &str| match s.trim().parse::<usize>() {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(format!("`{s}` is not a line number")),
    };
    let (first, last) = (number(first)?, number(last)?);
    if first > last {
        return Err(format!("{first} is after {last}"));
    }
    Ok((first, last))
}

/// Formats `text`, or only `lines` of it when there are some.
fn format_text(
    text: &str,
    config: &Config,
    lines: &[(usize, usize)],
) -> Result<String, FormatError> {
    if lines.is_empty() {
        return vfl_fmt::format(text, &config.format, config.standard());
    }
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let line_start = |n: usize| starts.get(n - 1).copied().unwrap_or(text.len());
    let ranges: Vec<_> = lines
        .iter()
        .map(|&(first, last)| {
            let end = starts.get(last).map_or(text.len(), |&next| next - 1);
            line_start(first)..end.max(line_start(first))
        })
        .collect();
    let edits = vfl_fmt::format_ranges(text, &ranges, &config.format, config.standard())?;
    Ok(vfl_fmt::apply_edits(text, &edits))
}

/// Formats files in place, or reports which would change with `check`.
fn fmt(paths: &[PathBuf], check: bool, lines: &[(usize, usize)]) -> Result<()> {
    if !lines.is_empty() && (paths.len() != 1 || paths[0].is_dir()) {
        bail!("--lines takes a single file");
    }
    if paths.len() == 1 && paths[0] == Path::new("-") {
        return fmt_stdin(lines);
    }
    let mut configs = ConfigCache::default();
    let mut changed = 0usize;
    let mut unchanged = 0usize;
    let mut failed = 0usize;
    for file in source_files(paths)? {
        let config = configs.for_file(&file)?;
        let bytes = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
        let (text, encoding) = decode(bytes);
        match format_text(&text, config, lines) {
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

fn fmt_stdin(lines: &[(usize, usize)]) -> Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes)?;
    let (text, encoding) = decode(bytes);
    let config = vfl_config::load_for_dir(Path::new("."))?;
    let out = format_text(&text, &config, lines)?;
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
    // Paths relative to the root, with `/` on every platform.
    let prefix = format!(
        "{}{}",
        project.root.dir.display(),
        std::path::MAIN_SEPARATOR
    );
    let relative = |text: String| text.replace(&prefix, "").replace('\\', "/");
    for (file, libraries) in project.files.iter().zip(&project.libraries) {
        let names: Vec<&str> = libraries.iter().map(|(l, _)| l.as_str()).collect();
        let shown = relative(file.path.display().to_string());
        println!("  {shown}  [{}]", names.join(", "));
        for (library, reason) in libraries {
            println!("      {library}: {}", relative(reason.to_string()));
        }
    }
    if !project.verilog.is_empty() {
        println!("verilog sources ({}):", project.verilog.len());
        for (file, libraries) in project.verilog.iter().zip(&project.verilog_libraries) {
            let names: Vec<&str> = libraries.iter().map(|(l, _)| l.as_str()).collect();
            let shown = relative(file.path.display().to_string());
            println!("  {shown}  [{}]", names.join(", "));
            for (library, reason) in libraries {
                println!("      {library}: {}", relative(reason.to_string()));
            }
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
