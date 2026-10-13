//! Library lists of other tools found in the tree: VUnit `run.py` scripts,
//! FuseSoC `.core` files, Vivado projects (`.xpr`) and Tcl scripts
//! (`read_vhdl`), Quartus settings (`.qsf`, `.qip`). Each is read without
//! running anything: only what can be seen statically counts, and a file
//! that cannot be understood contributes nothing.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use yaml_rust2::{Yaml, YamlLoader};

/// Library name to path patterns, relative to the tool file's directory
/// unless absolute.
pub type Libraries = BTreeMap<String, Vec<String>>;

/// Whether `path` may be a tool file worth reading.
pub fn is_tool_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name == "run.py" {
        return true;
    }
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("core" | "xpr" | "qsf" | "qip" | "tcl")
    )
}

/// The library assignments a tool file makes, if it is one.
pub fn import(path: &Path) -> Option<Libraries> {
    let text = std::fs::read_to_string(path).ok()?;
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let libraries = match extension.as_deref() {
        Some("py") => vunit(&text),
        Some("core") => fusesoc(&text),
        Some("xpr") => vivado_project(&text, path),
        Some("tcl") => vivado_tcl(&text, path),
        Some("qsf" | "qip") => quartus(&text, path),
        _ => return None,
    };
    (!libraries.is_empty()).then_some(libraries)
}

fn is_vhdl_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".vhd", ".vhdl", ".vho", ".vht"]
        .iter()
        .any(|e| lower.ends_with(e))
}

/// A literal path as a glob pattern.
fn literal(path: &str) -> String {
    globset::escape(&path.replace('\\', "/"))
}

fn add(libraries: &mut Libraries, library: &str, pattern: String) {
    let patterns = libraries.entry(library.to_owned()).or_default();
    if !patterns.contains(&pattern) {
        patterns.push(pattern);
    }
}

// ---- VUnit -------------------------------------------------------------------

/// A Python token, as far as reading `run.py` needs.
#[derive(Debug, Clone, PartialEq)]
enum Py {
    Name(String),
    Str(String),
    Punct(char),
}

fn python_tokens(text: &str) -> Vec<Py> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '#' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            // String prefixes: r"", f"", rb"" ...
            if i < chars.len()
                && (chars[i] == '"' || chars[i] == '\'')
                && word.len() <= 2
                && word.chars().all(|c| "rRfFbBuU".contains(c))
            {
                continue;
            }
            tokens.push(Py::Name(word));
        } else if c == '"' || c == '\'' {
            let triple = chars.get(i + 1) == Some(&c) && chars.get(i + 2) == Some(&c);
            let quote_len = if triple { 3 } else { 1 };
            i += quote_len;
            let mut value = String::new();
            while i < chars.len() {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    value.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                if chars[i] == c
                    && (!triple || (chars.get(i + 1) == Some(&c) && chars.get(i + 2) == Some(&c)))
                {
                    i += quote_len;
                    break;
                }
                if !triple && chars[i] == '\n' {
                    break;
                }
                value.push(chars[i]);
                i += 1;
            }
            tokens.push(Py::Str(value));
        } else if c == '\\' && chars.get(i + 1) == Some(&'\n') {
            // A line continuation.
            i += 2;
        } else if c == '\n' {
            tokens.push(Py::Punct('\n'));
            i += 1;
        } else if c.is_whitespace() {
            i += 1;
        } else {
            tokens.push(Py::Punct(c));
            i += 1;
        }
    }
    tokens
}

/// The path an expression builds from its string literals and known
/// variables, in order: `ROOT / "src" / "*.vhd"`, `join(root, "a.vhd")`,
/// `f"{root}/src/*.vhd"`. Leading parts that are not known are taken as
/// the script's directory.
fn python_path(tokens: &[Py], variables: &HashMap<String, String>) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for token in tokens {
        match token {
            Py::Str(s) => {
                // An f-string placeholder at the start: `{root}/src`.
                let s = match s.strip_prefix('{') {
                    Some(rest) => match rest.find('}') {
                        Some(end) => {
                            let name = &rest[..end];
                            let value = variables.get(name).cloned().unwrap_or_default();
                            format!("{value}/{}", &rest[end + 1..])
                        }
                        None => s.clone(),
                    },
                    None => s.clone(),
                };
                parts.push(s);
            }
            Py::Name(n) => {
                if let Some(value) = variables.get(n) {
                    parts.push(value.clone());
                }
            }
            Py::Punct(_) => {}
        }
    }
    let joined = parts
        .iter()
        .map(|p| p.trim_matches('/'))
        .filter(|p| !p.is_empty() && *p != ".")
        .collect::<Vec<_>>()
        .join("/");
    (!joined.is_empty()).then_some(joined)
}

/// Splits `tokens[start..]` (just after an opening parenthesis) at its
/// top-level commas, up to the closing parenthesis.
fn call_arguments(tokens: &[Py], start: usize) -> Vec<&[Py]> {
    let mut depth = 0;
    let mut args = Vec::new();
    let mut from = start;
    for (i, token) in tokens.iter().enumerate().skip(start) {
        match token {
            Py::Punct('(' | '[' | '{') => depth += 1,
            Py::Punct(')' | ']' | '}') if depth > 0 => depth -= 1,
            Py::Punct(')') => {
                if i > from {
                    args.push(&tokens[from..i]);
                }
                return args;
            }
            Py::Punct(',') if depth == 0 => {
                args.push(&tokens[from..i]);
                from = i + 1;
            }
            _ => {}
        }
    }
    args
}

/// The tokens of a statement starting at `start`: up to the end of the
/// logical line (balanced brackets).
fn statement_end(tokens: &[Py], start: usize) -> usize {
    let mut depth = 0i32;
    for (i, token) in tokens.iter().enumerate().skip(start) {
        match token {
            Py::Punct('(' | '[' | '{') => depth += 1,
            Py::Punct(')' | ']' | '}') => {
                depth -= 1;
                if depth < 0 {
                    return i;
                }
            }
            Py::Punct(';' | '\n') if depth == 0 => return i,
            Py::Name(n) if depth == 0 && i > start && is_statement_keyword(n) => return i,
            _ => {}
        }
    }
    tokens.len()
}

fn is_statement_keyword(name: &str) -> bool {
    matches!(
        name,
        "def" | "for" | "if" | "import" | "from" | "return" | "with" | "class"
    )
}

/// `lib = vu.add_library("lib")` and `lib.add_source_files("src/*.vhd")`.
fn vunit(text: &str) -> Libraries {
    let tokens = python_tokens(text);
    let mut libraries = Libraries::new();
    // Variables holding a library, and variables holding a path.
    let mut library_vars: HashMap<String, String> = HashMap::new();
    let mut path_vars: HashMap<String, String> = HashMap::new();
    let mut i = 0;
    while i < tokens.len() {
        // `NAME = ...`
        if let (Some(Py::Name(var)), Some(Py::Punct('=')), next) =
            (tokens.get(i), tokens.get(i + 1), tokens.get(i + 2))
            && next != Some(&Py::Punct('='))
        {
            let end = statement_end(&tokens, i + 2);
            let value = &tokens[i + 2..end];
            if let Some(library) = library_in(value) {
                library_vars.insert(var.clone(), library);
            } else if value
                .iter()
                .any(|t| matches!(t, Py::Name(n) if n == "__file__") || matches!(t, Py::Str(_)))
                && !value
                    .iter()
                    .any(|t| matches!(t, Py::Name(n) if n.starts_with("add_")))
            {
                let value = python_path(value, &path_vars).unwrap_or_default();
                path_vars.insert(var.clone(), value);
            }
        }
        // `X.add_source_files(...)` or `X.add_source_file(...)`.
        if let Some(Py::Name(method)) = tokens.get(i)
            && (method == "add_source_files" || method == "add_source_file")
            && tokens.get(i + 1) == Some(&Py::Punct('('))
            && i >= 2
            && tokens[i - 1] == Py::Punct('.')
        {
            let library = match &tokens[i - 2] {
                Py::Name(var) => library_vars.get(var).cloned(),
                // `vu.add_library("lib").add_source_files(...)`.
                Py::Punct(')') => {
                    let open = (0..i - 2).rev().find(|&j| tokens[j] == Py::Punct('('));
                    open.and_then(|j| library_in(&tokens[j.saturating_sub(1)..i - 1]))
                }
                _ => None,
            };
            let args = call_arguments(&tokens, i + 2);
            if let (Some(library), Some(first)) = (library, args.first())
                && let Some(pattern) = python_path(first, &path_vars)
            {
                add(&mut libraries, &library, pattern);
            }
        }
        i += 1;
    }
    // OSVVM comes with VUnit: adding a file to it must not make the few
    // files in the tree look like the whole library. (`vunit_lib` is kept:
    // only VUnit's own tests add to it, next to its sources.)
    let builtins = tokens.iter().any(|t| {
        matches!(t, Py::Name(n) if matches!(
            n.as_str(),
            "add_vhdl_builtins" | "add_osvvm" | "add_builtins" | "add_verification_components"
                | "add_random" | "add_com" | "add_json4vhdl" | "add_array_util"
        ))
    });
    if builtins {
        libraries.retain(|name, _| !name.eq_ignore_ascii_case("osvvm"));
    }
    libraries
}

/// The library named by `add_library("name")` or `library("name")` in
/// `tokens`.
fn library_in(tokens: &[Py]) -> Option<String> {
    let tokens: Vec<&Py> = tokens.iter().filter(|t| **t != Py::Punct('\n')).collect();
    tokens.windows(3).find_map(|w| match w {
        [Py::Name(m), Py::Punct('('), Py::Str(name)] if m == "add_library" || m == "library" => {
            Some(name.clone())
        }
        _ => None,
    })
}

// ---- FuseSoC -----------------------------------------------------------------

/// `filesets: { name: { files: [...], file_type: ..., logical_name: lib } }`.
fn fusesoc(text: &str) -> Libraries {
    let mut libraries = Libraries::new();
    let Ok(documents) = YamlLoader::load_from_str(text) else {
        return libraries;
    };
    let Some(document) = documents.first() else {
        return libraries;
    };
    let Yaml::Hash(filesets) = &document["filesets"] else {
        return libraries;
    };
    for fileset in filesets.values() {
        let fileset_library = fileset["logical_name"].as_str();
        let fileset_type = fileset["file_type"].as_str();
        let Yaml::Array(files) = &fileset["files"] else {
            continue;
        };
        for file in files {
            let (path, library, file_type) = match file {
                Yaml::String(path) => (path.as_str(), fileset_library, fileset_type),
                Yaml::Hash(entry) => {
                    let Some((Yaml::String(path), options)) = entry.iter().next() else {
                        continue;
                    };
                    (
                        path.as_str(),
                        options["logical_name"].as_str().or(fileset_library),
                        options["file_type"].as_str().or(fileset_type),
                    )
                }
                _ => continue,
            };
            let vhdl = match file_type {
                Some(t) => t.starts_with("vhdlSource"),
                None => is_vhdl_path(path),
            };
            if vhdl && let Some(library) = library {
                add(&mut libraries, library, literal(path));
            }
        }
    }
    libraries
}

// ---- Vivado ------------------------------------------------------------------

/// The value of attribute `name` in an XML start tag.
fn xml_attribute<'t>(tag: &'t str, name: &str) -> Option<&'t str> {
    let at = tag.find(&format!("{name}=\""))? + name.len() + 2;
    let end = tag[at..].find('"')?;
    Some(&tag[at..at + end])
}

fn xml_unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// `<File Path="$PSRCDIR/sources_1/new/top.vhd"> ... <Attr Name="Library"
/// Val="lib"/> ... </File>`. Files without a library go to
/// `xil_defaultlib`, as in Vivado.
fn vivado_project(text: &str, path: &Path) -> Libraries {
    let mut libraries = Libraries::new();
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let mut rest = text;
    while let Some(start) = rest.find("<File ") {
        rest = &rest[start..];
        let tag_end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..tag_end];
        let self_closing = tag.ends_with('/');
        let body_end = if self_closing {
            tag_end
        } else {
            rest.find("</File>").unwrap_or(rest.len())
        };
        let body = &rest[tag_end.min(body_end)..body_end];
        if let Some(file) = xml_attribute(tag, "Path").map(xml_unescape)
            && is_vhdl_path(&file)
        {
            let library = body
                .split("<Attr ")
                .skip(1)
                .find(|a| xml_attribute(a, "Name") == Some("Library"))
                .and_then(|a| xml_attribute(a, "Val"))
                .map(xml_unescape)
                .unwrap_or_else(|| "xil_defaultlib".to_owned());
            let file = file
                .replace("$PSRCDIR", &format!("{stem}.srcs"))
                .replace("$PPRDIR/", "")
                .replace("$PPRDIR", ".");
            add(&mut libraries, &library, literal(&file));
        }
        rest = &rest[body_end.max(1)..];
    }
    libraries
}

/// Splits a Tcl command line into words, keeping `{...}` and `"..."`
/// groups and `[...]` substitutions whole.
fn tcl_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            '{' | '"' | '[' => {
                let close = match c {
                    '{' => '}',
                    '"' => '"',
                    _ => ']',
                };
                let mut depth = 1;
                let mut group = String::new();
                for d in chars.by_ref() {
                    if d == c && c != '"' {
                        depth += 1;
                    } else if d == close {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    group.push(d);
                }
                if c == '[' {
                    word.push('[');
                    word.push_str(&group);
                    word.push(']');
                } else {
                    word.push_str(&group);
                }
            }
            _ => word.push(c),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// The logical lines of a Tcl script: comments dropped, `\` continuations
/// joined.
fn tcl_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if current.is_empty() && trimmed.starts_with('#') {
            continue;
        }
        match trimmed.strip_suffix('\\') {
            Some(start) => {
                current.push_str(start);
                current.push(' ');
            }
            None => {
                current.push_str(trimmed);
                lines.push(std::mem::take(&mut current));
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Tcl evaluation as far as file lists need it: `set` variables, `$var`,
/// `${var}` and `$::quartus(qip_path)`, and the commands that build paths
/// (`info script`, `file dirname|join|normalize|tail`, `pwd`, `glob`).
/// Anything else makes a word unknown, and it is skipped.
struct Tcl {
    /// The script's own directory, `/`-separated.
    dir: String,
    script: String,
    vars: HashMap<String, String>,
}

impl Tcl {
    fn new(script: &Path) -> Tcl {
        let script = script.to_string_lossy().replace('\\', "/");
        let dir = match script.rfind('/') {
            Some(slash) => script[..slash].to_owned(),
            None => ".".to_owned(),
        };
        let mut vars = HashMap::new();
        // Quartus sets this to the directory of the `.qip` being read.
        vars.insert("quartus(qip_path)".to_owned(), dir.clone());
        Tcl { dir, script, vars }
    }

    /// Runs the commands that matter between file lists: `set name value`.
    fn run(&mut self, words: &[String]) {
        if words.len() == 3
            && words[0] == "set"
            && let Some(value) = self.subst(&words[2])
        {
            self.vars
                .insert(words[1].trim_start_matches("::").to_owned(), value);
        }
    }

    /// A word with its variables and commands substituted.
    fn subst(&self, word: &str) -> Option<String> {
        let chars: Vec<char> = word.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '$' => {
                    let at_start = i == 0;
                    i += 1;
                    let mut name = String::new();
                    if chars.get(i) == Some(&'{') {
                        i += 1;
                        while i < chars.len() && chars[i] != '}' {
                            name.push(chars[i]);
                            i += 1;
                        }
                        i += 1;
                    } else {
                        while i < chars.len()
                            && (chars[i].is_ascii_alphanumeric() || matches!(chars[i], '_' | ':'))
                        {
                            name.push(chars[i]);
                            i += 1;
                        }
                        if chars.get(i) == Some(&'(') {
                            while i < chars.len() && chars[i] != ')' {
                                name.push(chars[i]);
                                i += 1;
                            }
                            name.push(')');
                            i += 1;
                        }
                    }
                    let name = name.trim_start_matches("::");
                    match self.var(name) {
                        Some(value) => out.push_str(&value),
                        // An unknown leading variable is most often the
                        // script's directory.
                        None if at_start && chars.get(i) == Some(&'/') => out.push_str(&self.dir),
                        None => return None,
                    }
                }
                '[' => {
                    let mut depth = 1;
                    let mut inner = String::new();
                    i += 1;
                    while i < chars.len() {
                        match chars[i] {
                            '[' => depth += 1,
                            ']' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        inner.push(chars[i]);
                        i += 1;
                    }
                    i += 1;
                    out.push_str(&self.command(&inner)?);
                }
                '\\' => {
                    out.push('/');
                    i += 1;
                }
                c => {
                    out.push(c);
                    i += 1;
                }
            }
        }
        Some(out)
    }

    fn var(&self, name: &str) -> Option<String> {
        if let Some(env) = name.strip_prefix("env(").and_then(|n| n.strip_suffix(')')) {
            return std::env::var(env).ok();
        }
        self.vars.get(name).cloned()
    }

    /// The result of `[inner]`.
    fn command(&self, inner: &str) -> Option<String> {
        let words = tcl_words(inner);
        let arg = |i: usize| words.get(i).and_then(|w| self.subst(w));
        match (words.first()?.as_str(), words.get(1).map(String::as_str)) {
            ("info", Some("script")) => Some(self.script.clone()),
            ("pwd", _) => Some(self.dir.clone()),
            ("file", Some("dirname")) => {
                let path = arg(2)?;
                let path = path.trim_end_matches('/');
                Some(match path.rfind('/') {
                    Some(0) => "/".to_owned(),
                    Some(slash) => path[..slash].to_owned(),
                    None => ".".to_owned(),
                })
            }
            ("file", Some("tail")) => {
                let path = arg(2)?;
                Some(path.rsplit('/').next().unwrap_or_default().to_owned())
            }
            ("file", Some("join")) => {
                let mut joined = String::new();
                for word in &words[2..] {
                    // An unknown variable here is most often the script's
                    // directory.
                    let part = match self.subst(word) {
                        Some(part) => part,
                        None if word.starts_with('$') => self.dir.clone(),
                        None => return None,
                    };
                    if part.starts_with('/') || joined.is_empty() {
                        joined = part;
                    } else {
                        joined = format!("{}/{part}", joined.trim_end_matches('/'));
                    }
                }
                Some(joined)
            }
            ("file", Some("normalize")) => {
                let path = arg(2)?;
                Some(if path.starts_with('/') {
                    path
                } else {
                    format!("{}/{path}", self.dir)
                })
            }
            ("glob", _) => {
                let mut directory = None;
                let mut i = 1;
                while i < words.len() {
                    match words[i].as_str() {
                        "-directory" | "-dir" | "-path" | "-types" | "-type" => {
                            if words[i].starts_with("-dir") {
                                directory = Some(arg(i + 1)?);
                            }
                            i += 2;
                        }
                        "--" => i += 1,
                        w if w.starts_with('-') => i += 1,
                        _ => {
                            let pattern = arg(i)?;
                            return Some(match directory {
                                Some(dir) => format!("{}/{pattern}", dir.trim_end_matches('/')),
                                None => pattern,
                            });
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// A file word as a pattern relative to the script's directory (or
    /// absolute): a whole `[glob ...]` stays a pattern, anything else is a
    /// literal path.
    fn path(&self, word: &str) -> Option<String> {
        let word = word.trim();
        let is_glob = word.starts_with("[glob ") && word.ends_with(']');
        let path = normalize_path(&self.subst(word)?);
        let path = match path.strip_prefix(&format!("{}/", self.dir)) {
            Some(rest) => rest.to_owned(),
            None => path,
        };
        if path.is_empty() || path == "." {
            return None;
        }
        Some(if is_glob { path } else { literal(&path) })
    }
}

/// Resolves `.` and `..` segments where possible.
fn normalize_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." if segments.last().is_some_and(|s| *s != "..") => {
                segments.pop();
            }
            ".." if absolute => {}
            _ => segments.push(segment),
        }
    }
    let joined = segments.join("/");
    if absolute {
        format!("/{joined}")
    } else {
        joined
    }
}

/// `read_vhdl [-library lib] [-vhdl2008] files`; files without a library
/// go to `xil_defaultlib`.
fn vivado_tcl(text: &str, script: &Path) -> Libraries {
    let mut tcl = Tcl::new(script);
    let mut libraries = Libraries::new();
    for line in tcl_lines(text) {
        let words = tcl_words(&line);
        if words.first().map(String::as_str) != Some("read_vhdl") {
            tcl.run(&words);
            continue;
        }
        let mut library = "xil_defaultlib".to_owned();
        let mut files = Vec::new();
        let mut i = 1;
        while i < words.len() {
            match words[i].as_str() {
                "-library" | "-lib" => {
                    if let Some(name) = words.get(i + 1).and_then(|w| tcl.subst(w)) {
                        library = name;
                    }
                    i += 1;
                }
                w if w.starts_with('-') => {}
                w if w.starts_with('[') => files.push(w.to_owned()),
                // `{a.vhd b.vhd}`: a list.
                w => files.extend(w.split_whitespace().map(str::to_owned)),
            }
            i += 1;
        }
        for file in files {
            if let Some(pattern) = tcl.path(&file) {
                add(&mut libraries, &library, pattern);
            }
        }
    }
    libraries
}

// ---- Quartus -----------------------------------------------------------------

/// `set_global_assignment -name VHDL_FILE path [-library lib]`; files
/// without a library are left to inference.
fn quartus(text: &str, script: &Path) -> Libraries {
    let mut tcl = Tcl::new(script);
    let mut libraries = Libraries::new();
    for line in tcl_lines(text) {
        let words = tcl_words(&line);
        if words.first().map(String::as_str) != Some("set_global_assignment") {
            tcl.run(&words);
            continue;
        }
        let mut name = None;
        let mut library = None;
        let mut value = None;
        let mut i = 1;
        while i < words.len() {
            match words[i].to_ascii_lowercase().as_str() {
                "-name" => {
                    name = words.get(i + 1).cloned();
                    i += 1;
                }
                "-library" => {
                    library = words.get(i + 1).and_then(|w| tcl.subst(w));
                    i += 1;
                }
                w if w.starts_with('-') => i += 1,
                _ => value = value.or_else(|| Some(words[i].clone())),
            }
            i += 1;
        }
        if name.is_some_and(|n| n.eq_ignore_ascii_case("VHDL_FILE"))
            && let (Some(library), Some(value)) = (library, value)
            && let Some(pattern) = tcl.path(&value)
        {
            add(&mut libraries, &library, pattern);
        }
    }
    libraries
}

/// Every tool file's libraries, in path order.
pub fn import_all(files: &[PathBuf]) -> Vec<(PathBuf, Libraries)> {
    files
        .iter()
        .filter_map(|f| import(f).map(|libs| (f.clone(), libs)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn libs(pairs: &[(&str, &[&str])]) -> Libraries {
        pairs
            .iter()
            .map(|(l, p)| (l.to_string(), p.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    #[test]
    fn vunit_run_py() {
        let text = r#"
from pathlib import Path
from vunit import VUnit

ROOT = Path(__file__).parent
SRC = ROOT / "src"
vu = VUnit.from_argv()
vu.add_vhdl_builtins()  # not a library of the project
lib = vu.add_library("lib")
lib.add_source_files(SRC / "*.vhd")
lib.add_source_files(ROOT / "test" / "tb_*.vhd")
vu.add_library("common").add_source_files(str(ROOT / "common" / "**" / "*.vhd"))
other = vu.add_library('other')
other.add_source_file(join(root, "other", "pkg.vhd"))
osvvm = vu.add_library("osvvm")
osvvm.add_source_files(ROOT / "my_osvvm_log.vhd")
tb = vu.library("lib")
tb.add_source_files(f"{ROOT}/more/*.vhd")
vu.main()
"#;
        assert_eq!(
            vunit(text),
            libs(&[
                ("common", &["common/**/*.vhd"]),
                ("lib", &["src/*.vhd", "test/tb_*.vhd", "more/*.vhd"]),
                ("other", &["other/pkg.vhd"]),
            ])
        );
    }

    #[test]
    fn fusesoc_core() {
        let text = "CAPI=2:
name: ::uart:1.0
filesets:
  rtl:
    files:
      - rtl/uart_pkg.vhd
      - rtl/uart.vhd
      - rtl/fifo.v: {file_type: verilogSource}
    file_type: vhdlSource-2008
    logical_name: uart_lib
  tb:
    files:
      - tb/tb_uart.vhd: {logical_name: tb_lib}
      - tb/helpers.vhd
    file_type: vhdlSource
targets:
  default:
    filesets: [rtl]
";
        assert_eq!(
            fusesoc(text),
            libs(&[
                ("tb_lib", &["tb/tb_uart.vhd"]),
                ("uart_lib", &["rtl/uart_pkg.vhd", "rtl/uart.vhd"]),
            ])
        );
    }

    #[test]
    fn vivado_xpr() {
        let text = r#"<?xml version="1.0" encoding="UTF-8"?>
<Project Version="7">
  <FileSets>
    <FileSet Name="sources_1" Type="DesignSrcs">
      <File Path="$PSRCDIR/sources_1/new/top.vhd">
        <FileInfo>
          <Attr Name="UsedIn" Val="synthesis"/>
        </FileInfo>
      </File>
      <File Path="$PPRDIR/../rtl/pkg.vhd">
        <FileInfo>
          <Attr Name="Library" Val="common"/>
        </FileInfo>
      </File>
      <File Path="$PPRDIR/../rtl/ip.v"/>
    </FileSet>
  </FileSets>
</Project>
"#;
        assert_eq!(
            vivado_project(text, Path::new("/p/proj/design.xpr")),
            libs(&[
                ("common", &["../rtl/pkg.vhd"]),
                ("xil_defaultlib", &["design.srcs/sources_1/new/top.vhd"]),
            ])
        );
    }

    #[test]
    fn vivado_tcl_script() {
        let text = "# build
set origin_dir [file dirname [info script]]
read_vhdl -library common $origin_dir/rtl/pkg.vhd
read_vhdl -vhdl2008 {rtl/a.vhd rtl/b.vhd}
read_vhdl -library tb \\
    [glob tb/*.vhd]
read_verilog rtl/c.v
";
        assert_eq!(
            vivado_tcl(text, Path::new("/p/build.tcl")),
            libs(&[
                ("common", &["rtl/pkg.vhd"]),
                ("tb", &["tb/*.vhd"]),
                ("xil_defaultlib", &["rtl/a.vhd", "rtl/b.vhd"]),
            ])
        );
    }

    #[test]
    fn quartus_settings() {
        let text = r#"set_global_assignment -name FAMILY "Cyclone V"
set_global_assignment -name VHDL_FILE rtl/top.vhd
set_global_assignment -name VHDL_FILE ../common/pkg.vhd -library common
set_global_assignment -name VHDL_FILE "my dir/x.vhd" -library "lib_x" -hdl_version VHDL_2008
set_global_assignment -library ip -name VHDL_FILE [file join $::quartus(qip_path) "pll.vhd"]
"#;
        assert_eq!(
            quartus(text, Path::new("/p/top.qsf")),
            libs(&[
                ("common", &["../common/pkg.vhd"]),
                ("ip", &["pll.vhd"]),
                ("lib_x", &["my dir/x.vhd"]),
            ])
        );
    }

    #[test]
    fn tcl_variables() {
        let text = r#"set path [file dirname [info script]]
set rtl "$path/rtl"
set common [file join [file dirname $path] common]
set lib my_lib
set_global_assignment -name VHDL_FILE $path/top.vhd -library $lib
set_global_assignment -name VHDL_FILE ${rtl}/core.vhd -library $lib
set_global_assignment -name VHDL_FILE [file join $rtl sub fifo.vhd] -library $lib
set_global_assignment -name VHDL_FILE $common/pkg.vhd -library common
set_global_assignment -name VHDL_FILE [file normalize $path/../ip/pll.vhd] -library ip
set_global_assignment -name VHDL_FILE $missing_var -library lost
"#;
        assert_eq!(
            quartus(text, Path::new("/p/fpga/sub.qip")),
            libs(&[
                ("common", &["/p/common/pkg.vhd"]),
                ("ip", &["/p/ip/pll.vhd"]),
                ("my_lib", &["top.vhd", "rtl/core.vhd", "rtl/sub/fifo.vhd"]),
            ])
        );
        let text = "set src [file dirname [info script]]/src
read_vhdl -library work_lib [glob -directory $src *.vhd]
";
        assert_eq!(
            vivado_tcl(text, Path::new("/p/build.tcl")),
            libs(&[("work_lib", &["src/*.vhd"])])
        );
    }

    #[test]
    fn tool_files_are_recognized() {
        assert!(is_tool_file(Path::new("a/run.py")));
        assert!(is_tool_file(Path::new("a/x.core")));
        assert!(is_tool_file(Path::new("a/x.XPR")));
        assert!(!is_tool_file(Path::new("a/setup.py")));
        assert!(!is_tool_file(Path::new("a/x.vhd")));
    }
}
