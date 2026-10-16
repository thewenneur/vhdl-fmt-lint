//! The checks of the `vendor/*` rules.

use std::path::{Path, PathBuf};

use rustc_hash::{FxHashMap, FxHashSet};
use vfl_syntax::{LineIndex, TextRange};

use crate::eval::Tcl;
use crate::known::{self, Spec};
use crate::rules::*;
use crate::suggest::near;
use crate::tcl::{self, Command, Script, Word, WordKind};
use crate::{Context, Finding, Kind};

mod statements;

/// The findings of every `vendor/*` rule in `text`, the file at `path`.
pub fn check(kind: Kind, path: &Path, text: &str, cx: &Context) -> Vec<Finding> {
    let mut checker = Checker {
        kind,
        path,
        text,
        cx,
        index: LineIndex::new(text),
        findings: Vec::new(),
        procs: FxHashSet::default(),
        assigned: FxHashMap::default(),
        pins: FxHashMap::default(),
        clocks: FxHashMap::default(),
        top: None,
        tcl: Tcl::new(path).strict(),
        reported: FxHashSet::default(),
    };
    if kind.is_statements() {
        checker.statements();
    } else {
        let script = tcl::parse(text);
        checker.procs(&script);
        checker.top = checker.top_entity(&script);
        checker.script(&script, true);
    }
    checker
        .findings
        .sort_by_key(|f| (f.range.start(), f.range.end(), f.rule));
    checker.findings.dedup();
    checker.findings
}

struct Checker<'a> {
    kind: Kind,
    path: &'a Path,
    text: &'a str,
    cx: &'a Context<'a>,
    index: LineIndex,
    findings: Vec<Finding>,
    /// Commands the file defines with `proc`.
    procs: FxHashSet<String>,
    /// The assignments made at the top level, by what they set: where and
    /// with what value.
    assigned: FxHashMap<String, (TextRange, String)>,
    /// The pins placed at the top level: where, and the port.
    pins: FxHashMap<String, (TextRange, String)>,
    /// The clocks created, by name.
    clocks: FxHashMap<String, TextRange>,
    /// Quartus `TOP_LEVEL_ENTITY`.
    top: Option<String>,
    tcl: Tcl,
    /// Syntax errors already reported, by range.
    reported: FxHashSet<TextRange>,
}

/// Commands whose braced words are scripts, and which of them.
fn bodies(name: &str, words: &[Word], text: &str) -> Vec<usize> {
    let n = words.len();
    match name {
        "if" => {
            // if cond body ?elseif cond body ...? ?else body?
            let mut out = Vec::new();
            let mut i = 2;
            while i < n {
                let w = words[i].text(text);
                if w == "then" {
                    i += 1;
                    continue;
                }
                out.push(i);
                i += 1;
                match words.get(i).map(|w| w.text(text)) {
                    Some("elseif") => i += 2,
                    Some("else") => i += 1,
                    _ => break,
                }
            }
            out
        }
        "foreach" | "lmap" if n >= 4 => vec![n - 1],
        "while" if n == 3 => vec![2],
        "for" if n == 5 => vec![1, 3, 4],
        "proc" if n == 4 => vec![3],
        "catch" if n >= 2 => vec![1],
        "namespace" if n >= 4 && words[1].text(text) == "eval" => vec![n - 1],
        "foreach_in_collection" if n == 4 => vec![3],
        _ => Vec::new(),
    }
}

impl<'a> Checker<'a> {
    fn push(&mut self, rule: &'static str, range: TextRange, message: impl Into<String>) {
        self.findings.push(Finding {
            rule,
            range,
            message: message.into(),
        });
    }

    fn line(&self, range: TextRange) -> u32 {
        self.index.line(range.start()) + 1
    }

    /// The commands the file defines, anywhere in it.
    fn procs(&mut self, script: &Script) {
        for command in &script.commands {
            let name = command.name(self.text);
            if name == "proc"
                && let Some(word) = command.words.get(1)
            {
                self.procs
                    .insert(word.text(self.text).trim_start_matches("::").to_owned());
            }
            for i in bodies(name, &command.words, self.text) {
                let word = &command.words[i];
                if word.kind == WordKind::Braced {
                    let inner = tcl::parse_range(self.text, word.inner);
                    self.procs(&inner);
                }
            }
        }
    }

    fn top_entity(&self, script: &Script) -> Option<String> {
        script.commands.iter().find_map(|command| {
            if command.name(self.text) != "set_global_assignment" {
                return None;
            }
            let args = self.args(command, None);
            let name = args.option("-name")?;
            name.text(self.text)
                .eq_ignore_ascii_case("TOP_LEVEL_ENTITY")
                .then(|| {
                    args.positional
                        .first()
                        .map(|w| w.text(self.text).to_owned())
                })?
        })
    }

    fn script(&mut self, script: &Script, top: bool) {
        for error in &script.errors {
            if self.reported.insert(error.range) {
                self.push(SYNTAX, error.range, error.message.clone());
            }
        }
        for command in &script.commands {
            self.command(command, top);
        }
    }

    fn spec(&self, name: &str) -> Option<&'static Spec> {
        // Other tools of a flow (SpyGlass, ...) have their own.
        if self.kind == Kind::Tcl && name == "set_parameter" {
            return None;
        }
        let lists: &[&[Spec]] = match self.kind {
            Kind::Qsf | Kind::Qip => &[known::QUARTUS],
            Kind::Sdc => &[known::SDC, known::XDC],
            Kind::Xdc => &[known::SDC, known::XDC],
            Kind::Tcl => &[known::QUARTUS, known::SDC, known::XDC],
            Kind::Pdc => &[known::PDC, known::SDC],
            Kind::Lpf | Kind::Cst => &[],
        };
        lists.iter().flat_map(|l| l.iter()).find(|s| s.name == name)
    }

    fn known_command(&self, name: &str) -> bool {
        let name = name.trim_start_matches("::");
        let base = name.rsplit("::").next().unwrap_or(name);
        known::TCL.contains(&base)
            || self.spec(base).is_some()
            || self.procs.contains(name)
            || self.procs.contains(base)
            || name.contains("::")
    }

    fn command_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = known::TCL.to_vec();
        let lists: &[&[Spec]] = match self.kind {
            Kind::Qsf | Kind::Qip => &[known::QUARTUS],
            Kind::Pdc => &[known::PDC, known::SDC],
            _ => &[known::SDC, known::XDC],
        };
        names.extend(lists.iter().flat_map(|l| l.iter()).map(|s| s.name));
        names
    }

    fn command(&mut self, command: &Command, top: bool) {
        let text = self.text;
        let name_word = &command.words[0];
        let name = name_word.text(text);
        // Scripts in braces and brackets are checked too.
        for i in bodies(name, &command.words, text) {
            let word = &command.words[i];
            if word.kind == WordKind::Braced {
                let inner = tcl::parse_range(text, word.inner);
                self.script(&inner, false);
            }
        }
        for word in &command.words {
            self.nested(word);
        }
        if name == "set" {
            if top {
                self.tcl.run(text, &command.words);
            } else {
                self.tcl.forget(text, &command.words);
            }
        }
        if !name_word.is_literal(text) || name.is_empty() {
            return;
        }
        if !self.known_command(name) {
            // A Tcl script may call anything its tools define.
            if self.kind != Kind::Tcl || self.cx.strict_commands {
                match near(name, self.command_names()) {
                    Some(meant) => self.push(
                        UNKNOWN_COMMAND,
                        name_word.range,
                        format!("unknown command `{name}`; did you mean `{meant}`?"),
                    ),
                    None if self.cx.strict_commands => self.push(
                        UNKNOWN_COMMAND,
                        name_word.range,
                        format!("unknown command `{name}` in a {} file", self.kind.name()),
                    ),
                    None => {}
                }
            }
            if self.kind != Kind::Tcl {
                return;
            }
        }
        let spec = self.spec(name);
        let args = self.args(command, spec);
        if let Some(spec) = spec {
            self.options(command, spec, &args);
        }
        match name {
            "set_global_assignment"
            | "set_instance_assignment"
            | "set_parameter"
            | "set_io_assignment"
            | "set_power_file_assignment"
                if spec.is_some() =>
            {
                self.assignment(command, &args, top)
            }
            "set_location_assignment" => self.location(&args, top),
            "set_false_path" => {
                let restricted = args.options.iter().any(|(o, _, _)| {
                    o.ends_with("from") || o.ends_with("to") || o.ends_with("through")
                });
                if !restricted {
                    self.push(
                        MISSING_ARGUMENT,
                        name_word.range,
                        "`set_false_path` without `-from`, `-to` or `-through` cuts every path",
                    );
                }
            }
            "set_property" => self.property(&args, top),
            "set_io" if self.kind == Kind::Pdc => self.set_io(command, top),
            "ldc_set_location" => self.ldc_location(command, top),
            "create_clock" if top => {
                if let Some(clock) = args.option("-name").or_else(|| args.positional.first()) {
                    let key = clock.text(text).to_owned();
                    if args.flag("-add") {
                        return;
                    }
                    if let Some(first) = self.clocks.get(&key) {
                        let line = self.line(*first);
                        self.push(
                            DUPLICATE_ASSIGNMENT,
                            clock.range,
                            format!("clock `{key}` is already created on line {line}"),
                        );
                    } else {
                        self.clocks.insert(key, clock.range);
                    }
                }
            }
            "source" => {
                if let Some(word) = args.positional.last() {
                    self.file(word);
                }
            }
            _ if self.kind == Kind::Tcl && known::VIVADO_READ.contains(&name) => {
                for word in args.positional.clone() {
                    if word.kind == WordKind::Braced {
                        for item in tcl::list_items(text, word.inner) {
                            let item = Word {
                                kind: WordKind::Braced,
                                range: item,
                                inner: item,
                            };
                            self.file(&item);
                        }
                    } else {
                        self.file(&word);
                    }
                }
            }
            _ => {}
        }
    }

    /// The command of a `[...]` word, checked as a script.
    fn nested(&mut self, word: &Word) {
        if word.kind == WordKind::Braced {
            return;
        }
        let text = self.text;
        let inner = word.text(text);
        if !inner.contains('[') {
            return;
        }
        // Each top-level `[...]` of the word.
        let bytes = inner.as_bytes();
        let base = usize::from(word.inner.start());
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => i += 2,
                b'[' => {
                    let start = i + 1;
                    let mut depth = 1;
                    let mut j = start;
                    while j < bytes.len() {
                        match bytes[j] {
                            b'\\' => j += 1,
                            b'[' => depth += 1,
                            b']' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        j += 1;
                    }
                    let end = j.min(bytes.len());
                    let range = TextRange::new(
                        ((base + start) as u32).into(),
                        ((base + end) as u32).into(),
                    );
                    let script = tcl::parse_range(text, range);
                    self.script(&script, false);
                    for command in &script.commands {
                        self.ports_of(command);
                    }
                    i = end + 1;
                }
                _ => i += 1,
            }
        }
    }

    fn args(&self, command: &Command, spec: Option<&Spec>) -> Args {
        let text = self.text;
        let mut args = Args::default();
        let words = &command.words[1..];
        let mut i = 0;
        while i < words.len() {
            let word = &words[i];
            if word.is_option(text) {
                let option = word.text(text);
                let valued = match spec {
                    Some(spec) => spec.valued.contains(&option),
                    // Quartus options all take a value but these.
                    None => !matches!(option, "-disable" | "-remove" | "-rise" | "-fall"),
                };
                if valued {
                    args.options
                        .push((option.to_owned(), word.clone(), words.get(i + 1).cloned()));
                    i += 2;
                    continue;
                }
                args.options.push((option.to_owned(), word.clone(), None));
            } else {
                args.positional.push(word.clone());
            }
            i += 1;
        }
        args
    }

    fn options(&mut self, command: &Command, spec: &Spec, args: &Args) {
        let name = spec.name;
        for (o, option, value) in &args.options {
            let o = o.as_str();
            let known = spec.valued.contains(&o)
                || spec.flags.contains(&o)
                || known::COMMON_FLAGS.contains(&o);
            if !known && spec.closed {
                let all = spec.valued.iter().chain(spec.flags).copied();
                let message = match near(o, all) {
                    Some(meant) => {
                        format!("`{name}` has no option `{o}`; did you mean `{meant}`?")
                    }
                    None => format!("`{name}` has no option `{o}`"),
                };
                self.push(UNKNOWN_OPTION, option.range, message);
            }
            if spec.valued.contains(&o) && value.is_none() {
                self.push(
                    MISSING_ARGUMENT,
                    option.range,
                    format!("`{o}` of `{name}` needs a value"),
                );
            }
        }
        for required in spec.required {
            if args.option(required).is_none() && !args.flag(required) {
                self.push(
                    MISSING_ARGUMENT,
                    command.words[0].range,
                    format!("`{name}` needs `{required}`"),
                );
            }
        }
        let removing = args.flag("-remove");
        let needed = match name {
            "set_global_assignment"
            | "set_instance_assignment"
            | "set_parameter"
            | "set_io_assignment"
                if !removing =>
            {
                1
            }
            "set_location_assignment" if removing => 0,
            "set_property" => {
                let given = [("-dict", 2), ("-name", 1), ("-value", 1), ("-objects", 1)]
                    .iter()
                    .filter(|(o, _)| args.option(o).is_some())
                    .map(|(_, n)| n)
                    .sum::<usize>();
                3usize.saturating_sub(given)
            }
            _ => spec.positional,
        };
        if args.positional.len() < needed {
            let what = match name {
                "set_location_assignment" => "a location".to_owned(),
                "set_property" if needed == 3 => "a property, a value and objects".to_owned(),
                "set_property" => "objects".to_owned(),
                "set_input_delay" | "set_output_delay" => "a delay and ports".to_owned(),
                "create_generated_clock" => "the clock's source objects".to_owned(),
                _ if needed == 1 && name.ends_with("_assignment") || name == "set_parameter" => {
                    "a value".to_owned()
                }
                _ if needed == 1 => "a value".to_owned(),
                _ => format!("{needed} arguments"),
            };
            self.push(
                MISSING_ARGUMENT,
                command.words[0].range,
                format!("`{name}` needs {what}"),
            );
        }
    }

    /// `set_global_assignment -name NAME value`, and the like.
    fn assignment(&mut self, command: &Command, args: &Args, top: bool) {
        let text = self.text;
        let Some(name_word) = args.option("-name") else {
            return;
        };
        let name = name_word.text(text).to_ascii_uppercase();
        if name_word.is_literal(text)
            && !known::QUARTUS_NAMES.contains(&name.as_str())
            && let Some(meant) = near(&name, known::QUARTUS_NAMES.iter().copied())
        {
            self.push(
                MISSPELLED_NAME,
                name_word.range,
                format!("unknown assignment `{name}`; did you mean `{meant}`?"),
            );
        }
        let value = args.positional.first();
        let command_name = command.name(text);
        if command_name == "set_global_assignment"
            && known::QUARTUS_FILES.contains(&name.as_str())
            && let Some(value) = value
        {
            self.file(value);
        }
        if command_name == "set_instance_assignment"
            && is_io_assignment(&name)
            && let Some(to) = args.option("-to")
        {
            self.port(to);
        }
        if !top || args.flag("-remove") {
            return;
        }
        let multi = name.ends_with("_FILE")
            || name.ends_with("_FILES")
            || name.ends_with("_PARAMETER")
            || name.ends_with("_PARAMETER_ASSIGNMENT")
            || known::QUARTUS_MULTI.contains(&name.as_str());
        let mut key = format!("{command_name} {name}");
        for option in ["-to", "-from", "-section_id", "-entity", "-library", "-tag"] {
            if let Some(word) = args.option(option) {
                key.push_str(&format!(" {option}={}", word.text(text)));
            }
        }
        let value_text = value.map_or(String::new(), |v| v.text(text).to_owned());
        if multi {
            key.push_str(&format!(" ={value_text}"));
        }
        let at = value.map_or(name_word.range, |v| v.range);
        self.assign(key, at, &name, value_text);
    }

    fn assign(&mut self, key: String, at: TextRange, what: &str, value: String) {
        // Scripts set and reset properties as they go.
        if self.kind == Kind::Tcl || is_dynamic(&key) {
            return;
        }
        match self.assigned.get(&key) {
            Some((first, previous)) => {
                let line = self.line(*first);
                let message = if *previous == value {
                    format!("`{what}` is already set to `{value}` on line {line}")
                } else {
                    format!(
                        "`{what}` is already set on line {line}: `{value}` overrides `{previous}`"
                    )
                };
                self.push(DUPLICATE_ASSIGNMENT, at, message);
            }
            None => {
                self.assigned.insert(key, (at, value));
            }
        }
    }

    /// `set_location_assignment PIN_A1 -to port`.
    fn location(&mut self, args: &Args, top: bool) {
        let text = self.text;
        let (Some(location), Some(to)) = (args.positional.first(), args.option("-to")) else {
            return;
        };
        let location_text = location.text(text).to_ascii_uppercase();
        let is_pin = location_text.starts_with("PIN_");
        if is_pin {
            self.port(to);
        }
        if !top || args.flag("-remove") {
            return;
        }
        let port = to.text(text).to_owned();
        self.assign(
            format!("set_location_assignment -to={port}"),
            location.range,
            &format!("the location of {port}"),
            location_text.clone(),
        );
        if is_pin {
            self.pin(location_text, location.range, port);
        }
    }

    fn pin(&mut self, pin: String, at: TextRange, port: String) {
        match self.pins.get(&pin) {
            Some((first, other))
                if !other.eq_ignore_ascii_case(&port)
                    && self.in_design(other)
                    && self.in_design(&port) =>
            {
                let line = self.line(*first);
                self.push(
                    PIN_CONFLICT,
                    at,
                    format!("pin {pin} is already assigned to `{other}` on line {line}"),
                );
            }
            Some(_) => {}
            None => {
                self.pins.insert(pin, (at, port));
            }
        }
    }

    /// Libero `set_io port -pinname A1 ...`, or `set_io -port_name port
    /// -pin_name A1 ...`.
    fn set_io(&mut self, command: &Command, top: bool) {
        let text = self.text;
        let args = self.args(command, None);
        let option = |names: &[&str]| {
            args.options
                .iter()
                .rev()
                .find(|(o, _, _)| names.iter().any(|n| o.eq_ignore_ascii_case(n)))
                .and_then(|(_, _, value)| value.clone())
        };
        let Some(port) = option(&["-port_name"]).or_else(|| args.positional.first().cloned())
        else {
            return;
        };
        self.port(&port);
        let (Some(pin), true) = (option(&["-pinname", "-pin_name"]), top) else {
            return;
        };
        if !pin.is_literal(text) || !port.is_literal(text) {
            return;
        }
        let port = port.text(text).to_owned();
        let pin_text = pin.text(text).to_ascii_uppercase();
        self.assign(
            format!("set_io -pinname {port}"),
            pin.range,
            &format!("the pin of {port}"),
            pin_text.clone(),
        );
        self.pin(pin_text, pin.range, port);
    }

    /// Radiant `ldc_set_location -site {A1} [get_ports port]`.
    fn ldc_location(&mut self, command: &Command, top: bool) {
        let text = self.text;
        let args = self.args(command, None);
        let (Some(site), Some(objects), true) =
            (args.option("-site"), args.positional.first(), top)
        else {
            return;
        };
        let Some(port) = self.single_port(objects) else {
            return;
        };
        let site_text = site.text(text).trim().to_ascii_uppercase();
        if !site.is_literal(text) || !is_pin(&site_text) {
            return;
        }
        self.assign(
            format!("ldc_set_location -site {port}"),
            site.range,
            &format!("the site of {port}"),
            site_text.clone(),
        );
        self.pin(site_text, site.range, port);
    }

    /// `set_property NAME value objects`, or `set_property -dict {...}
    /// objects`.
    fn property(&mut self, args: &Args, top: bool) {
        let text = self.text;
        let mut pairs: Vec<(Word, Word)> = Vec::new();
        let objects;
        if let Some(dict) = args.option("-dict") {
            objects = args.positional.first();
            let items = if dict.kind == WordKind::Braced || dict.kind == WordKind::Quoted {
                tcl::list_items(text, dict.inner)
            } else {
                Vec::new()
            };
            for pair in items.chunks(2) {
                if let [name, value] = pair {
                    let word = |range| Word {
                        kind: WordKind::Braced,
                        range,
                        inner: range,
                    };
                    pairs.push((word(*name), word(*value)));
                }
            }
        } else {
            objects = args.positional.get(2);
            if let (Some(name), Some(value)) = (args.positional.first(), args.positional.get(1)) {
                pairs.push((name.clone(), value.clone()));
            }
        }
        let port = objects.and_then(|o| self.single_port(o));
        for (name, value) in &pairs {
            let property = name.text(text).to_ascii_uppercase();
            if name.is_literal(text)
                && !known::VIVADO_PROPERTIES.contains(&property.as_str())
                && let Some(meant) = near(&property, known::VIVADO_PROPERTIES.iter().copied())
            {
                self.push(
                    MISSPELLED_NAME,
                    name.range,
                    format!("unknown property `{property}`; did you mean `{meant}`?"),
                );
            }
            let Some(objects) = objects else { continue };
            if !top {
                continue;
            }
            let target = match &port {
                Some(port) => format!("port {port}"),
                None => objects
                    .text(text)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
            };
            let value_text = value.text(text).to_owned();
            self.assign(
                format!("set_property {property} {target}"),
                value.range,
                &format!("{property} of {target}"),
                value_text.clone(),
            );
            if let Some(port) = &port
                && property == "PACKAGE_PIN"
            {
                self.pin(value_text.to_ascii_uppercase(), value.range, port.clone());
            }
        }
    }

    /// The port of `[get_ports name]`, when it names exactly one.
    fn single_port(&self, objects: &Word) -> Option<String> {
        let text = self.text;
        let inner = objects.command(text)?;
        let script = tcl::parse_range(text, inner);
        let command = script.commands.first()?;
        if command.name(text) != "get_ports" || command.words.len() != 2 {
            return None;
        }
        let word = &command.words[1];
        let items = tcl::list_items(text, word.inner);
        match items.as_slice() {
            [one] if !text[*one].contains(['*', '?', '$', '[']) || is_index(&text[*one]) => {
                Some(text[*one].to_owned())
            }
            _ => None,
        }
    }

    /// Checks the names `get_ports` takes against the design's ports.
    fn ports_of(&mut self, command: &Command) {
        let text = self.text;
        if command.name(text) != "get_ports" {
            return;
        }
        let words = &command.words[1..];
        if words.iter().any(|w| {
            matches!(
                w.text(text),
                "-regexp" | "-filter" | "-of_objects" | "-of" | "-nocase"
            )
        }) {
            return;
        }
        for word in words {
            if word.is_option(text) || !word.is_literal(text) && word.kind != WordKind::Braced {
                continue;
            }
            for item in tcl::list_items(text, word.inner) {
                self.port_item(item);
            }
        }
    }

    fn port(&mut self, word: &Word) {
        if word.is_literal(self.text) {
            self.port_item(word.inner);
        }
    }

    /// Whether `port` (or the port of bit `port`) is a port of the design,
    /// or may be when the design's ports are not known. Board files place
    /// one pin under several names, of which a design uses one: the tools
    /// ignore the others.
    fn in_design(&self, port: &str) -> bool {
        let Some(ports) = self.cx.ports.filter(|p| !p.is_empty()) else {
            return true;
        };
        let base = port
            .split(['[', '('])
            .next()
            .unwrap_or(port)
            .trim_start_matches('\\')
            .trim();
        ports.has(self.top.as_deref(), base)
    }

    /// Reports `range` when it names no port of the design.
    fn port_item(&mut self, range: TextRange) {
        let Some(ports) = self.cx.ports.filter(|p| !p.is_empty()) else {
            return;
        };
        let name = &self.text[range];
        if name.contains(['*', '?', '$', '[']) && !is_index(name) || name.contains('|') {
            return;
        }
        // `data[3]` and `data(3)` are bits of `data`.
        let base = name
            .split(['[', '('])
            .next()
            .unwrap_or(name)
            .trim_start_matches('\\')
            .trim();
        if base.is_empty() || ports.has(self.top.as_deref(), base) {
            return;
        }
        let message = match &self.top {
            Some(top) if ports.knows(top) => format!("`{top}` has no port `{base}`"),
            _ => format!("no entity or module of the design has a port `{base}`"),
        };
        self.push(UNKNOWN_PORT, range, message);
    }

    /// Reports a file word whose file does not exist.
    fn file(&mut self, word: &Word) {
        let text = self.text;
        if word.text(text).contains(['*', '?']) {
            return;
        }
        // A script's literal relative paths depend on where it is run
        // from, often a build directory with generated files: only the
        // paths it builds from its own location are checked.
        if self.kind == Kind::Tcl
            && word.is_literal(text)
            && !Path::new(word.text(text)).is_absolute()
        {
            return;
        }
        let Some((path, is_glob)) = self.tcl.path(text, word) else {
            return;
        };
        if is_glob || path.contains(['*', '?']) || path.contains("://") {
            return;
        }
        // The same for a relative path a script sets in a variable.
        if self.kind == Kind::Tcl {
            let value = self
                .tcl
                .word(text, word)
                .unwrap_or_default()
                .replace('\\', "/");
            let value = crate::eval::normalize_path(&value);
            if !(Path::new(&value).is_absolute()
                || looks_absolute(&value)
                || value.starts_with(&format!("{}/", self.tcl.dir)))
            {
                return;
            }
        }
        if self.exists(&path) {
            return;
        }
        self.push(
            MISSING_FILE,
            word.range,
            format!("`{}` does not exist", word.text(text)),
        );
    }

    /// Whether a path of the file exists: absolute, or relative to the
    /// file's directory, the project root, or for a `.qip`, the directory
    /// of a Quartus project above it.
    fn exists(&self, path: &str) -> bool {
        let candidate = Path::new(path);
        if candidate.is_absolute() || looks_absolute(path) {
            return candidate.exists();
        }
        let dir = self.path.parent().unwrap_or(Path::new("."));
        let mut bases: Vec<PathBuf> = vec![dir.to_owned()];
        if let Some(root) = self.cx.root {
            bases.push(root.to_owned());
        }
        if matches!(self.kind, Kind::Qip | Kind::Tcl | Kind::Sdc) {
            bases.extend(dir.ancestors().skip(1).take(6).map(Path::to_owned));
        }
        // Build flows often run the tool in a directory below the file:
        // `../x` may be meant from there.
        let mut relative = path;
        let mut tries: Vec<&str> = vec![relative];
        while let Some(rest) = relative.strip_prefix("../") {
            relative = rest;
            tries.push(relative);
        }
        bases
            .iter()
            .any(|base| tries.iter().any(|path| base.join(path).exists()))
    }
}

/// `data[3]`: a bit of a port, which is not a pattern.
/// Whether `key` holds a variable or a command, whose value is not known.
fn is_dynamic(key: &str) -> bool {
    key.contains('$')
        || key
            .match_indices('[')
            .any(|(i, _)| !key[i + 1..].starts_with(|c: char| c.is_ascii_digit()))
}

/// Whether `site` names a package pin (`A1`, `AB12`, `23`), not a slice
/// or a region.
fn is_pin(site: &str) -> bool {
    let digits = site.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    site.len() - digits.len() <= 2
        && !digits.is_empty()
        && digits.len() <= 3
        && digits.chars().all(|c| c.is_ascii_digit())
}

fn is_index(name: &str) -> bool {
    name.ends_with(']')
        && name.split_once('[').is_some_and(|(_, i)| {
            i[..i.len() - 1]
                .chars()
                .all(|c| c.is_ascii_digit() || c == ':')
        })
}

fn looks_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() > 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

/// Quartus instance assignments made to pins.
fn is_io_assignment(name: &str) -> bool {
    matches!(
        name,
        "IO_STANDARD"
            | "CURRENT_STRENGTH_NEW"
            | "SLEW_RATE"
            | "WEAK_PULL_UP_RESISTOR"
            | "OUTPUT_TERMINATION"
            | "INPUT_TERMINATION"
            | "VIRTUAL_PIN"
            | "FAST_INPUT_REGISTER"
            | "FAST_OUTPUT_REGISTER"
            | "FAST_OUTPUT_ENABLE_REGISTER"
            | "IO_MAXIMUM_TOGGLE_RATE"
            | "XCVR_IO_PIN_TERMINATION"
    )
}

#[derive(Default)]
struct Args {
    /// Options, as written, with their word and value.
    options: Vec<(String, Word, Option<Word>)>,
    positional: Vec<Word>,
}

impl Args {
    /// The value of option `name`; the last one wins.
    fn option(&self, name: &str) -> Option<&Word> {
        self.options
            .iter()
            .rev()
            .find(|(o, _, v)| o == name && v.is_some())
            .and_then(|(_, _, v)| v.as_ref())
    }

    fn flag(&self, name: &str) -> bool {
        self.options.iter().any(|(o, _, _)| o == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ports;

    fn run(kind: Kind, text: &str, cx: &Context) -> Vec<(&'static str, String, String)> {
        let path = Path::new("/nowhere/project/top.qsf");
        check(kind, path, text, cx)
            .into_iter()
            .map(|f| (f.rule, text[f.range].to_owned(), f.message))
            .collect()
    }

    fn rules(kind: Kind, text: &str) -> Vec<(&'static str, String)> {
        run(kind, text, &Context::default())
            .into_iter()
            .map(|(r, t, _)| (r, t))
            .collect()
    }

    #[test]
    fn quartus_typos() {
        let text = "set_global_assignmnt -name FAMILY \"Cyclone V\"\n\
                    set_global_assignment -name VHLD_INPUT_VERSION VHDL_2008\n\
                    set_global_assignment -nmae DEVICE 5CSEMA5\n\
                    set_global_assignment -name MY_OWN_THING 1\n";
        assert_eq!(
            rules(Kind::Qsf, text),
            [
                (UNKNOWN_COMMAND, "set_global_assignmnt".into()),
                (MISSPELLED_NAME, "VHLD_INPUT_VERSION".into()),
                (MISSING_ARGUMENT, "set_global_assignment".into()),
                (UNKNOWN_OPTION, "-nmae".into()),
            ]
        );
    }

    #[test]
    fn missing_arguments() {
        let text = "set_global_assignment -name TOP_LEVEL_ENTITY\n\
                    set_location_assignment -to clk\n\
                    set_instance_assignment -name IO_STANDARD \"3.3-V LVTTL\" -to\n";
        let found = rules(Kind::Qsf, text);
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(found.iter().all(|(r, _)| *r == MISSING_ARGUMENT));
        let sdc = "create_clock -name clk [get_ports clk]\nset_false_path\n\
                   set_input_delay -clock clk 2\n\
                   set_clock_groups -asynchronous\n";
        assert_eq!(
            rules(Kind::Sdc, sdc),
            [
                (MISSING_ARGUMENT, "create_clock".into()),
                (MISSING_ARGUMENT, "set_false_path".into()),
                (MISSING_ARGUMENT, "set_input_delay".into()),
                (MISSING_ARGUMENT, "set_clock_groups".into()),
            ]
        );
    }

    #[test]
    fn duplicates_and_pins() {
        let text = "set_global_assignment -name FAMILY \"Cyclone V\"\n\
                    set_global_assignment -name FAMILY \"Cyclone IV E\"\n\
                    set_global_assignment -name VHDL_FILE a.vhd\n\
                    set_global_assignment -name VHDL_FILE b.vhd\n\
                    set_location_assignment PIN_A1 -to clk\n\
                    set_location_assignment PIN_A1 -to rst\n\
                    set_location_assignment PIN_B2 -to clk\n";
        let found = run(Kind::Qsf, text, &Context::default());
        let kept: Vec<_> = found
            .iter()
            .filter(|(r, _, _)| *r != MISSING_FILE)
            .map(|(r, t, m)| (*r, t.as_str(), m.as_str()))
            .collect();
        assert_eq!(
            kept,
            [
                (
                    DUPLICATE_ASSIGNMENT,
                    "\"Cyclone IV E\"",
                    "`FAMILY` is already set on line 1: `Cyclone IV E` overrides `Cyclone V`"
                ),
                (
                    PIN_CONFLICT,
                    "PIN_A1",
                    "pin PIN_A1 is already assigned to `clk` on line 5"
                ),
                (
                    DUPLICATE_ASSIGNMENT,
                    "PIN_B2",
                    "`the location of clk` is already set on line 5: `PIN_B2` overrides `PIN_A1`"
                ),
            ]
        );
        let xdc = "set_property PACKAGE_PIN W5 [get_ports clk]\n\
                   set_property -dict {PACKAGE_PIN W5 IOSTANDARD LVCMOS33} [get_ports {rst}]\n\
                   set_property IOSTANDRD LVCMOS33 [get_ports clk]\n\
                   create_clock -period 10 -name sys [get_ports clk]\n\
                   create_clock -period 5 -name sys [get_ports clk]\n";
        assert_eq!(
            rules(Kind::Xdc, xdc),
            [
                (PIN_CONFLICT, "W5".into()),
                (MISSPELLED_NAME, "IOSTANDRD".into()),
                (DUPLICATE_ASSIGNMENT, "sys".into()),
            ]
        );
    }

    #[test]
    fn branches_are_not_duplicates() {
        let text = "if {$a} {\n  set_location_assignment PIN_A1 -to clk\n} else {\n  \
                    set_location_assignment PIN_A1 -to rst\n  set_global_assignmen -name X 1\n}\n";
        assert_eq!(
            rules(Kind::Qsf, text),
            [(UNKNOWN_COMMAND, "set_global_assignmen".into())]
        );
    }

    #[test]
    fn nested_commands_are_checked() {
        let xdc = "set_property IOSTANDARD LVCMOS33 [get_prots {a b}]\n";
        assert_eq!(
            rules(Kind::Xdc, xdc),
            [(UNKNOWN_COMMAND, "get_prots".into())]
        );
    }

    fn rules_of(
        kind: Kind,
        name: &str,
        text: &str,
        ports: Option<&Ports>,
    ) -> Vec<(&'static str, String)> {
        let cx = Context {
            ports,
            ..Context::default()
        };
        check(kind, Path::new(name), text, &cx)
            .into_iter()
            .map(|f| (f.rule, text[f.range].to_owned()))
            .collect()
    }

    fn design_ports() -> Ports {
        let mut ports = Ports::default();
        ports.add("top", ["clk", "led"]);
        ports
    }

    #[test]
    fn lattice_preferences() {
        let text = "LOCATE COMP \"clk\" SITE \"A1\";\n\
                    LOCATE COMP \"led[0]\" SITE \"A1\";\n\
                    LOCATE COMP \"btn\" SITE \"B2\";\n\
                    LOCATE COMP \"u_core\" SITE \"R2C3D\";\n\
                    IOBUF PORT \"clk\" IO_TYPE=LVCMOS33 PULMODE=UP;\n\
                    IOBUF PORT \"clk\" IO_TYPE=LVCMOS25;\n\
                    FREQUENCY PORT \"clk\" 12 MHZ;\n\
                    FREQUENCY 50 MHZ;\n\
                    LOCATE COMP \"led[1]\";\n\
                    IOBUFF ALLPORTS IO_TYPE=LVCMOS33;\n\
                    LOCATE COMP \"led[2]\" SITE \"C3\"\n\
                    BLOCK RESETPATHS;\n\
                    LOCATE COMP \"spare\" SITE \"C3\";\n";
        let ports = design_ports();
        assert_eq!(
            rules_of(Kind::Lpf, "pins.lpf", text, Some(&ports)),
            [
                (PIN_CONFLICT, "\"A1\"".into()),
                (UNKNOWN_PORT, "btn".into()),
                (MISSPELLED_NAME, "PULMODE".into()),
                (DUPLICATE_ASSIGNMENT, "IO_TYPE=LVCMOS25".into()),
                (MISSING_ARGUMENT, "LOCATE COMP \"led[1]\"".into()),
                (UNKNOWN_COMMAND, "IOBUFF".into()),
                (SYNTAX, "".into()),
                // Not a pin conflict: `spare` is not in the design.
                (UNKNOWN_PORT, "spare".into()),
            ]
        );
    }

    #[test]
    fn gowin_constraints() {
        let text = "// pins\n\
                    IO_LOC \"clk\" 35;\n\
                    IO_LOC \"led[0]\" 35;\n\
                    IO_LOC \"led[1]\" 10,11;\n\
                    IO_LOC \"led[1]\" 12;\n\
                    IO_PORT \"clk\" IO_TYPE=LVCMOS33 PULLMODE=UP;\n\
                    IO_LOC \"nope\";\n\
                    IO_LCO \"led[2]\" 14; // vfl-ignore\n";
        let ports = design_ports();
        assert_eq!(
            rules_of(Kind::Cst, "pins.cst", text, Some(&ports)),
            [
                (PIN_CONFLICT, "35".into()),
                (DUPLICATE_ASSIGNMENT, "12".into()),
                (MISSPELLED_NAME, "PULLMODE".into()),
                (MISSING_ARGUMENT, "IO_LOC \"nope\"".into()),
                (UNKNOWN_COMMAND, "IO_LCO".into()),
            ]
        );
    }

    #[test]
    fn physical_constraints() {
        let libero = "set_io clk -pinname A1 -fixed yes -DIRECTION INPUT\n\
                      set_io {led[0]} -pinname A1 -fixed yes\n\
                      set_io btn -pinname B2\n\
                      set_io -port_name led -pin_name A1 -fixed true\n\
                      set_iobnak Bank1 -vcci 3.30\n";
        let ports = design_ports();
        assert_eq!(
            rules_of(Kind::Pdc, "io.pdc", libero, Some(&ports)),
            [
                (PIN_CONFLICT, "A1".into()),
                (UNKNOWN_PORT, "btn".into()),
                (PIN_CONFLICT, "A1".into()),
                (UNKNOWN_COMMAND, "set_iobnak".into()),
            ]
        );
        let radiant = "ldc_set_location -site {A1} [get_ports clk]\n\
                       ldc_set_location -site {A1} [get_ports {led[0]}]\n\
                       ldc_set_port -iobuf {IO_TYPE=LVCMOS33} [get_ports led]\n\
                       ldc_set_port [get_ports led]\n\
                       create_clock -name clk -period 10 [get_ports clk]\n";
        assert_eq!(
            rules_of(Kind::Pdc, "pins.ldc", radiant, Some(&ports)),
            [
                (PIN_CONFLICT, "{A1}".into()),
                (MISSING_ARGUMENT, "ldc_set_port".into()),
            ]
        );
    }

    #[test]
    fn script_forms() {
        let text = "set_property -name STEPS.SYNTH -value true -objects [get_runs synth_1]\n\
                    set_property -name X -value 1 [current_design]\n\
                    set_parameter -name Language -value vhdl\n\
                    set_property top a [current_fileset]\n\
                    set_property top b [current_fileset]\n\
                    file delete {*}[glob *.log]\n";
        let found = check(Kind::Tcl, Path::new("run.tcl"), text, &Context::default());
        assert!(found.is_empty(), "{found:?}");
        let found = check(
            Kind::Xdc,
            Path::new("a.xdc"),
            "set_property -name IOSTANDARD -value LVCMOS33\n",
            &Context::default(),
        );
        assert_eq!(
            found.iter().map(|f| f.rule).collect::<Vec<_>>(),
            [MISSING_ARGUMENT]
        );
    }

    #[test]
    fn missing_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.vhd"), "").unwrap();
        std::fs::create_dir(dir.path().join("ip")).unwrap();
        std::fs::write(dir.path().join("ip/pll.vhd"), "").unwrap();
        let qsf = dir.path().join("top.qsf");
        let text = "set_global_assignment -name VHDL_FILE a.vhd\n\
                    set_global_assignment -name VHDL_FILE b.vhd\n\
                    set_global_assignment -name VHDL_FILE [file join $::quartus(qip_path) ip/pll.vhd]\n\
                    set_global_assignment -name VHDL_FILE $UNKNOWN/x.vhd\n\
                    set_global_assignment -name FAMILY a.vhd\n\
                    set_global_assignment -name VHDL_FILE ../a.vhd\n";
        let found: Vec<_> = check(Kind::Qsf, &qsf, text, &Context::default())
            .into_iter()
            .map(|f| (f.rule, text[f.range].to_owned()))
            .collect();
        // An unknown variable could be anything: not reported.
        assert_eq!(found, [(MISSING_FILE, "b.vhd".to_owned())]);
        let tcl = dir.path().join("build.tcl");
        let text = "set dir [file dirname [info script]]\n\
                    set root \"\"\n\
                    if {[info exists ::env(ROOT)]} { set root $::env(ROOT) }\n\
                    source \"${root}/tools/common.tcl\"\n\
                    read_vhdl -library work {a.vhd gone.vhd}\n\
                    read_vhdl $dir/ip/pll.vhd\n\
                    read_xdc $dir/pins.xdc\n\
                    set design ./gone.vhd\n\
                    add_files $design\n";
        let found: Vec<_> = check(Kind::Tcl, &tcl, text, &Context::default())
            .into_iter()
            .map(|f| (f.rule, text[f.range].to_owned()))
            .collect();
        // Literal relative paths depend on where the script runs.
        assert_eq!(found, [(MISSING_FILE, "$dir/pins.xdc".to_owned())]);
    }

    #[test]
    fn unknown_ports() {
        let mut ports = Ports::default();
        ports.add("top", ["clk", "rst_n", "leds"]);
        ports.add("other", ["data"]);
        let cx = Context {
            ports: Some(&ports),
            ..Context::default()
        };
        let qsf = "set_global_assignment -name TOP_LEVEL_ENTITY top\n\
                   set_location_assignment PIN_A1 -to clk\n\
                   set_location_assignment PIN_A2 -to leds[3]\n\
                   set_location_assignment PIN_A3 -to led\n\
                   set_location_assignment PIN_A4 -to data\n\
                   set_instance_assignment -name IO_STANDARD \"2.5 V\" -to rst\n\
                   set_location_assignment PIN_A5 -to leds[*]\n";
        let found: Vec<_> = run(Kind::Qsf, qsf, &cx)
            .into_iter()
            .filter(|(r, _, _)| *r == UNKNOWN_PORT)
            .map(|(_, t, m)| (t, m))
            .collect();
        assert_eq!(
            found,
            [
                ("led".to_owned(), "`top` has no port `led`".to_owned()),
                ("data".to_owned(), "`top` has no port `data`".to_owned()),
                ("rst".to_owned(), "`top` has no port `rst`".to_owned()),
            ]
        );
        let xdc = "set_property PACKAGE_PIN W5 [get_ports {clk}]\n\
                   set_property IOSTANDARD LVCMOS33 [get_ports {leds[0] lds[1]}]\n\
                   set_input_delay -clock clk 1 [get_ports -regexp {x.*}]\n";
        let found: Vec<_> = run(Kind::Xdc, xdc, &cx)
            .into_iter()
            .map(|(r, t, _)| (r, t))
            .collect();
        assert_eq!(found, [(UNKNOWN_PORT, "lds[1]".to_owned())]);
    }

    #[test]
    fn clean_files_have_no_findings() {
        let sdc = "set_time_format -unit ns -decimal_places 3\n\
                   create_clock -name clk -period 20.000 [get_ports {clk}]\n\
                   derive_pll_clocks\nderive_clock_uncertainty\n\
                   set_false_path -from [get_ports {rst_n}] -to *\n\
                   set_input_delay -clock clk -max 3 [get_ports {din[*]}]\n\
                   proc my_helper {x} { return $x }\nmy_helper 1\n";
        assert_eq!(rules(Kind::Sdc, sdc), []);
    }
}
