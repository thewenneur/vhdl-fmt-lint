//! `vfl order`: the files of the project in compile order, as a list, JSON
//! or a script for GHDL, Questa/ModelSim or Vivado.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::ValueEnum;
use vfl_project::{CompileStep, Language, Project, compile_order, find_root, verilog_standard};
use vfl_syntax::Standard;

#[derive(Clone, Copy, ValueEnum)]
pub enum OrderFormat {
    /// `library path`, one file per line.
    List,
    /// A JSON object with the files, their libraries and any cycles.
    Json,
    /// A shell script running `ghdl -a`.
    Ghdl,
    /// A Questa/ModelSim `.do` script running `vlib`, `vmap`, `vcom` and `vlog`.
    Questa,
    /// A Vivado Tcl script running `read_vhdl` and `read_verilog`.
    Vivado,
}

pub struct Options {
    pub path: PathBuf,
    pub format: OrderFormat,
    pub top: Option<String>,
    pub absolute: bool,
    pub output: Option<PathBuf>,
}

/// Prints the compile order; `Ok(false)` when it has cycles.
pub fn run(options: Options) -> Result<bool> {
    let root = find_root(&std::path::absolute(&options.path)?);
    let project = Project::load(root)?;
    for (path, error) in &project.unreadable {
        eprintln!("warning: {}: {error}", path.display());
    }
    let order = compile_order(&project, options.top.as_deref())?;
    let shown = |path: &Path| -> String {
        let path = if options.absolute {
            path
        } else {
            path.strip_prefix(&project.root.dir).unwrap_or(path)
        };
        path.to_string_lossy().replace('\\', "/")
    };
    let text = render(
        &project,
        &order.steps,
        &order.cycles,
        options.format,
        &shown,
        options.absolute,
    );
    match &options.output {
        Some(file) => {
            std::fs::write(file, text).with_context(|| format!("writing {}", file.display()))?
        }
        None => print!("{text}"),
    }
    for cycle in &order.cycles {
        let steps: Vec<String> = cycle
            .iter()
            .map(|s| format!("{} ({})", shown(&s.path), s.library))
            .collect();
        eprintln!(
            "error: these files use each other's units, so no compile order works: {}",
            steps.join(", ")
        );
    }
    Ok(order.cycles.is_empty())
}

fn render(
    project: &Project,
    steps: &[CompileStep],
    cycles: &[Vec<CompileStep>],
    format: OrderFormat,
    shown: &dyn Fn(&Path) -> String,
    absolute: bool,
) -> String {
    let standard = project.config.standard();
    let sv = |step: &CompileStep| {
        verilog_standard(&project.config, &step.path)
            .is_some_and(|s| s.as_str().starts_with("1800"))
    };
    let mut libraries: Vec<&str> = Vec::new();
    for step in steps {
        if !libraries.contains(&step.library.as_str()) {
            libraries.push(&step.library);
        }
    }
    let header = |out: &mut String, comment: &str, tool: &str| {
        let _ = writeln!(
            out,
            "{comment} Compile order of {} for {tool}, written by `vfl order`.",
            project.root.dir.display()
        );
        if !absolute {
            let _ = writeln!(
                out,
                "{comment} Paths are relative to the project root: run it from there."
            );
        }
    };
    let mut out = String::new();
    match format {
        OrderFormat::List => {
            for step in steps {
                let _ = writeln!(out, "{} {}", step.library, shown(&step.path));
            }
        }
        OrderFormat::Json => {
            let step_json = |s: &CompileStep| {
                serde_json::json!({
                    "path": shown(&s.path),
                    "library": s.library,
                    "language": match s.language {
                        Language::Vhdl => "vhdl",
                        Language::Verilog if sv(s) => "systemverilog",
                        Language::Verilog => "verilog",
                    },
                })
            };
            let value = serde_json::json!({
                "root": project.root.dir.to_string_lossy(),
                "standard": standard.as_str(),
                "libraries": libraries,
                "files": steps.iter().map(step_json).collect::<Vec<_>>(),
                "cycles": cycles
                    .iter()
                    .map(|c| c.iter().map(step_json).collect::<Vec<_>>())
                    .collect::<Vec<_>>(),
            });
            out = serde_json::to_string_pretty(&value).expect("JSON") + "\n";
        }
        OrderFormat::Ghdl => {
            out.push_str("#!/bin/sh\n");
            header(&mut out, "#", "GHDL");
            out.push_str("set -e\n");
            let std = match standard {
                Standard::Vhdl1993 => "93c",
                Standard::Vhdl2008 => "08",
                Standard::Vhdl2019 => "19",
            };
            let synopsys = if uses_synopsys_packages(project) {
                " -fsynopsys"
            } else {
                ""
            };
            for step in steps {
                let path = shell_quote(&shown(&step.path));
                match step.language {
                    Language::Vhdl => {
                        let _ = writeln!(
                            out,
                            "ghdl -a --std={std}{synopsys} --work={} {path}",
                            step.library
                        );
                    }
                    Language::Verilog => {
                        let _ = writeln!(out, "# not for GHDL (Verilog): {path}");
                    }
                }
            }
        }
        OrderFormat::Questa => {
            header(&mut out, "#", "Questa/ModelSim");
            for library in &libraries {
                let _ = writeln!(out, "vlib {library}\nvmap {library} {library}");
            }
            let std = match standard {
                Standard::Vhdl1993 => "-93",
                Standard::Vhdl2008 => "-2008",
                Standard::Vhdl2019 => "-2019",
            };
            for step in steps {
                let path = tcl_quote(&shown(&step.path));
                match step.language {
                    Language::Vhdl => {
                        let _ = writeln!(out, "vcom {std} -work {} {path}", step.library);
                    }
                    Language::Verilog => {
                        let sv = if sv(step) { " -sv" } else { "" };
                        let _ = writeln!(out, "vlog{sv} -work {} {path}", step.library);
                    }
                }
            }
        }
        OrderFormat::Vivado => {
            header(&mut out, "#", "Vivado");
            let std = match standard {
                Standard::Vhdl1993 => "",
                Standard::Vhdl2008 => " -vhdl2008",
                Standard::Vhdl2019 => " -vhdl2019",
            };
            for step in steps {
                let path = tcl_quote(&shown(&step.path));
                match step.language {
                    Language::Vhdl => {
                        let _ = writeln!(out, "read_vhdl -library {}{std} {path}", step.library);
                    }
                    Language::Verilog => {
                        let sv = if sv(step) { " -sv" } else { "" };
                        let _ = writeln!(out, "read_verilog -library {}{sv} {path}", step.library);
                    }
                }
            }
        }
    }
    out
}

/// Whether a file uses the Synopsys packages GHDL only has with
/// `-fsynopsys`.
fn uses_synopsys_packages(project: &Project) -> bool {
    project.files.iter().any(|f| {
        f.summary.refs.iter().any(|r| {
            r.library == "ieee"
                && matches!(
                    r.unit.as_str(),
                    "std_logic_arith"
                        | "std_logic_unsigned"
                        | "std_logic_signed"
                        | "std_logic_misc"
                )
        })
    })
}

fn shell_quote(text: &str) -> String {
    if text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+:@%".contains(c))
    {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

fn tcl_quote(text: &str) -> String {
    if text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+:@%".contains(c))
    {
        text.to_owned()
    } else {
        format!("{{{text}}}")
    }
}
