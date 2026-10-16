//! Tcl evaluation as far as file lists need it: `set` variables, `$var`,
//! `${var}`, `$::quartus(qip_path)` and `$::env(X)`, and the commands that
//! build paths (`info script`, `file dirname|join|normalize|tail`, `pwd`,
//! `glob`). Anything else makes a word unknown.

use rustc_hash::{FxHashMap, FxHashSet};
use std::path::Path;

use crate::tcl::{self, Word, WordKind};

pub struct Tcl {
    /// The script's own directory, `/`-separated.
    pub dir: String,
    /// Whether an unknown variable makes a word unknown, rather than be
    /// taken for the script's directory as file lists most often mean.
    pub strict: bool,
    script: String,
    vars: FxHashMap<String, String>,
    /// Variables set in branches or loops, whose value is not known.
    unknown: FxHashSet<String>,
}

impl Tcl {
    pub fn new(script: &Path) -> Tcl {
        let script = script.to_string_lossy().replace('\\', "/");
        let dir = match script.rfind('/') {
            Some(slash) => script[..slash].to_owned(),
            None => ".".to_owned(),
        };
        let mut vars = FxHashMap::default();
        // Quartus sets this to the directory of the `.qip` being read.
        vars.insert("quartus(qip_path)".to_owned(), dir.clone());
        Tcl {
            dir,
            strict: false,
            script,
            vars,
            unknown: FxHashSet::default(),
        }
    }

    /// The same, with unknown variables making words unknown.
    pub fn strict(self) -> Self {
        Tcl {
            strict: true,
            ..self
        }
    }

    /// Runs the commands that matter between file lists: `set name value`.
    pub fn run(&mut self, source: &str, words: &[Word]) {
        if words.len() == 3
            && words[0].text(source) == "set"
            && let Some(value) = self.word(source, &words[2])
        {
            let name = words[1].text(source).trim_start_matches("::").to_owned();
            self.unknown.remove(&name);
            self.vars.insert(name, value);
        }
    }

    /// Notes a `set` that may or may not run, in a branch or a loop: its
    /// variable's value is no longer known.
    pub fn forget(&mut self, source: &str, words: &[Word]) {
        if words.len() >= 2 && words[0].text(source) == "set" {
            let name = words[1].text(source).trim_start_matches("::");
            self.unknown.insert(name.to_owned());
        }
    }

    /// The value of a word: a braced word as it is, any other with its
    /// variables and commands substituted.
    pub fn word(&self, source: &str, word: &Word) -> Option<String> {
        match word.kind {
            WordKind::Braced => Some(word.text(source).to_owned()),
            _ => self.subst(word.text(source)),
        }
    }

    /// A word with its variables and commands substituted.
    pub fn subst(&self, word: &str) -> Option<String> {
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
                    if name.is_empty() {
                        out.push('$');
                        continue;
                    }
                    let name = name.trim_start_matches("::");
                    match self.var(name) {
                        Some(value) => out.push_str(&value),
                        // An unknown leading variable is most often the
                        // script's directory.
                        None if !self.strict && at_start && chars.get(i) == Some(&'/') => {
                            out.push_str(&self.dir)
                        }
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
        if self.unknown.contains(name) {
            return None;
        }
        self.vars.get(name).cloned()
    }

    /// The result of `[inner]`.
    fn command(&self, inner: &str) -> Option<String> {
        let script = tcl::parse(inner);
        let words = &script.commands.first()?.words;
        let text = |i: usize| words.get(i).map(|w| w.text(inner));
        let arg = |i: usize| words.get(i).and_then(|w| self.word(inner, w));
        match (text(0)?, text(1)) {
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
                    let part = match self.word(inner, word) {
                        Some(part) => part,
                        None if !self.strict && word.text(inner).starts_with('$') => {
                            self.dir.clone()
                        }
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
                    match text(i)? {
                        "-directory" | "-dir" | "-path" | "-types" | "-type" => {
                            if text(i)?.starts_with("-dir") {
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

    /// A file word as a path relative to the script's directory (or
    /// absolute), `.` and `..` resolved, and whether it is a `[glob ...]`
    /// pattern.
    pub fn path(&self, source: &str, word: &Word) -> Option<(String, bool)> {
        let text = word.text(source).trim();
        let is_glob =
            word.kind != WordKind::Braced && text.starts_with("[glob ") && text.ends_with(']');
        let path = normalize_path(&self.word(source, word)?.replace('\\', "/"));
        let path = match path.strip_prefix(&format!("{}/", self.dir)) {
            Some(rest) => rest.to_owned(),
            None => path,
        };
        if path.is_empty() || path == "." {
            return None;
        }
        Some((path, is_glob))
    }
}

/// Resolves `.` and `..` segments where possible.
pub fn normalize_path(path: &str) -> String {
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
