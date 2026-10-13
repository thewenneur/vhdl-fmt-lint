//! Code generated from an entity declaration: an instantiation, a
//! component declaration or instantiation, and a testbench skeleton.
//!
//! [`Entity::from_unit`] reads the generics and ports of an entity from
//! its syntax tree; [`Entity::learn_from`] looks at its architectures for
//! the clocks (`rising_edge(clk)`) and the reset levels (`rst = '0'`).
//! The generators write plain VHDL and pass it through the formatter, so
//! the result follows the `[format]` settings; names and labels come from
//! [`GenerateConfig`] (the `[generate]` table of `vhone.toml`).
//!
//! ```
//! use vhone_config::Config;
//! use vhone_gen::Entity;
//! use vhone_syntax::{Standard, parse};
//!
//! let text = "entity e is port (clk : in bit; q : out bit); end;";
//! let root = parse(text, Standard::Vhdl2008).syntax();
//! let unit = root.children().next().unwrap();
//! let entity = Entity::from_unit(&unit, "work").unwrap();
//! let config = Config::default();
//! assert_eq!(
//!     vhone_gen::instance(&entity, &config).unwrap(),
//!     "u_e : entity work.e\n  port map (\n    clk => clk,\n    q   => q\n  );\n"
//! );
//! ```

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use regex::Regex;
use vhone_config::{Config, GenerateConfig};
use vhone_syntax::{Standard, SyntaxKind, SyntaxNode, SyntaxToken, TextRange};

/// A port mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    In,
    Out,
    Inout,
    Buffer,
    Linkage,
    /// A VHDL-2019 mode view.
    View,
}

/// A port of an entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub name: String,
    pub mode: Mode,
    /// The subtype as written, on one line and without comments. For a
    /// mode view, the subtype after `of`, if any.
    pub subtype: String,
    /// The mode view (`view v of t`), as written.
    pub view: Option<String>,
    pub default: Option<String>,
}

/// What a generic is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenericKind {
    /// A constant, with its subtype.
    Constant { subtype: String },
    /// A generic type (VHDL-2008).
    Type,
    /// A generic subprogram (VHDL-2008).
    Subprogram,
    /// A generic package (VHDL-2008).
    Package,
}

/// A generic of an entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generic {
    pub name: String,
    pub kind: GenericKind,
    /// The default value, or `<>` for a subprogram taken from the context.
    pub default: Option<String>,
    /// The declaration as written, on one line and without comments.
    pub text: String,
}

/// The reset of a testbench.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reset {
    pub name: String,
    pub active_low: bool,
}

/// An entity declaration, as read by [`Entity::from_unit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entity {
    pub name: String,
    pub library: String,
    pub generics: Vec<Generic>,
    pub ports: Vec<Port>,
    /// The library, use and context clauses before the entity, each on
    /// one line.
    pub context: Vec<String>,
    /// Ports the architectures use as clocks.
    clocks: Vec<String>,
    /// Ports the architectures compare against a level, and that level.
    levels: Vec<(String, bool)>,
}

/// Why nothing was generated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenerateError {
    /// A `[generate]` pattern is not a valid regular expression.
    Pattern { key: &'static str, error: String },
    /// The generated text does not format; a bug.
    Format(String),
}

impl std::fmt::Display for GenerateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GenerateError::Pattern { key, error } => {
                write!(f, "invalid generate.{key}: {error}")
            }
            GenerateError::Format(message) => {
                write!(f, "the generated code does not format: {message}")
            }
        }
    }
}

impl std::error::Error for GenerateError {}

/// Every `DESIGN_UNIT` of a file that holds an entity declaration.
pub fn entity_units(root: &SyntaxNode) -> impl Iterator<Item = SyntaxNode> + '_ {
    root.children()
        .filter(|u| u.kind() == SyntaxKind::DESIGN_UNIT)
        .filter(|u| u.children().any(|n| n.kind() == SyntaxKind::ENTITY_DECL))
}

/// A file to look for entities in.
#[derive(Debug, Clone)]
pub struct Source<'a> {
    pub path: &'a Path,
    pub root: SyntaxNode,
    /// The libraries the file belongs to.
    pub libraries: Vec<String>,
}

/// An entity found by [`find_entities`].
#[derive(Debug, Clone)]
pub struct Found {
    pub entity: Entity,
    pub path: PathBuf,
    /// The range of the entity declaration in its file.
    pub range: TextRange,
}

/// The entities of `sources` named `name` (`counter` or `lib.counter`,
/// any case), or all of them, each in every library of its file. Those
/// found by name have learned from their architectures in `sources`.
pub fn find_entities(sources: &[Source], name: Option<&str>) -> Vec<Found> {
    let wanted = name.map(|name| match name.rsplit_once('.') {
        Some((library, name)) => (
            Some(vhone_syntax::identifier_key(library)),
            vhone_syntax::identifier_key(name),
        ),
        None => (None, vhone_syntax::identifier_key(name)),
    });
    let mut found = Vec::new();
    for source in sources {
        for unit in entity_units(&source.root) {
            let libraries = if source.libraries.is_empty() {
                vec!["work".to_owned()]
            } else {
                source.libraries.clone()
            };
            for library in &libraries {
                let Some(mut entity) = Entity::from_unit(&unit, library) else {
                    continue;
                };
                if let Some((lib, name)) = &wanted
                    && (vhone_syntax::identifier_key(&entity.name) != *name
                        || lib
                            .as_ref()
                            .is_some_and(|l| *l != vhone_syntax::identifier_key(library)))
                {
                    continue;
                }
                let key = vhone_syntax::identifier_key(&entity.name);
                for other in sources.iter().filter(|_| wanted.is_some()) {
                    for arch in other.root.children().flat_map(|u| u.children()) {
                        if arch.kind() == SyntaxKind::ARCHITECTURE_BODY
                            && architecture_entity(&arch).is_some_and(|e| e == key)
                        {
                            entity.learn_from(&arch);
                        }
                    }
                }
                let range = unit
                    .children()
                    .find(|n| n.kind() == SyntaxKind::ENTITY_DECL)
                    .map(|n| n.text_range())
                    .unwrap_or_else(|| unit.text_range());
                found.push(Found {
                    entity,
                    path: source.path.to_owned(),
                    range,
                });
            }
        }
    }
    found
}

/// The entity name of an architecture, as a lookup key.
fn architecture_entity(arch: &SyntaxNode) -> Option<String> {
    let name = arch
        .children_with_tokens()
        .skip_while(|e| e.kind() != SyntaxKind::OF_KW)
        .find_map(|e| e.into_node())?;
    let token = name
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| is_name(t.kind()))?;
    Some(vhone_syntax::identifier_key(token.text()))
}

/// The text of `node` on one line: comments dropped, whitespace runs
/// turned into one space.
fn flat(node: &SyntaxNode) -> String {
    let mut out = String::new();
    let mut space = false;
    for token in node
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
    {
        if token.kind().is_trivia() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push_str(token.text());
    }
    out
}

fn tokens(node: &SyntaxNode) -> impl Iterator<Item = SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
}

fn is_name(kind: SyntaxKind) -> bool {
    matches!(kind, SyntaxKind::IDENT | SyntaxKind::EXT_IDENT)
}

/// The names of an `IDENT_LIST`, or the direct identifier of `node`.
fn names(node: &SyntaxNode) -> Vec<String> {
    match node.children().find(|n| n.kind() == SyntaxKind::IDENT_LIST) {
        Some(list) => tokens(&list)
            .filter(|t| is_name(t.kind()))
            .map(|t| t.text().to_owned())
            .collect(),
        None => tokens(node)
            .find(|t| is_name(t.kind()))
            .map(|t| vec![t.text().to_owned()])
            .unwrap_or_default(),
    }
}

/// The text after `:=` in an interface declaration.
fn default_value(node: &SyntaxNode) -> Option<String> {
    let after = node
        .children_with_tokens()
        .skip_while(|e| e.kind() != SyntaxKind::COLON_EQ)
        .skip(1)
        .find_map(|e| e.into_node())?;
    Some(flat(&after))
}

fn interface_elements(clause: &SyntaxNode) -> Vec<SyntaxNode> {
    clause
        .children()
        .find(|n| n.kind() == SyntaxKind::INTERFACE_LIST)
        .map(|l| l.children().collect())
        .unwrap_or_default()
}

impl Entity {
    /// Reads the entity of a `DESIGN_UNIT`, which is in `library`.
    pub fn from_unit(unit: &SyntaxNode, library: &str) -> Option<Entity> {
        let decl = unit
            .children()
            .find(|n| n.kind() == SyntaxKind::ENTITY_DECL)?;
        let name = tokens(&decl).find(|t| is_name(t.kind()))?.text().to_owned();
        let context = unit
            .children()
            .filter(|n| n.kind() == SyntaxKind::CONTEXT_CLAUSE)
            .flat_map(|c| c.children())
            .map(|c| flat(&c))
            .collect();
        let mut entity = Entity {
            name,
            library: library.to_owned(),
            generics: Vec::new(),
            ports: Vec::new(),
            context,
            clocks: Vec::new(),
            levels: Vec::new(),
        };
        for clause in decl.children() {
            match clause.kind() {
                SyntaxKind::GENERIC_CLAUSE => {
                    for element in interface_elements(&clause) {
                        entity.add_generic(&element);
                    }
                }
                SyntaxKind::PORT_CLAUSE => {
                    for element in interface_elements(&clause) {
                        entity.add_port(&element);
                    }
                }
                _ => {}
            }
        }
        Some(entity)
    }

    fn add_generic(&mut self, element: &SyntaxNode) {
        let text = flat(element);
        match element.kind() {
            SyntaxKind::INTERFACE_OBJECT_DECL => {
                let subtype = element
                    .children()
                    .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)
                    .map(|n| flat(&n))
                    .unwrap_or_default();
                let default = default_value(element);
                for name in names(element) {
                    self.generics.push(Generic {
                        name,
                        kind: GenericKind::Constant {
                            subtype: subtype.clone(),
                        },
                        default: default.clone(),
                        text: text.clone(),
                    });
                }
            }
            SyntaxKind::INTERFACE_TYPE_DECL => {
                for name in names(element) {
                    self.generics.push(Generic {
                        name,
                        kind: GenericKind::Type,
                        default: None,
                        text: text.clone(),
                    });
                }
            }
            SyntaxKind::INTERFACE_PACKAGE_DECL => {
                for name in names(element) {
                    self.generics.push(Generic {
                        name,
                        kind: GenericKind::Package,
                        default: None,
                        text: text.clone(),
                    });
                }
            }
            SyntaxKind::INTERFACE_SUBPROGRAM_DECL => {
                // `function f ... is <>` or `is name`.
                let name = element
                    .descendants_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| {
                        matches!(
                            t.kind(),
                            SyntaxKind::IDENT | SyntaxKind::EXT_IDENT | SyntaxKind::STRING_LITERAL
                        )
                    });
                let Some(name) = name else { return };
                let default = element
                    .children_with_tokens()
                    .skip_while(|e| e.kind() != SyntaxKind::IS_KW)
                    .skip(1)
                    .find(|e| !e.kind().is_trivia())
                    .map(|e| match e {
                        rowan::NodeOrToken::Node(n) => flat(&n),
                        rowan::NodeOrToken::Token(t) => t.text().to_owned(),
                    });
                self.generics.push(Generic {
                    name: name.text().to_owned(),
                    kind: GenericKind::Subprogram,
                    default,
                    text,
                });
            }
            _ => {}
        }
    }

    fn add_port(&mut self, element: &SyntaxNode) {
        if element.kind() != SyntaxKind::INTERFACE_OBJECT_DECL {
            return;
        }
        let mut mode = Mode::In;
        let mut view = None;
        let mut subtype = String::new();
        for child in element.children_with_tokens() {
            match child.kind() {
                SyntaxKind::IN_KW => mode = Mode::In,
                SyntaxKind::OUT_KW => mode = Mode::Out,
                SyntaxKind::INOUT_KW => mode = Mode::Inout,
                SyntaxKind::BUFFER_KW => mode = Mode::Buffer,
                SyntaxKind::LINKAGE_KW => mode = Mode::Linkage,
                SyntaxKind::MODE_VIEW_INDICATION => {
                    let node = child.as_node().unwrap();
                    mode = Mode::View;
                    view = Some(flat(node));
                    if let Some(of) = node
                        .children()
                        .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)
                    {
                        subtype = flat(&of);
                    }
                }
                SyntaxKind::SUBTYPE_INDICATION => subtype = flat(child.as_node().unwrap()),
                _ => {}
            }
        }
        let default = default_value(element);
        for name in names(element) {
            self.ports.push(Port {
                name,
                mode,
                subtype: subtype.clone(),
                view: view.clone(),
                default: default.clone(),
            });
        }
    }

    /// Learns from an architecture of the entity (or any code using its
    /// ports): which ports are clocks, and which level resets compare to.
    pub fn learn_from(&mut self, node: &SyntaxNode) {
        let tokens: Vec<SyntaxToken> = node
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| !t.kind().is_trivia())
            .collect();
        let key = |t: &SyntaxToken| vhone_syntax::identifier_key(t.text());
        let port = |t: &SyntaxToken| {
            is_name(t.kind())
                && self
                    .ports
                    .iter()
                    .any(|p| vhone_syntax::identifier_key(&p.name) == key(t))
        };
        let mut clocks = Vec::new();
        let mut levels = Vec::new();
        for (i, t) in tokens.iter().enumerate() {
            let at = |j: usize| tokens.get(j);
            // `rising_edge(clk)`, `falling_edge(clk)`.
            if is_name(t.kind())
                && matches!(key(t).as_str(), "rising_edge" | "falling_edge")
                && at(i + 1).is_some_and(|t| t.kind() == SyntaxKind::LPAREN)
                && at(i + 2).is_some_and(port)
                && at(i + 3).is_some_and(|t| t.kind() == SyntaxKind::RPAREN)
            {
                clocks.push(key(&tokens[i + 2]));
            }
            // `clk'event`.
            if port(t)
                && at(i + 1).is_some_and(|t| t.kind() == SyntaxKind::TICK)
                && at(i + 2).is_some_and(|t| t.text().eq_ignore_ascii_case("event"))
            {
                clocks.push(key(t));
            }
            // `rst = '0'`.
            if port(t)
                && at(i + 1).is_some_and(|t| t.kind() == SyntaxKind::EQ)
                && let Some(level) = at(i + 2).filter(|t| t.kind() == SyntaxKind::CHAR_LITERAL)
            {
                match level.text() {
                    "'0'" => levels.push((key(t), true)),
                    "'1'" => levels.push((key(t), false)),
                    _ => {}
                }
            }
        }
        for clock in clocks {
            if !self.clocks.contains(&clock) {
                self.clocks.push(clock);
            }
        }
        for level in levels {
            if !self.levels.iter().any(|(n, _)| *n == level.0) {
                self.levels.push(level);
            }
        }
    }

    /// The input ports driven as clocks in a testbench.
    pub fn clocks(&self, config: &GenerateConfig) -> Result<Vec<&Port>, GenerateError> {
        let pattern = pattern("clock_pattern", &config.clock_pattern)?;
        Ok(self
            .ports
            .iter()
            .filter(|p| p.mode == Mode::In && is_bit(&p.subtype))
            .filter(|p| {
                self.clocks.contains(&vhone_syntax::identifier_key(&p.name))
                    || (self.clocks.is_empty() && pattern.is_match(&p.name))
            })
            .collect())
    }

    /// The input ports driven as resets in a testbench.
    pub fn resets(&self, config: &GenerateConfig) -> Result<Vec<Reset>, GenerateError> {
        let reset = pattern("reset_pattern", &config.reset_pattern)?;
        let low = pattern("active_low_pattern", &config.active_low_pattern)?;
        let clocks = self.clocks(config)?;
        Ok(self
            .ports
            .iter()
            .filter(|p| p.mode == Mode::In && is_bit(&p.subtype))
            .filter(|p| !clocks.iter().any(|c| c.name == p.name))
            .filter(|p| reset.is_match(&p.name))
            .map(|p| {
                let key = vhone_syntax::identifier_key(&p.name);
                let active_low = match self.levels.iter().find(|(n, _)| *n == key) {
                    Some((_, low)) => *low,
                    None => low.is_match(&p.name),
                };
                Reset {
                    name: p.name.clone(),
                    active_low,
                }
            })
            .collect())
    }
}

fn pattern(key: &'static str, text: &str) -> Result<Regex, GenerateError> {
    Regex::new(text).map_err(|e| GenerateError::Pattern {
        key,
        error: e.to_string(),
    })
}

/// The base name of a subtype, lower case: `std_logic` for
/// `ieee.std_logic_1164.std_logic`.
fn base(subtype: &str) -> String {
    let name = subtype
        .split(['(', ' '])
        .next()
        .unwrap_or_default()
        .rsplit('.')
        .next()
        .unwrap_or_default();
    name.to_ascii_lowercase()
}

/// Whether a subtype is one bit: `std_logic`, `std_ulogic` or `bit`.
fn is_bit(subtype: &str) -> bool {
    matches!(base(subtype).as_str(), "std_logic" | "std_ulogic" | "bit")
}

/// Whether a subtype names an array type without a constraint, which a
/// signal needs.
fn unconstrained(subtype: &str) -> bool {
    !subtype.contains('(')
        && matches!(
            base(subtype).as_str(),
            "std_logic_vector"
                | "std_ulogic_vector"
                | "bit_vector"
                | "unsigned"
                | "signed"
                | "u_unsigned"
                | "u_signed"
                | "boolean_vector"
                | "integer_vector"
                | "real_vector"
                | "time_vector"
                | "sfixed"
                | "ufixed"
                | "string"
        )
}

/// A placeholder value for a generic without a default.
fn placeholder(subtype: &str) -> String {
    match base(subtype).as_str() {
        "integer" | "natural" => "0".into(),
        "positive" => "1".into(),
        "boolean" => "false".into(),
        "real" => "0.0".into(),
        "time" | "delay_length" => "1 ns".into(),
        "string" => "\"\"".into(),
        "std_logic" | "std_ulogic" | "bit" => "'0'".into(),
        "character" => "' '".into(),
        _ if unconstrained(subtype) || subtype.contains('(') => "(others => '0')".into(),
        _ => format!("{subtype}'low"),
    }
}

/// `{entity}` in a name template, replaced.
fn expand(template: &str, entity: &str) -> String {
    template.replace("{entity}", entity)
}

/// Joins `items` as an association list body, one per line.
fn associations(items: &[(String, String)]) -> String {
    items
        .iter()
        .map(|(formal, actual)| format!("    {formal} => {actual}"))
        .collect::<Vec<_>>()
        .join(",\n")
}

/// The `generic map` and `port map` of an instance, with each formal
/// associated with the same name.
fn maps(entity: &Entity) -> String {
    let mut out = String::new();
    let generics: Vec<(String, String)> = entity
        .generics
        .iter()
        .filter(|g| !(g.kind == GenericKind::Subprogram && g.default.is_some()))
        .map(|g| (g.name.clone(), g.name.clone()))
        .collect();
    if !generics.is_empty() {
        let _ = write!(out, "\n  generic map (\n{}\n  )", associations(&generics));
    }
    let ports: Vec<(String, String)> = entity
        .ports
        .iter()
        .map(|p| (p.name.clone(), p.name.clone()))
        .collect();
    if !ports.is_empty() {
        let _ = write!(out, "\n  port map (\n{}\n  )", associations(&ports));
    }
    out
}

/// Formats a whole file.
fn format(text: &str, config: &Config) -> Result<String, GenerateError> {
    vhone_fmt::format(text, &config.format, config.standard())
        .map_err(|e| GenerateError::Format(e.to_string()))
}

/// Formats `inner` as the statements (or declarations, with `declarative`)
/// of an architecture, and returns it without the architecture and with
/// one indentation level less.
fn format_fragment(
    inner: &str,
    declarative: bool,
    config: &Config,
) -> Result<String, GenerateError> {
    const MARK: &str = "vhone_generated_";
    let wrapped = if declarative {
        format!("architecture {MARK} of {MARK} is\n{inner}\nbegin\nend;\n")
    } else {
        format!("architecture {MARK} of {MARK} is\nbegin\n{inner}\nend;\n")
    };
    let out = format(&wrapped, config)?;
    let lines: Vec<&str> = out.lines().collect();
    let begin = lines
        .iter()
        .position(|l| l.trim().eq_ignore_ascii_case("begin"))
        .ok_or_else(|| GenerateError::Format("no `begin`".into()))?;
    let body = if declarative {
        &lines[1..begin]
    } else {
        &lines[begin + 1..lines.len() - 1]
    };
    let unit = if config.format.use_tabs {
        "\t".to_owned()
    } else {
        " ".repeat(config.format.indent as usize)
    };
    let mut result = String::new();
    for line in body {
        result.push_str(line.strip_prefix(unit.as_str()).unwrap_or(line));
        result.push('\n');
    }
    Ok(result)
}

/// An entity instantiation: `u_e : entity work.e generic map (...) port map (...);`.
pub fn instance(entity: &Entity, config: &Config) -> Result<String, GenerateError> {
    let label = expand(&config.generate.instance_label, &entity.name);
    let library = if entity.library.is_empty() {
        "work"
    } else {
        &entity.library
    };
    let text = format!(
        "{label} : entity {library}.{}{};",
        entity.name,
        maps(entity)
    );
    format_fragment(&text, false, config)
}

/// A component instantiation: `u_e : e generic map (...) port map (...);`.
pub fn component_instance(entity: &Entity, config: &Config) -> Result<String, GenerateError> {
    let label = expand(&config.generate.instance_label, &entity.name);
    let text = format!("{label} : {}{};", entity.name, maps(entity));
    format_fragment(&text, false, config)
}

/// The interface clauses of a component or entity declaration.
fn clauses(entity: &Entity) -> String {
    let mut out = String::new();
    if !entity.generics.is_empty() {
        let lines: Vec<String> = entity
            .generics
            .iter()
            .map(|g| match &g.kind {
                GenericKind::Constant { subtype } => match &g.default {
                    Some(d) => format!("    {} : {subtype} := {d}", g.name),
                    None => format!("    {} : {subtype}", g.name),
                },
                _ => format!("    {}", g.text),
            })
            .collect();
        // Several names of one subprogram or type declaration are one line.
        let mut seen = Vec::new();
        let lines: Vec<String> = lines
            .into_iter()
            .filter(|l| {
                let dup = seen.contains(l);
                seen.push(l.clone());
                !dup
            })
            .collect();
        let _ = write!(out, "\n  generic (\n{}\n  );", lines.join(";\n"));
    }
    if !entity.ports.is_empty() {
        let lines: Vec<String> = entity
            .ports
            .iter()
            .map(|p| {
                let mode = match (&p.mode, &p.view) {
                    (Mode::View, Some(view)) => return format!("    {} : {view}", p.name),
                    (Mode::In, _) => "in",
                    (Mode::Out, _) => "out",
                    (Mode::Inout, _) => "inout",
                    (Mode::Buffer, _) => "buffer",
                    (Mode::Linkage, _) => "linkage",
                    (Mode::View, None) => "in",
                };
                match &p.default {
                    Some(d) => format!("    {} : {mode} {} := {d}", p.name, p.subtype),
                    None => format!("    {} : {mode} {}", p.name, p.subtype),
                }
            })
            .collect();
        let _ = write!(out, "\n  port (\n{}\n  );", lines.join(";\n"));
    }
    out
}

/// A component declaration matching the entity.
pub fn component(entity: &Entity, config: &Config) -> Result<String, GenerateError> {
    let text = format!(
        "component {} is{}\nend component {};",
        entity.name,
        clauses(entity),
        entity.name
    );
    format_fragment(&text, true, config)
}

/// The file name of the testbench of `entity`, without directory.
pub fn testbench_file_name(entity: &Entity, config: &GenerateConfig, extension: &str) -> String {
    format!(
        "{}.{extension}",
        expand(&config.testbench_name, &entity.name)
    )
}

/// A testbench: an entity without ports, a signal for every port, the
/// design under test, a clock for every clock input, and a stimulus
/// process that holds the resets for `reset_cycles` clock cycles.
pub fn testbench(entity: &Entity, config: &Config) -> Result<String, GenerateError> {
    let generate = &config.generate;
    let standard = config.standard();
    let name = expand(&generate.testbench_name, &entity.name);
    let clocks = entity.clocks(generate)?;
    let resets = entity.resets(generate)?;
    let library = if entity.library.is_empty() || entity.library.eq_ignore_ascii_case("work") {
        "work".to_owned()
    } else {
        entity.library.clone()
    };

    let mut out = String::new();
    // Context: the entity's own clauses, with what the testbench needs.
    let mut context: Vec<String> = Vec::new();
    let mut add = |clause: String| {
        if !context.iter().any(|c| c.eq_ignore_ascii_case(&clause)) {
            context.push(clause);
        }
    };
    let uses_std_logic = entity
        .ports
        .iter()
        .any(|p| base(&p.subtype).starts_with("std_"));
    if uses_std_logic || entity.context.is_empty() {
        add("library ieee;".into());
        add("use ieee.std_logic_1164.all;".into());
    }
    for clause in &entity.context {
        add(clause.clone());
    }
    if library != "work" {
        add(format!("library {library};"));
    }
    for clause in &context {
        out.push_str(clause);
        out.push('\n');
    }
    let _ = write!(out, "\nentity {name} is\nend entity {name};\n\n");
    let _ = writeln!(
        out,
        "architecture {} of {name} is",
        generate.testbench_architecture
    );

    // Generics become constants, so the port subtypes that use them stay valid.
    let mut constants = String::new();
    for g in &entity.generics {
        match &g.kind {
            GenericKind::Constant { subtype } => {
                let (value, todo) = match &g.default {
                    Some(d) => (d.clone(), ""),
                    None => (placeholder(subtype), "  -- TODO: choose a value"),
                };
                let _ = writeln!(
                    constants,
                    "  constant {} : {subtype} := {value};{todo}",
                    g.name
                );
            }
            GenericKind::Type => {
                let _ = writeln!(
                    constants,
                    "  subtype {} is integer;  -- TODO: the type to test with",
                    g.name
                );
            }
            GenericKind::Subprogram | GenericKind::Package => {}
        }
    }
    if !constants.is_empty() {
        out.push_str(&constants);
        out.push('\n');
    }
    if !clocks.is_empty() {
        let _ = writeln!(
            out,
            "  constant CLK_PERIOD : time := {};\n",
            generate.clock_period
        );
    }
    for p in &entity.ports {
        if p.mode == Mode::View && p.subtype.is_empty() {
            let _ = writeln!(
                out,
                "  -- TODO: signal {} : <the record type of {}>;",
                p.name,
                p.view.as_deref().unwrap_or("its view")
            );
            continue;
        }
        let (subtype, todo) = if unconstrained(&p.subtype) {
            let range = if base(&p.subtype) == "string" {
                "(1 to 8)"
            } else {
                "(7 downto 0)"
            };
            (format!("{}{range}", p.subtype), "  -- TODO: the width")
        } else {
            (p.subtype.clone(), "")
        };
        let initial = if clocks.iter().any(|c| c.name == p.name) {
            Some(bit_value(false))
        } else if let Some(r) = resets.iter().find(|r| r.name == p.name) {
            Some(bit_value(!r.active_low))
        } else if p.mode == Mode::In {
            match &p.default {
                Some(d) => Some(d.clone()),
                None if is_bit(&p.subtype) => Some(bit_value(false)),
                None if subtype.contains('(') && is_logic_vector(&p.subtype) => {
                    Some("(others => '0')".into())
                }
                None => None,
            }
        } else {
            None
        };
        match initial {
            Some(v) => {
                let _ = writeln!(out, "  signal {} : {subtype} := {v};{todo}", p.name);
            }
            None => {
                let _ = writeln!(out, "  signal {} : {subtype};{todo}", p.name);
            }
        }
    }
    if !clocks.is_empty() {
        out.push_str("\n  signal done : boolean := false;\n");
    }
    out.push_str("begin\n");

    // The design under test.
    let mut dut = format!(
        "  {} : entity {library}.{}",
        generate.testbench_instance, entity.name
    );
    let generic_map: Vec<String> = entity
        .generics
        .iter()
        .filter(|g| matches!(g.kind, GenericKind::Constant { .. } | GenericKind::Type))
        .map(|g| format!("      {} => {}", g.name, g.name))
        .collect();
    if !generic_map.is_empty() {
        let _ = write!(
            dut,
            "\n    generic map (\n{}\n    )",
            generic_map.join(",\n")
        );
    }
    let port_map: Vec<String> = entity
        .ports
        .iter()
        .map(|p| format!("      {} => {}", p.name, p.name))
        .collect();
    if !port_map.is_empty() {
        let _ = write!(dut, "\n    port map (\n{}\n    )", port_map.join(",\n"));
    }
    out.push_str(&dut);
    out.push_str(";\n");

    for clock in &clocks {
        let zero = bit_value(false);
        let _ = write!(
            out,
            "\n  {name} <= not {name} after CLK_PERIOD / 2 when not done else {zero};\n",
            name = clock.name
        );
    }

    // Stimulus.
    out.push_str("\n  stimulus : process\n  begin\n");
    let wait_cycles = |out: &mut String, cycles: u32| match clocks.first() {
        Some(clock) => {
            let _ = writeln!(out, "    for i in 1 to {cycles} loop");
            let _ = writeln!(out, "      wait until rising_edge({});", clock.name);
            out.push_str("    end loop;\n");
        }
        None => {
            let _ = writeln!(out, "    wait for {cycles} * {};", generate.clock_period);
        }
    };
    if !resets.is_empty() {
        for r in &resets {
            let _ = writeln!(out, "    {} <= {};", r.name, bit_value(!r.active_low));
        }
        wait_cycles(&mut out, generate.reset_cycles);
        for r in &resets {
            let _ = writeln!(out, "    {} <= {};", r.name, bit_value(r.active_low));
        }
        out.push('\n');
    }
    out.push_str("    -- TODO: drive the inputs and check the outputs.\n");
    wait_cycles(&mut out, 100);
    out.push('\n');
    let _ = writeln!(out, "    report \"{name}: done\";");
    if !clocks.is_empty() {
        out.push_str("    done <= true;\n");
    }
    if standard >= Standard::Vhdl2008 {
        out.push_str("    std.env.finish;\n");
    }
    out.push_str("    wait;\n  end process stimulus;\n");
    let _ = writeln!(out, "end architecture {};", generate.testbench_architecture);
    format(&out, config)
}

fn is_logic_vector(subtype: &str) -> bool {
    matches!(
        base(subtype).as_str(),
        "std_logic_vector" | "std_ulogic_vector" | "bit_vector" | "unsigned" | "signed"
    )
}

/// `'0'` or `'1'`.
fn bit_value(high: bool) -> String {
    if high { "'1'" } else { "'0'" }.into()
}

#[cfg(test)]
mod tests;
