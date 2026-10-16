//! Design specifications: a page per entity built from the code (its
//! interface, clocks and resets, behavior, state machines, registers and
//! instances) and from spec comments, with an index of the requirements.
//!
//! Spec comments start with a tag (`[doc.spec]` names them):
//!
//! ```vhdl
//! -- @spec The transmitter idles high.
//! -- @req UART-TX-1: A frame starts with one start bit.
//! -- @verifies UART-TX-1, UART-TX-2: frame checks
//! ```
//!
//! The comment lines below a tagged one continue it, up to a blank comment
//! line or another tag. A comment applies to the declaration or statement
//! below it, or to the one whose line it ends.

use std::collections::{BTreeMap, HashMap, HashSet};

use vfl_config::{DocConfig, DocFormat, SpecConfig, SpecSection};
use vfl_syntax::{LineIndex, SyntaxKind, SyntaxNode, SyntaxToken, identifier_key};

use crate::comments::brief;
use crate::model::{Architecture, Design, Entity, FileInput, Process, ident, label, own_name};
use crate::page::{Block, Cell, Inline, NavItem, OutputFile, Page, code, text};

// ---------------------------------------------------------------- Comments

/// What a tagged comment says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagKind {
    Spec,
    Requirement,
    Verifies,
}

/// The tags of a project's spec comments.
#[derive(Debug, Clone, Default)]
pub(crate) struct Tags {
    spec: String,
    requirement: String,
    verifies: String,
}

impl Tags {
    pub(crate) fn new(config: &SpecConfig) -> Tags {
        Tags {
            spec: config.tag.clone(),
            requirement: config.requirement_tag.clone(),
            verifies: config.verifies_tag.clone(),
        }
    }

    /// The tag `body` (a comment without its delimiters) starts with, and
    /// the text after it.
    pub(crate) fn tagged<'t>(&self, body: &'t str) -> Option<(TagKind, &'t str)> {
        let body = body.trim_start();
        [
            (TagKind::Spec, &self.spec),
            (TagKind::Requirement, &self.requirement),
            (TagKind::Verifies, &self.verifies),
        ]
        .into_iter()
        .find_map(|(kind, tag)| {
            let rest = body
                .strip_prefix(tag.as_str())
                .filter(|_| !tag.is_empty())?;
            match rest.chars().next() {
                None => Some((kind, rest)),
                Some(c) if c.is_whitespace() || c == ':' => {
                    Some((kind, rest.strip_prefix(':').unwrap_or(rest).trim()))
                }
                Some(_) => None,
            }
        })
    }
}

/// The text of a comment without its delimiters and doc marker.
pub(crate) fn comment_body<'t>(token: &'t SyntaxToken, marker: &str) -> &'t str {
    let text = token.text();
    match token.kind() {
        SyntaxKind::LINE_COMMENT => {
            let rest = text.strip_prefix("--").unwrap_or(text);
            match rest.strip_prefix(marker) {
                Some(rest) if !marker.is_empty() => rest,
                _ => rest,
            }
        }
        SyntaxKind::BLOCK_COMMENT => {
            let inner = text.strip_prefix("/*").unwrap_or(text);
            inner.strip_suffix("*/").unwrap_or(inner)
        }
        _ => "",
    }
}

/// The part a comment plays in a run of comment lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Role<'t> {
    /// Starts a spec comment.
    Start(TagKind, &'t str),
    /// Continues the spec comment above.
    Continue(&'t str),
    /// Not part of a spec comment.
    Other,
}

/// Reads a run of comments (no blank line or code between them) line by
/// line: tagged comments and the lines continuing them.
#[derive(Debug, Default)]
pub(crate) struct Reader {
    open: bool,
    fence: bool,
}

impl Reader {
    /// The role of the next comment of the run. `own_line`: nothing but
    /// whitespace before it on its line.
    pub(crate) fn next<'t>(
        &mut self,
        body: &'t str,
        block: bool,
        own_line: bool,
        tags: &Tags,
    ) -> Role<'t> {
        if let Some((kind, rest)) = tags.tagged(body) {
            // A block comment, or one at the end of a line of code, is whole.
            self.open = !block && own_line;
            self.fence = rest.trim_start().starts_with("```");
            return Role::Start(kind, rest);
        }
        if !self.open || block || !own_line {
            self.open = false;
            return Role::Other;
        }
        let line = body.strip_prefix(' ').unwrap_or(body).trim_end();
        if line.trim_start().starts_with("```") {
            self.fence = !self.fence;
            return Role::Continue(line);
        }
        let blank = line
            .chars()
            .all(|c| c.is_whitespace() || "-=*#~_+/".contains(c));
        if blank && !self.fence {
            self.open = false;
            return Role::Other;
        }
        Role::Continue(line)
    }

    /// Ends the run: code or a blank line follows.
    pub(crate) fn reset(&mut self) {
        self.open = false;
        self.fence = false;
    }
}

// ---------------------------------------------------------------- Model

/// One spec comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecItem {
    pub kind: TagKind,
    /// The requirement's ID, or the IDs a `verifies` comment names.
    pub ids: Vec<String>,
    pub text: String,
    /// Relative to the project root, with `/` separators.
    pub file: String,
    /// The 1-based line of the tag.
    pub line: u32,
    /// The design unit it is in: an entity (for its architectures too) or
    /// a package (for its body too).
    pub unit: UnitRef,
    /// The architecture it is in.
    pub architecture: Option<String>,
    pub anchor: Anchor,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UnitRef {
    pub package: bool,
    pub library: String,
    /// The name as written.
    pub name: String,
}

/// What a spec comment applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub kind: AnchorKind,
    /// The names it declares (`a, b : in bit` declares two), else its label,
    /// else empty.
    pub names: Vec<String>,
    /// The 1-based line of the declaration or statement.
    pub line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnchorKind {
    /// The entity or package itself.
    Unit,
    Architecture,
    Generic,
    Port,
    Signal,
    Constant,
    Variable,
    Type,
    Alias,
    Subprogram,
    Component,
    Process,
    Instance,
    Assignment,
    Generate,
    Statement,
}

impl AnchorKind {
    fn label(self) -> &'static str {
        match self {
            AnchorKind::Unit => "",
            AnchorKind::Architecture => "Architecture",
            AnchorKind::Generic => "Generic",
            AnchorKind::Port => "Port",
            AnchorKind::Signal => "Signal",
            AnchorKind::Constant => "Constant",
            AnchorKind::Variable => "Variable",
            AnchorKind::Type => "Type",
            AnchorKind::Alias => "Alias",
            AnchorKind::Subprogram => "Subprogram",
            AnchorKind::Component => "Component",
            AnchorKind::Process => "Process",
            AnchorKind::Instance => "Instance",
            AnchorKind::Assignment => "Assignment",
            AnchorKind::Generate => "Generate",
            AnchorKind::Statement => "Statement",
        }
    }
}

impl SpecItem {
    /// Whether it applies to `kind` named `key`.
    fn applies_to(&self, kind: AnchorKind, key: &str) -> bool {
        self.anchor.kind == kind && self.anchor.names.iter().any(|n| identifier_key(n) == key)
    }

    fn requirement(&self) -> Option<&str> {
        match self.kind {
            TagKind::Requirement => Some(self.ids.first().map_or("", String::as_str)),
            _ => None,
        }
    }
}

/// The spec comments of `files`, in file order.
pub(crate) fn collect(files: &[FileInput], config: &DocConfig) -> Vec<SpecItem> {
    let tags = Tags::new(&config.spec);
    let mut items = Vec::new();
    for file in files {
        let lines = LineIndex::new(file.text);
        let line_of = |offset| lines.line_col(file.text, offset).line + 1;
        let mut reader = Reader::default();
        // The spec comment being read, and where it ends.
        let mut current: Option<(SpecItem, SyntaxToken, bool)> = None;
        let flush = |current: &mut Option<(SpecItem, SyntaxToken, bool)>,
                     items: &mut Vec<SpecItem>| {
            if let Some((mut item, last, own_line)) = current.take() {
                let at = if own_line {
                    next_code(&last).or_else(|| previous_code(&last))
                } else {
                    previous_code(&last)
                };
                if let Some(at) = at
                    && let Some((unit, architecture, anchor)) = anchor_of(&at, &line_of, file)
                {
                    item.text = item.text.trim().to_owned();
                    item.unit = unit;
                    item.architecture = architecture;
                    item.anchor = anchor;
                    items.push(item);
                }
            }
        };
        for token in file
            .root
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
        {
            match token.kind() {
                SyntaxKind::WHITESPACE => {
                    if token.text().matches('\n').count() > 1 {
                        reader.reset();
                        flush(&mut current, &mut items);
                    }
                }
                SyntaxKind::LINE_COMMENT | SyntaxKind::BLOCK_COMMENT => {
                    let body = comment_body(&token, &config.comment_marker);
                    let own_line = starts_line(&token);
                    let block = token.kind() == SyntaxKind::BLOCK_COMMENT;
                    match reader.next(body, block, own_line, &tags) {
                        Role::Start(kind, rest) => {
                            flush(&mut current, &mut items);
                            let (ids, text) = split_ids(kind, rest);
                            let item = SpecItem {
                                kind,
                                ids,
                                text,
                                file: file.path.clone(),
                                line: line_of(token.text_range().start()),
                                unit: UnitRef {
                                    package: false,
                                    library: String::new(),
                                    name: String::new(),
                                },
                                architecture: None,
                                anchor: Anchor {
                                    kind: AnchorKind::Unit,
                                    names: Vec::new(),
                                    line: 0,
                                },
                            };
                            current = Some((item, token.clone(), own_line));
                        }
                        Role::Continue(line) => {
                            if let Some((item, last, _)) = &mut current {
                                if !item.text.is_empty() {
                                    item.text.push('\n');
                                }
                                item.text.push_str(line);
                                *last = token.clone();
                            }
                        }
                        Role::Other => flush(&mut current, &mut items),
                    }
                }
                _ => {
                    reader.reset();
                    flush(&mut current, &mut items);
                }
            }
        }
        flush(&mut current, &mut items);
    }
    items
}

/// The IDs a tag's text starts with, and the rest of it.
fn split_ids(kind: TagKind, rest: &str) -> (Vec<String>, String) {
    match kind {
        TagKind::Spec => (Vec::new(), rest.to_owned()),
        TagKind::Requirement => {
            let rest = rest.trim_start();
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let id = rest[..end].trim_end_matches([':', ',', '.']);
            let text = rest[end..]
                .trim_start()
                .trim_start_matches([':', '-'])
                .trim_start();
            (vec![id.to_owned()], text.to_owned())
        }
        TagKind::Verifies => {
            let (ids, note) = rest.split_once(':').unwrap_or((rest, ""));
            let ids = ids
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .collect();
            (ids, note.trim().to_owned())
        }
    }
}

fn is_trivia(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::WHITESPACE | SyntaxKind::LINE_COMMENT | SyntaxKind::BLOCK_COMMENT
    )
}

/// The first token of code after `token`.
fn next_code(token: &SyntaxToken) -> Option<SyntaxToken> {
    let mut at = token.next_token();
    while let Some(t) = at {
        if !is_trivia(t.kind()) {
            return Some(t);
        }
        at = t.next_token();
    }
    None
}

/// The last token of code before `token`, separators skipped: the end of
/// the declaration or statement a trailing comment ends the line of.
fn previous_code(token: &SyntaxToken) -> Option<SyntaxToken> {
    let mut at = token.prev_token();
    while let Some(t) = at {
        if !is_trivia(t.kind()) && !matches!(t.kind(), SyntaxKind::SEMI | SyntaxKind::COMMA) {
            return Some(t);
        }
        at = t.prev_token();
    }
    None
}

/// Whether only whitespace precedes `token` on its line.
fn starts_line(token: &SyntaxToken) -> bool {
    match token.prev_token() {
        None => true,
        Some(prev) if prev.kind() == SyntaxKind::WHITESPACE => {
            prev.text().contains('\n') || prev.prev_token().is_none()
        }
        Some(_) => false,
    }
}

/// The unit, architecture and declaration or statement `token` is part of.
fn anchor_of(
    token: &SyntaxToken,
    line_of: &dyn Fn(vfl_syntax::TextSize) -> u32,
    file: &FileInput,
) -> Option<(UnitRef, Option<String>, Anchor)> {
    let mut candidates: Vec<(AnchorKind, SyntaxNode)> = Vec::new();
    let mut unit_node = None;
    for node in token.parent_ancestors() {
        let kind = match node.kind() {
            SyntaxKind::ENTITY_DECL
            | SyntaxKind::ARCHITECTURE_BODY
            | SyntaxKind::PACKAGE_DECL
            | SyntaxKind::PACKAGE_BODY => {
                unit_node = Some(node);
                break;
            }
            SyntaxKind::DESIGN_UNIT => {
                // Before the unit: its context clause.
                unit_node = node.children().find(|n| {
                    matches!(
                        n.kind(),
                        SyntaxKind::ENTITY_DECL
                            | SyntaxKind::ARCHITECTURE_BODY
                            | SyntaxKind::PACKAGE_DECL
                            | SyntaxKind::PACKAGE_BODY
                    )
                });
                candidates.clear();
                break;
            }
            SyntaxKind::INTERFACE_OBJECT_DECL
            | SyntaxKind::INTERFACE_TYPE_DECL
            | SyntaxKind::INTERFACE_SUBPROGRAM_DECL
            | SyntaxKind::INTERFACE_PACKAGE_DECL => {
                if node
                    .ancestors()
                    .any(|a| a.kind() == SyntaxKind::PORT_CLAUSE)
                {
                    AnchorKind::Port
                } else {
                    AnchorKind::Generic
                }
            }
            SyntaxKind::SIGNAL_DECL => AnchorKind::Signal,
            SyntaxKind::CONSTANT_DECL => AnchorKind::Constant,
            SyntaxKind::VARIABLE_DECL => AnchorKind::Variable,
            SyntaxKind::TYPE_DECL | SyntaxKind::SUBTYPE_DECL => AnchorKind::Type,
            SyntaxKind::ALIAS_DECL => AnchorKind::Alias,
            SyntaxKind::SUBPROGRAM_DECL | SyntaxKind::SUBPROGRAM_BODY => AnchorKind::Subprogram,
            SyntaxKind::COMPONENT_DECL => AnchorKind::Component,
            SyntaxKind::PROCESS_STMT => AnchorKind::Process,
            SyntaxKind::COMPONENT_INSTANTIATION_STMT => AnchorKind::Instance,
            SyntaxKind::SIGNAL_ASSIGN_STMT | SyntaxKind::SELECTED_ASSIGN_STMT => {
                AnchorKind::Assignment
            }
            SyntaxKind::FOR_GENERATE_STMT
            | SyntaxKind::IF_GENERATE_STMT
            | SyntaxKind::CASE_GENERATE_STMT
            | SyntaxKind::BLOCK_STMT => AnchorKind::Generate,
            SyntaxKind::ASSERT_STMT | SyntaxKind::PROCEDURE_CALL_STMT => AnchorKind::Statement,
            _ => continue,
        };
        candidates.push((kind, node));
    }
    let unit_node = unit_node?;
    let (unit, architecture) = unit_of(&unit_node, &file.library)?;
    // The outermost declaration or statement, through generate and block
    // statements unless nothing else is there.
    let chosen = candidates
        .iter()
        .rev()
        .find(|(k, _)| *k != AnchorKind::Generate)
        .or_else(|| candidates.first());
    let anchor = match chosen {
        Some((kind, node)) => Anchor {
            kind: *kind,
            names: names_of(*kind, node),
            line: line_of(node.text_range().start()),
        },
        None => Anchor {
            kind: if unit_node.kind() == SyntaxKind::ARCHITECTURE_BODY {
                AnchorKind::Architecture
            } else {
                AnchorKind::Unit
            },
            names: architecture.iter().cloned().collect(),
            line: line_of(unit_node.text_range().start()),
        },
    };
    Some((unit, architecture, anchor))
}

fn unit_of(node: &SyntaxNode, library: &str) -> Option<(UnitRef, Option<String>)> {
    let named = |n: &SyntaxNode| {
        n.children()
            .find(|c| c.kind() == SyntaxKind::NAME)
            .and_then(|c| ident(&c))
            .map(|t| t.text().to_owned())
    };
    let first_ident = |n: &SyntaxNode| ident(n).map(|t| t.text().to_owned());
    let (package, name, architecture) = match node.kind() {
        SyntaxKind::ENTITY_DECL => (false, first_ident(node)?, None),
        SyntaxKind::ARCHITECTURE_BODY => (false, named(node)?, Some(first_ident(node)?)),
        SyntaxKind::PACKAGE_DECL | SyntaxKind::PACKAGE_BODY => (true, first_ident(node)?, None),
        _ => return None,
    };
    Some((
        UnitRef {
            package,
            library: library.to_owned(),
            name,
        },
        architecture,
    ))
}

fn names_of(kind: AnchorKind, node: &SyntaxNode) -> Vec<String> {
    let list = || {
        node.children()
            .find(|n| n.kind() == SyntaxKind::IDENT_LIST)
            .map(|l| {
                l.children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
                    .map(|t| t.text().to_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    match kind {
        AnchorKind::Generic
        | AnchorKind::Port
        | AnchorKind::Signal
        | AnchorKind::Constant
        | AnchorKind::Variable => {
            let names = list();
            if names.is_empty() {
                own_name(node).into_iter().collect()
            } else {
                names
            }
        }
        AnchorKind::Subprogram => node
            .children()
            .find(|n| {
                matches!(
                    n.kind(),
                    SyntaxKind::FUNCTION_SPEC | SyntaxKind::PROCEDURE_SPEC
                )
            })
            .and_then(|s| own_name(&s))
            .into_iter()
            .collect(),
        AnchorKind::Type | AnchorKind::Alias | AnchorKind::Component => {
            own_name(node).into_iter().collect()
        }
        _ => label(node).into_iter().collect(),
    }
}

/// What is wrong with the spec comments: requirements without an ID or
/// given twice, and tests verifying unknown requirements.
pub fn problems(items: &[SpecItem]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: HashMap<&str, &SpecItem> = HashMap::new();
    for item in items {
        let Some(id) = item.requirement() else {
            continue;
        };
        if id.is_empty() {
            out.push(format!(
                "{}:{}: requirement without an ID",
                item.file, item.line
            ));
        } else if let Some(first) = seen.get(id) {
            out.push(format!(
                "{}:{}: requirement {id} is already given at {}:{}",
                item.file, item.line, first.file, first.line
            ));
        } else {
            seen.insert(id, item);
        }
    }
    for item in items.iter().filter(|i| i.kind == TagKind::Verifies) {
        if item.ids.is_empty() {
            out.push(format!(
                "{}:{}: verifies no requirement",
                item.file, item.line
            ));
        }
        for id in &item.ids {
            if !seen.contains_key(id.as_str()) {
                out.push(format!(
                    "{}:{}: verifies unknown requirement {id}",
                    item.file, item.line
                ));
            }
        }
    }
    out
}

// ---------------------------------------------------------------- Pages

/// The files of the specification of `design`, relative to the output
/// directory.
pub fn generate(design: &Design, items: &[SpecItem], config: &DocConfig) -> Vec<OutputFile> {
    let spec = &config.spec;
    let format = spec.format.unwrap_or(config.format);
    let title = spec
        .title
        .clone()
        .unwrap_or_else(|| format!("{} specification", design.title));
    let ctx = Ctx::new(design, items, config, format);
    let mut pages = Vec::new();
    let mut nav = Vec::new();
    let mut listed = Vec::new();
    for (i, e) in design.entities.iter().enumerate() {
        let own = ctx.items_of(false, &e.library, &e.name);
        if (e.is_testbench() && !spec.testbenches) || (spec.annotated_only && own.is_empty()) {
            continue;
        }
        pages.push(ctx.entity_page(i, &own));
        nav.push(NavItem {
            section: "Entities",
            page: e.page.clone(),
            label: e.name.clone(),
        });
        listed.push((
            e.page.clone(),
            e.name.clone(),
            "entity",
            brief(&e.description),
            own,
        ));
    }
    for p in &design.packages {
        let own = ctx.items_of(true, &p.library, &p.name);
        if own.is_empty() {
            continue;
        }
        pages.push(ctx.package_page(p, &own));
        nav.push(NavItem {
            section: "Packages",
            page: p.page.clone(),
            label: p.name.clone(),
        });
        listed.push((
            p.page.clone(),
            p.name.clone(),
            "package",
            brief(&p.description),
            own,
        ));
    }
    pages.insert(0, ctx.index(&title, &listed));
    if nav.len() > 500 {
        nav.clear();
    }
    let config = DocConfig {
        format,
        ..config.clone()
    };
    crate::render(&pages, &title, &nav, &config)
}

/// Section numbers, as `2.1`.
#[derive(Default)]
struct Numbers {
    section: u32,
    sub: u32,
}

impl Numbers {
    fn section(&mut self, title: &str) -> Block {
        self.section += 1;
        self.sub = 0;
        Block::Heading {
            level: 2,
            content: vec![text(format!("{} {title}", self.section))],
            id: Some(identifier_key(title).replace(' ', "-")),
        }
    }

    fn sub(&mut self, content: Vec<Inline>) -> Block {
        self.sub += 1;
        let mut all = vec![text(format!("{}.{} ", self.section, self.sub))];
        all.extend(content);
        Block::Heading {
            level: 3,
            content: all,
            id: None,
        }
    }
}

struct Ctx<'a> {
    design: &'a Design,
    items: &'a [SpecItem],
    config: &'a DocConfig,
    format: DocFormat,
    /// Requirement ID -> the `verifies` comments naming it.
    verified: HashMap<&'a str, Vec<&'a SpecItem>>,
}

impl<'a> Ctx<'a> {
    fn new(
        design: &'a Design,
        items: &'a [SpecItem],
        config: &'a DocConfig,
        format: DocFormat,
    ) -> Self {
        let mut verified: HashMap<&str, Vec<&SpecItem>> = HashMap::new();
        for item in items.iter().filter(|i| i.kind == TagKind::Verifies) {
            for id in &item.ids {
                verified.entry(id.as_str()).or_default().push(item);
            }
        }
        Ctx {
            design,
            items,
            config,
            format,
            verified,
        }
    }

    fn sections(&self) -> &[SpecSection] {
        &self.config.spec.sections
    }

    /// The spec comments and requirements of a unit (not its `verifies`).
    fn items_of(&self, package: bool, library: &str, name: &str) -> Vec<&'a SpecItem> {
        let key = identifier_key(name);
        let library = identifier_key(library);
        self.items
            .iter()
            .filter(|i| {
                i.kind != TagKind::Verifies
                    && i.unit.package == package
                    && identifier_key(&i.unit.name) == key
                    && identifier_key(&i.unit.library) == library
            })
            .collect()
    }

    fn extension(&self) -> &'static str {
        match self.format {
            DocFormat::Html => "html",
            DocFormat::Markdown => "md",
            DocFormat::Asciidoc => "adoc",
        }
    }

    fn doc(&self, text: &str, blocks: &mut Vec<Block>) {
        crate::pages::doc(text, blocks, self.config);
    }

    /// Spec comments as blocks: specs as text, requirements as a list.
    fn item_blocks(&self, items: &[&SpecItem], blocks: &mut Vec<Block>) {
        let mut requirements = Vec::new();
        for item in items {
            match item.kind {
                TagKind::Spec => self.doc(&item.text, blocks),
                TagKind::Requirement => requirements.push(format!(
                    "- `{}` {}",
                    item.ids.join(", "),
                    item.text.split_whitespace().collect::<Vec<_>>().join(" ")
                )),
                TagKind::Verifies => {}
            }
        }
        if !requirements.is_empty() {
            blocks.push(Block::Doc(requirements.join("\n")));
        }
    }

    /// Spec text and requirement IDs in a table cell.
    fn item_cell(&self, items: &[&SpecItem]) -> Cell {
        let mut cell = Vec::new();
        for item in items {
            if !cell.is_empty() {
                cell.push(text(" "));
            }
            match item.kind {
                TagKind::Spec => cell.push(text(
                    item.text.split_whitespace().collect::<Vec<_>>().join(" "),
                )),
                TagKind::Requirement => cell.push(code(item.ids.join(", "))),
                TagKind::Verifies => {}
            }
        }
        cell
    }

    /// Who verifies requirement `id`.
    fn verified_cell(&self, id: &str) -> Cell {
        let mut cell = Vec::new();
        for (n, item) in self.verified.get(id).into_iter().flatten().enumerate() {
            if n > 0 {
                cell.push(text("; "));
            }
            let mut name = item.unit.name.clone();
            if let Some(label) = item.anchor.names.first().filter(|_| {
                !matches!(
                    item.anchor.kind,
                    AnchorKind::Unit | AnchorKind::Architecture
                )
            }) {
                name = format!("{name}.{label}");
            }
            cell.push(code(name));
            cell.push(text(" "));
            cell.push(code(format!("{}:{}", item.file, item.line)));
        }
        cell
    }

    fn anchor_para(&self, item: &SpecItem) -> Block {
        let a = &item.anchor;
        let mut para = Vec::new();
        let what = a.kind.label();
        if !what.is_empty() {
            para.push(text(format!("{what} ")));
        }
        if a.names.is_empty() {
            para.push(text(format!("(line {})", a.line)));
        } else {
            for (i, n) in a.names.iter().enumerate() {
                if i > 0 {
                    para.push(text(", "));
                }
                para.push(code(n.clone()));
            }
        }
        para.push(text(" · "));
        para.push(code(format!("{}:{}", item.file, a.line)));
        Block::Para(para)
    }

    /// Spec comments grouped by what they apply to, each group under a line
    /// naming it.
    fn grouped(&self, items: &[&SpecItem], blocks: &mut Vec<Block>) {
        let mut groups: Vec<Vec<&SpecItem>> = Vec::new();
        for item in items {
            match groups
                .iter_mut()
                .find(|g| g[0].anchor == item.anchor && g[0].file == item.file)
            {
                Some(group) => group.push(item),
                None => groups.push(vec![item]),
            }
        }
        for group in groups {
            blocks.push(self.anchor_para(group[0]));
            self.item_blocks(&group, blocks);
        }
    }

    fn requirements_table(&self, items: &[&SpecItem]) -> Option<Block> {
        let rows: Vec<Vec<Cell>> = items
            .iter()
            .filter_map(|item| {
                let id = item.requirement()?;
                let mut applies = Vec::new();
                match item.anchor.kind {
                    AnchorKind::Unit if item.unit.package => applies.push(text("package")),
                    AnchorKind::Unit => applies.push(text("entity")),
                    kind => applies.push(text(format!("{} ", kind.label().to_lowercase()))),
                }
                for (i, n) in item.anchor.names.iter().enumerate() {
                    if i > 0 {
                        applies.push(text(", "));
                    }
                    applies.push(code(n.clone()));
                }
                Some(vec![
                    vec![code(id.to_owned())],
                    vec![text(
                        item.text.split_whitespace().collect::<Vec<_>>().join(" "),
                    )],
                    applies,
                    vec![code(format!("{}:{}", item.file, item.line))],
                    self.verified_cell(id),
                ])
            })
            .collect();
        (!rows.is_empty()).then(|| Block::Table {
            head: vec!["ID", "Requirement", "Applies to", "Source", "Verified by"],
            rows,
        })
    }

    fn index(
        &self,
        title: &str,
        listed: &[(String, String, &'static str, String, Vec<&SpecItem>)],
    ) -> Page {
        let mut blocks = vec![Block::Heading {
            level: 1,
            content: vec![text(title.to_owned())],
            id: None,
        }];
        let requirements: Vec<&SpecItem> = self
            .items
            .iter()
            .filter(|i| i.kind == TagKind::Requirement)
            .collect();
        let verified = requirements
            .iter()
            .filter(|r| {
                r.requirement()
                    .is_some_and(|id| self.verified.contains_key(id))
            })
            .count();
        blocks.push(Block::Para(vec![text(format!(
            "{} units · {} requirements, {} verified",
            listed.len(),
            requirements.len(),
            verified
        ))]));
        if !listed.is_empty() {
            blocks.push(Block::Heading {
                level: 2,
                content: vec![text("Units")],
                id: Some("units".into()),
            });
            blocks.push(Block::Table {
                head: vec!["Unit", "Kind", "Requirements", "Summary"],
                rows: listed
                    .iter()
                    .map(|(page, name, kind, summary, own)| {
                        let count = own
                            .iter()
                            .filter(|i| i.kind == TagKind::Requirement)
                            .count();
                        vec![
                            vec![Inline::Link {
                                text: name.clone(),
                                code: true,
                                page: page.clone(),
                                anchor: None,
                            }],
                            vec![text(*kind)],
                            vec![text(count.to_string())],
                            vec![text(summary.clone())],
                        ]
                    })
                    .collect(),
            });
        }
        if !requirements.is_empty() {
            blocks.push(Block::Heading {
                level: 2,
                content: vec![text("Requirements")],
                id: Some("requirements".into()),
            });
            let pages: HashMap<(bool, String, String), String> = self
                .design
                .entities
                .iter()
                .map(|e| ((false, identifier_key(&e.library), e.key()), e.page.clone()))
                .chain(self.design.packages.iter().map(|p| {
                    (
                        (true, identifier_key(&p.library), identifier_key(&p.name)),
                        p.page.clone(),
                    )
                }))
                .collect();
            let shown: HashSet<&str> = listed.iter().map(|l| l.0.as_str()).collect();
            blocks.push(Block::Table {
                head: vec!["ID", "Unit", "Requirement", "Verified by"],
                rows: requirements
                    .iter()
                    .map(|r| {
                        let id = r.requirement().unwrap_or_default();
                        let key = (
                            r.unit.package,
                            identifier_key(&r.unit.library),
                            identifier_key(&r.unit.name),
                        );
                        let unit = match pages.get(&key).filter(|p| shown.contains(p.as_str())) {
                            Some(page) => Inline::Link {
                                text: r.unit.name.clone(),
                                code: true,
                                page: page.clone(),
                                anchor: Some("requirements".into()),
                            },
                            None => code(r.unit.name.clone()),
                        };
                        vec![
                            vec![code(id.to_owned())],
                            vec![unit],
                            vec![text(
                                r.text.split_whitespace().collect::<Vec<_>>().join(" "),
                            )],
                            self.verified_cell(id),
                        ]
                    })
                    .collect(),
            });
        }
        let problems = problems(self.items);
        if !problems.is_empty() {
            blocks.push(Block::Heading {
                level: 2,
                content: vec![text("Problems")],
                id: Some("problems".into()),
            });
            blocks.push(Block::List(
                problems.into_iter().map(|p| vec![text(p)]).collect(),
            ));
        }
        Page {
            id: "index".into(),
            title: title.to_owned(),
            blocks,
        }
    }

    fn package_page(&self, p: &crate::model::Package, own: &[&SpecItem]) -> Page {
        let mut blocks = vec![Block::Heading {
            level: 1,
            content: vec![text("Package "), code(p.name.clone())],
            id: None,
        }];
        blocks.push(Block::Para(vec![
            text("Library "),
            code(p.library.clone()),
            text(" · "),
            code(format!("{}:{}", p.file, p.line)),
        ]));
        let mut n = Numbers::default();
        let unit: Vec<&SpecItem> = own
            .iter()
            .copied()
            .filter(|i| i.anchor.kind == AnchorKind::Unit)
            .collect();
        let rest: Vec<&SpecItem> = own
            .iter()
            .copied()
            .filter(|i| i.anchor.kind != AnchorKind::Unit)
            .collect();
        blocks.push(n.section("Overview"));
        self.doc(&p.description, &mut blocks);
        self.item_blocks(&unit, &mut blocks);
        if !rest.is_empty() {
            blocks.push(n.section("Declarations"));
            self.grouped(&rest, &mut blocks);
        }
        if let Some(table) = self.requirements_table(own) {
            blocks.push(n.section("Requirements"));
            blocks.push(table);
        }
        Page {
            id: p.page.clone(),
            title: p.name.clone(),
            blocks,
        }
    }

    fn entity_page(&self, index: usize, own: &[&SpecItem]) -> Page {
        let e = &self.design.entities[index];
        let what = if e.verilog { "Module " } else { "Entity " };
        let mut blocks = vec![Block::Heading {
            level: 1,
            content: vec![text(what), code(e.name.clone())],
            id: None,
        }];
        let mut meta = vec![
            text("Library "),
            code(e.library.clone()),
            text(" · "),
            code(format!("{}:{}", e.file, e.line)),
        ];
        if !e.architectures.is_empty() && !e.verilog {
            meta.push(text(" · architecture "));
            for (i, a) in e.architectures.iter().enumerate() {
                if i > 0 {
                    meta.push(text(", "));
                }
                meta.push(code(a.name.clone()));
            }
        }
        blocks.push(Block::Para(meta));
        let mut claimed: HashSet<usize> = HashSet::new();
        let mut n = Numbers::default();
        // The behavior takes what the other sections leave: it is written
        // last, in its place. (position in `blocks`, section number)
        let mut behavior_at = None;
        for section in self.sections() {
            match section {
                SpecSection::Overview => {
                    blocks.push(n.section("Overview"));
                    self.doc(&e.description, &mut blocks);
                    let unit = self.pick(own, &mut claimed, |i| i.anchor.kind == AnchorKind::Unit);
                    self.item_blocks(&unit, &mut blocks);
                }
                SpecSection::Interface => {
                    self.interface(index, own, &mut claimed, &mut n, &mut blocks)
                }
                SpecSection::Clocks => self.clocks(index, &mut n, &mut blocks),
                SpecSection::Behavior => {
                    let heading = n.section("Behavior");
                    behavior_at = Some((blocks.len(), n.section));
                    blocks.push(heading);
                }
                SpecSection::Fsm => self.fsms(e, own, &mut claimed, &mut n, &mut blocks),
                SpecSection::Registers => self.registers(e, own, &mut claimed, &mut n, &mut blocks),
                SpecSection::Instances => {
                    self.instances(index, own, &mut claimed, &mut n, &mut blocks)
                }
                SpecSection::Requirements => {
                    if let Some(table) = self.requirements_table(own) {
                        blocks.push(n.section("Requirements"));
                        blocks.push(table);
                    }
                }
            }
        }
        if let Some((at, number)) = behavior_at {
            let mut sub = Numbers {
                section: number,
                sub: 0,
            };
            let mut behavior = Vec::new();
            self.behavior(e, own, &mut claimed, &mut sub, &mut behavior);
            blocks.splice(at + 1..at + 1, behavior);
        }
        Page {
            id: e.page.clone(),
            title: e.name.clone(),
            blocks,
        }
    }

    /// The items of `own` that `keep` takes and no section took yet; marks
    /// them taken.
    fn pick(
        &self,
        own: &[&'a SpecItem],
        claimed: &mut HashSet<usize>,
        keep: impl Fn(&SpecItem) -> bool,
    ) -> Vec<&'a SpecItem> {
        let mut out = Vec::new();
        for (i, item) in own.iter().enumerate() {
            if !claimed.contains(&i) && keep(item) {
                claimed.insert(i);
                out.push(*item);
            }
        }
        out
    }

    fn interface(
        &self,
        index: usize,
        own: &[&'a SpecItem],
        claimed: &mut HashSet<usize>,
        n: &mut Numbers,
        blocks: &mut Vec<Block>,
    ) {
        let e = &self.design.entities[index];
        blocks.push(n.section("Interface"));
        if self.config.diagrams.symbol && !e.ports.is_empty() {
            blocks.push(Block::Diagram {
                file: format!("{}.symbol", e.page),
                alt: format!("Symbol of {}", e.name),
                svg: crate::svg::symbol(e, crate::pages::symbol_options(self.config)),
                mermaid: None,
            });
        }
        if !e.generics.is_empty() {
            blocks.push(n.sub(vec![text("Generics")]));
            let rows = e
                .generics
                .iter()
                .map(|g| {
                    let key = identifier_key(&g.name);
                    let specs = self.pick(own, claimed, |i| {
                        i.architecture.is_none() && i.applies_to(AnchorKind::Generic, &key)
                    });
                    vec![
                        vec![code(g.name.clone())],
                        type_cell(&g.ty),
                        g.default.iter().map(|d| code(d.clone())).collect(),
                        vec![text(g.description.clone())],
                        self.item_cell(&specs),
                    ]
                })
                .collect();
            blocks.push(Block::Table {
                head: vec!["Name", "Type", "Default", "Description", "Specification"],
                rows,
            });
        }
        if !e.ports.is_empty() {
            blocks.push(n.sub(vec![text("Ports")]));
            let rows = e
                .ports
                .iter()
                .map(|p| {
                    let key = identifier_key(&p.name);
                    let specs = self.pick(own, claimed, |i| {
                        i.architecture.is_none() && i.applies_to(AnchorKind::Port, &key)
                    });
                    vec![
                        vec![code(p.name.clone())],
                        vec![text(if p.mode.is_empty() {
                            "in"
                        } else {
                            p.mode.as_str()
                        })],
                        type_cell(&p.ty),
                        timing_cell(&timing(self.design, index, &key, 0)),
                        vec![text(p.description.clone())],
                        self.item_cell(&specs),
                    ]
                })
                .collect();
            blocks.push(Block::Table {
                head: vec![
                    "Name",
                    "Direction",
                    "Type",
                    "Timing",
                    "Description",
                    "Specification",
                ],
                rows,
            });
        }
    }

    fn clocks(&self, index: usize, n: &mut Numbers, blocks: &mut Vec<Block>) {
        let e = &self.design.entities[index];
        // Clock key -> (name, falling, processes); reset key -> (name, kind, clock, processes).
        let mut clocks: BTreeMap<String, (String, bool, Vec<String>)> = BTreeMap::new();
        let mut resets: BTreeMap<String, (String, String, String, Vec<String>)> = BTreeMap::new();
        for arch in &e.architectures {
            for p in &arch.processes {
                let Some(clock) = &p.clock else { continue };
                let name = process_name(p);
                clocks
                    .entry(identifier_key(clock))
                    .or_insert_with(|| (clock.clone(), p.falling, Vec::new()))
                    .2
                    .push(name.clone());
                if let Some(reset) = &p.reset {
                    resets
                        .entry(identifier_key(&reset.signal))
                        .or_insert_with(|| {
                            (
                                reset.signal.clone(),
                                reset_kind(p),
                                clock.clone(),
                                Vec::new(),
                            )
                        })
                        .3
                        .push(name);
                }
            }
        }
        // Ports that are clocks or resets of the instances only.
        for port in &e.ports {
            let key = identifier_key(&port.name);
            let users = || -> Vec<String> {
                e.architectures
                    .iter()
                    .flat_map(|a| &a.instances)
                    .filter(|i| {
                        i.port_map
                            .iter()
                            .any(|(_, actual)| identifier_key(actual) == key)
                    })
                    .map(|i| i.path())
                    .collect()
            };
            match timing(self.design, index, &key, 0) {
                Timing::Clock(falling) if !clocks.contains_key(&key) => {
                    let used = users();
                    clocks.insert(key, (port.name.clone(), falling, used));
                }
                Timing::Reset(kind, clock) if !resets.contains_key(&key) => {
                    let used = users();
                    resets.insert(
                        key,
                        (port.name.clone(), kind, clock.unwrap_or_default(), used),
                    );
                }
                _ => {}
            }
        }
        blocks.push(n.section("Clocks and resets"));
        if clocks.is_empty() {
            blocks.push(Block::Para(vec![text("No clock.")]));
            return;
        }
        let names = |list: &[String]| -> Cell {
            let mut cell = Vec::new();
            for (i, name) in list.iter().enumerate() {
                if i > 0 {
                    cell.push(text(", "));
                }
                cell.push(code(name.clone()));
            }
            cell
        };
        blocks.push(Block::Table {
            head: vec!["Clock", "Edge", "Used by"],
            rows: clocks
                .values()
                .map(|(name, falling, processes)| {
                    vec![
                        vec![code(name.clone())],
                        vec![text(if *falling { "falling" } else { "rising" })],
                        names(processes),
                    ]
                })
                .collect(),
        });
        if !resets.is_empty() {
            blocks.push(Block::Table {
                head: vec!["Reset", "Kind", "Clock", "Used by"],
                rows: resets
                    .values()
                    .map(|(name, kind, clock, processes)| {
                        vec![
                            vec![code(name.clone())],
                            vec![text(kind.clone())],
                            if clock.is_empty() {
                                Vec::new()
                            } else {
                                vec![code(clock.clone())]
                            },
                            names(processes),
                        ]
                    })
                    .collect(),
            });
        }
        for a in 0..e.architectures.len() {
            let Some(domains) = crate::clocks::domains(self.design, index, a) else {
                continue;
            };
            if self.config.diagrams.clocks
                && let Some((svg, _)) = crate::clocks::diagram(self.design, index, a)
            {
                blocks.push(Block::Diagram {
                    file: format!(
                        "{}.{}.clocks",
                        e.page,
                        identifier_key(&e.architectures[a].name)
                    ),
                    alt: format!("Clock domains of {}({})", e.name, e.architectures[a].name),
                    svg,
                    mermaid: None,
                });
            }
            if !domains.crossings.is_empty() {
                blocks.push(Block::Table {
                    head: vec!["Crossing", "From", "To", "Read by"],
                    rows: domains
                        .crossings
                        .iter()
                        .map(|c| {
                            vec![
                                vec![code(c.signal.clone())],
                                vec![code(c.from.clone())],
                                vec![code(c.to.clone())],
                                vec![code(c.reader.clone())],
                            ]
                        })
                        .collect(),
                });
            }
        }
    }

    fn behavior(
        &self,
        e: &Entity,
        own: &[&'a SpecItem],
        claimed: &mut HashSet<usize>,
        n: &mut Numbers,
        blocks: &mut Vec<Block>,
    ) {
        // Interface specs no section took (it is left out).
        let interface = self.pick(own, claimed, |i| {
            i.architecture.is_none() && i.anchor.kind != AnchorKind::Unit
        });
        self.grouped(&interface, blocks);
        for arch in &e.architectures {
            let key = identifier_key(&arch.name);
            let in_arch = |i: &SpecItem| {
                i.architecture
                    .as_deref()
                    .is_some_and(|a| identifier_key(a) == key)
            };
            if !e.verilog {
                blocks.push(n.sub(vec![text("Architecture "), code(arch.name.clone())]));
            }
            self.doc(&arch.description, blocks);
            let top = self.pick(own, claimed, |i| {
                in_arch(i) && i.anchor.kind == AnchorKind::Architecture
            });
            self.item_blocks(&top, blocks);
            if !arch.processes.is_empty() {
                blocks.push(Block::Table {
                    head: vec!["Process", "Kind", "Drives", "Description"],
                    rows: arch
                        .processes
                        .iter()
                        .map(|p| {
                            let mut drives = Vec::new();
                            for (i, d) in p.drives.iter().enumerate() {
                                if i > 0 {
                                    drives.push(text(", "));
                                }
                                drives.push(code(d.clone()));
                            }
                            vec![
                                if p.label.is_some() {
                                    vec![code(process_name(p))]
                                } else {
                                    vec![text(process_name(p))]
                                },
                                process_kind(p),
                                drives,
                                vec![text(p.description.clone())],
                            ]
                        })
                        .collect(),
                });
            }
            let rest = self.pick(own, claimed, |i| in_arch(i));
            self.grouped(&rest, blocks);
        }
        // Spec comments in architectures of other files, say.
        let rest = self.pick(own, claimed, |i| i.kind != TagKind::Verifies);
        self.grouped(&rest, blocks);
    }

    fn fsms(
        &self,
        e: &Entity,
        own: &[&'a SpecItem],
        claimed: &mut HashSet<usize>,
        n: &mut Numbers,
        blocks: &mut Vec<Block>,
    ) {
        if e.architectures.iter().all(|a| a.fsms.is_empty()) {
            return;
        }
        blocks.push(n.section("State machines"));
        for arch in &e.architectures {
            let arch_key = identifier_key(&arch.name);
            for fsm in &arch.fsms {
                blocks.push(n.sub(vec![text("State machine "), code(fsm.signal.clone())]));
                let mut meta = vec![text("Type "), code(fsm.type_name.clone())];
                if let Some(process) = &fsm.process {
                    meta.push(text(" · process "));
                    meta.push(code(process.clone()));
                }
                if let Some(initial) = &fsm.initial {
                    meta.push(text(" · initial state "));
                    meta.push(code(initial.clone()));
                }
                blocks.push(Block::Para(meta));
                self.doc(&fsm.description, blocks);
                // The state signal's name, without a record element.
                let signal = identifier_key(fsm.signal.split('.').next().unwrap_or(&fsm.signal));
                let process = fsm.process.as_deref().map(identifier_key);
                let type_key = identifier_key(&fsm.type_name);
                let specs = self.pick(own, claimed, |i| {
                    i.architecture
                        .as_deref()
                        .is_some_and(|a| identifier_key(a) == arch_key)
                        && (i.applies_to(AnchorKind::Signal, &signal)
                            || i.applies_to(AnchorKind::Type, &type_key)
                            || process
                                .as_deref()
                                .is_some_and(|p| i.applies_to(AnchorKind::Process, p)))
                });
                self.item_blocks(&specs, blocks);
                if self.config.diagrams.fsm {
                    blocks.push(Block::Diagram {
                        file: format!(
                            "{}.{arch_key}.fsm-{}",
                            e.page,
                            crate::pages::file_safe(&fsm.signal)
                        ),
                        alt: format!("State machine {}", fsm.signal),
                        svg: crate::svg::fsm(fsm),
                        mermaid: Some(crate::text::fsm_mermaid(fsm)),
                    });
                }
                let mut states = vec![text("States: ")];
                for (i, s) in fsm.states.iter().enumerate() {
                    if i > 0 {
                        states.push(text(", "));
                    }
                    states.push(code(s.clone()));
                }
                blocks.push(Block::Para(states));
                blocks.push(Block::Table {
                    head: vec!["From", "To", "Condition"],
                    rows: fsm
                        .transitions
                        .iter()
                        .map(|t| {
                            vec![
                                vec![code(t.from.clone())],
                                vec![code(t.to.clone())],
                                t.condition.iter().map(|c| code(c.clone())).collect(),
                            ]
                        })
                        .collect(),
                });
            }
        }
    }

    fn registers(
        &self,
        e: &Entity,
        own: &[&'a SpecItem],
        claimed: &mut HashSet<usize>,
        n: &mut Numbers,
        blocks: &mut Vec<Block>,
    ) {
        let mut rows = Vec::new();
        for arch in &e.architectures {
            let arch_key = identifier_key(&arch.name);
            let mut seen = HashSet::new();
            for p in arch.processes.iter().filter(|p| p.clock.is_some()) {
                for driven in &p.drives {
                    let key = identifier_key(driven);
                    if !seen.insert(key.clone()) {
                        continue;
                    }
                    let (ty, description, kind) = match find_object(arch, e, &key) {
                        Some(found) => found,
                        None => continue,
                    };
                    let specs = self.pick(own, claimed, |i| {
                        (i.architecture
                            .as_deref()
                            .is_some_and(|a| identifier_key(a) == arch_key)
                            && i.applies_to(AnchorKind::Signal, &key))
                            || (i.architecture.is_none()
                                && kind == AnchorKind::Port
                                && i.applies_to(AnchorKind::Port, &key))
                    });
                    let mut clock = vec![code(p.clock.clone().unwrap_or_default())];
                    if p.falling {
                        clock.push(text(" (falling)"));
                    }
                    let reset: Cell = match &p.reset {
                        Some(r) => vec![
                            code(r.signal.clone()),
                            text(format!(" ({})", reset_kind(p))),
                        ],
                        None => Vec::new(),
                    };
                    rows.push(vec![
                        vec![code(driven.clone())],
                        type_cell(&ty),
                        clock,
                        reset,
                        vec![code(process_name(p))],
                        vec![text(description)],
                        self.item_cell(&specs),
                    ]);
                }
            }
        }
        if rows.is_empty() {
            return;
        }
        blocks.push(n.section("Registers"));
        blocks.push(Block::Table {
            head: vec![
                "Register",
                "Type",
                "Clock",
                "Reset",
                "Process",
                "Description",
                "Specification",
            ],
            rows,
        });
    }

    fn instances(
        &self,
        index: usize,
        own: &[&'a SpecItem],
        claimed: &mut HashSet<usize>,
        n: &mut Numbers,
        blocks: &mut Vec<Block>,
    ) {
        let e = &self.design.entities[index];
        if e.architectures.iter().all(|a| a.instances.is_empty()) {
            return;
        }
        blocks.push(n.section("Sub-blocks"));
        let page_link = |i: usize| format!("{}.{}", self.design.entities[i].page, self.extension());
        for (a, arch) in e.architectures.iter().enumerate() {
            if arch.instances.is_empty() {
                continue;
            }
            let arch_key = identifier_key(&arch.name);
            if self.config.diagrams.block
                && let Some(svg) = crate::block::block(self.design, index, a, Some(&page_link))
            {
                blocks.push(Block::Diagram {
                    file: format!("{}.{arch_key}.block", e.page),
                    alt: format!("Block diagram of {}({})", e.name, arch.name),
                    svg,
                    mermaid: None,
                });
            }
            let rows = arch
                .instances
                .iter()
                .map(|instance| {
                    let key = identifier_key(&instance.label);
                    let specs = self.pick(own, claimed, |i| {
                        i.architecture
                            .as_deref()
                            .is_some_and(|a| identifier_key(a) == arch_key)
                            && i.applies_to(AnchorKind::Instance, &key)
                    });
                    let unit = match crate::hierarchy::resolve(self.design, instance, &e.library) {
                        Some(target) => vec![Inline::Link {
                            text: instance.unit.clone(),
                            code: true,
                            page: self.design.entities[target].page.clone(),
                            anchor: None,
                        }],
                        None => vec![code(instance.unit.clone())],
                    };
                    vec![
                        vec![code(instance.path())],
                        unit,
                        vec![text(instance.description.clone())],
                        self.item_cell(&specs),
                    ]
                })
                .collect();
            blocks.push(Block::Table {
                head: vec!["Instance", "Unit", "Description", "Specification"],
                rows,
            });
        }
    }
}

fn type_cell(ty: &str) -> Cell {
    if ty.is_empty() {
        Vec::new()
    } else {
        vec![code(ty.to_owned())]
    }
}

fn process_name(p: &Process) -> String {
    p.label
        .clone()
        .unwrap_or_else(|| format!("(line {})", p.line))
}

fn reset_kind(p: &Process) -> String {
    let Some(r) = &p.reset else {
        return String::new();
    };
    let kind = if r.asynchronous {
        "asynchronous"
    } else {
        "synchronous"
    };
    match r.active_high {
        Some(true) => format!("{kind}, active high"),
        Some(false) => format!("{kind}, active low"),
        None => kind.to_owned(),
    }
}

fn process_kind(p: &Process) -> Cell {
    match &p.clock {
        Some(clock) => {
            let mut cell = vec![
                text(if p.falling {
                    "clocked on falling "
                } else {
                    "clocked on "
                }),
                code(clock.clone()),
            ];
            if let Some(r) = &p.reset {
                cell.push(text(format!(
                    ", {} reset ",
                    if r.asynchronous {
                        "asynchronous"
                    } else {
                        "synchronous"
                    }
                )));
                cell.push(code(r.signal.clone()));
            }
            cell
        }
        None if p.sensitivity.is_empty() => vec![text("waits")],
        None => vec![text("combinational")],
    }
}

/// The type, description and kind of the signal or port named `key`.
fn find_object(arch: &Architecture, e: &Entity, key: &str) -> Option<(String, String, AnchorKind)> {
    if let Some(s) = arch.signals.iter().find(|s| identifier_key(&s.name) == key) {
        return Some((s.ty.clone(), s.description.clone(), AnchorKind::Signal));
    }
    e.ports
        .iter()
        .find(|p| identifier_key(&p.name) == key)
        .map(|p| (p.ty.clone(), p.description.clone(), AnchorKind::Port))
}

/// When a port changes or is read.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Timing {
    /// A clock, with whether it is used on its falling edge.
    Clock(bool),
    /// A reset: its kind, and its clock if synchronous.
    Reset(String, Option<String>),
    /// The clocks it is registered and sampled on, with their edge.
    On {
        registered: Vec<(String, bool)>,
        sampled: Vec<(String, bool)>,
    },
    Combinational,
    Unknown,
}

/// How deep [`timing`] follows a port into the instances it connects to.
const MAX_DEPTH: usize = 8;

/// When port `key` of entity `index` changes or is read: from its own
/// processes and assignments, else from the ports of the instances it is
/// connected to.
fn timing(design: &Design, index: usize, key: &str, depth: usize) -> Timing {
    let e = &design.entities[index];
    let mut registered: Vec<(String, bool)> = Vec::new();
    let mut sampled: Vec<(String, bool)> = Vec::new();
    let mut combinational = false;
    let push = |list: &mut Vec<(String, bool)>, clock: &str, falling: bool| {
        if !list
            .iter()
            .any(|(c, _)| identifier_key(c) == identifier_key(clock))
        {
            list.push((clock.to_owned(), falling));
        }
    };
    for arch in &e.architectures {
        for p in &arch.processes {
            if p.clock.as_deref().is_some_and(|c| identifier_key(c) == key) {
                return Timing::Clock(p.falling);
            }
            if let Some(r) = p
                .reset
                .as_ref()
                .filter(|r| identifier_key(&r.signal) == key)
            {
                let clock = (!r.asynchronous).then(|| p.clock.clone()).flatten();
                return Timing::Reset(reset_kind(p), clock);
            }
            let drives = p.drives.iter().any(|d| identifier_key(d) == key);
            let reads = p.reads.iter().any(|r| identifier_key(r) == key);
            match (&p.clock, drives, reads) {
                (Some(c), true, _) => push(&mut registered, c, p.falling),
                (Some(c), false, true) => push(&mut sampled, c, p.falling),
                (None, true, _) | (None, _, true) => combinational = true,
                _ => {}
            }
        }
    }
    // Through the instances, with their clocks renamed to the actuals.
    if depth < MAX_DEPTH {
        for arch in &e.architectures {
            for instance in &arch.instances {
                let Some(child) = crate::hierarchy::resolve(design, instance, &e.library) else {
                    continue;
                };
                let actual_of = |formal: &str| {
                    instance
                        .port_map
                        .iter()
                        .find(|(f, _)| identifier_key(f) == identifier_key(formal))
                        .map_or_else(|| formal.to_owned(), |(_, a)| a.clone())
                };
                for (formal, actual) in &instance.port_map {
                    if identifier_key(actual) != key || formal.is_empty() {
                        continue;
                    }
                    match timing(design, child, &identifier_key(formal), depth + 1) {
                        Timing::Clock(falling) => return Timing::Clock(falling),
                        Timing::Reset(kind, clock) => {
                            return Timing::Reset(kind, clock.map(|c| actual_of(&c)));
                        }
                        Timing::On {
                            registered: r,
                            sampled: s,
                        } => {
                            for (c, f) in r {
                                push(&mut registered, &actual_of(&c), f);
                            }
                            for (c, f) in s {
                                push(&mut sampled, &actual_of(&c), f);
                            }
                        }
                        Timing::Combinational => combinational = true,
                        Timing::Unknown => {}
                    }
                }
            }
        }
    }
    if !registered.is_empty() || !sampled.is_empty() {
        return Timing::On {
            registered,
            sampled,
        };
    }
    let assigned = e
        .architectures
        .iter()
        .flat_map(|a| &a.assignments)
        .any(|a| {
            identifier_key(&a.target) == key || a.reads.iter().any(|r| identifier_key(r) == key)
        });
    if combinational || assigned {
        Timing::Combinational
    } else {
        Timing::Unknown
    }
}

fn timing_cell(timing: &Timing) -> Cell {
    match timing {
        Timing::Clock(false) => vec![text("clock")],
        Timing::Clock(true) => vec![text("clock (falling edge)")],
        Timing::Reset(kind, None) => vec![text(format!("reset ({kind})"))],
        Timing::Reset(kind, Some(clock)) => vec![
            text(format!("reset ({kind}, on ")),
            code(clock.clone()),
            text(")"),
        ],
        Timing::On {
            registered,
            sampled,
        } => {
            let mut cell = Vec::new();
            for (what, list) in [("registered on ", registered), ("sampled on ", sampled)] {
                if list.is_empty() {
                    continue;
                }
                if !cell.is_empty() {
                    cell.push(text("; "));
                }
                cell.push(text(what));
                for (i, (clock, falling)) in list.iter().enumerate() {
                    if i > 0 {
                        cell.push(text(", "));
                    }
                    if *falling {
                        cell.push(text("falling "));
                    }
                    cell.push(code(clock.clone()));
                }
            }
            cell
        }
        Timing::Combinational => vec![text("combinational")],
        Timing::Unknown => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags() -> Tags {
        Tags::new(&SpecConfig::default())
    }

    #[test]
    fn reads_tags() {
        let t = tags();
        assert_eq!(
            t.tagged(" @spec Idle high"),
            Some((TagKind::Spec, "Idle high"))
        );
        assert_eq!(
            t.tagged("@req: X-1 a"),
            Some((TagKind::Requirement, "X-1 a"))
        );
        assert_eq!(t.tagged("@verifies A"), Some((TagKind::Verifies, "A")));
        assert_eq!(t.tagged("@spec"), Some((TagKind::Spec, "")));
        assert_eq!(t.tagged("@specs"), None);
        assert_eq!(t.tagged("see @spec"), None);
    }

    #[test]
    fn splits_ids() {
        assert_eq!(
            split_ids(TagKind::Requirement, "UART-1: Idle high."),
            (vec!["UART-1".into()], "Idle high.".into())
        );
        assert_eq!(
            split_ids(TagKind::Requirement, "UART-1 - Idle high."),
            (vec!["UART-1".into()], "Idle high.".into())
        );
        assert_eq!(
            split_ids(TagKind::Verifies, "A-1, A-2 B: frames"),
            (
                vec!["A-1".into(), "A-2".into(), "B".into()],
                "frames".into()
            )
        );
    }

    #[test]
    fn continues_until_a_blank_line() {
        let t = tags();
        let mut r = Reader::default();
        assert_eq!(
            r.next(" @spec a", false, true, &t),
            Role::Start(TagKind::Spec, "a")
        );
        assert_eq!(r.next(" b", false, true, &t), Role::Continue("b"));
        assert_eq!(r.next("", false, true, &t), Role::Other);
        assert_eq!(r.next(" c", false, true, &t), Role::Other);
        // A trailing one is whole.
        assert_eq!(
            r.next(" @req X", false, false, &t),
            Role::Start(TagKind::Requirement, "X")
        );
        assert_eq!(r.next(" d", false, true, &t), Role::Other);
        // Blank lines in a fence continue it.
        assert_eq!(
            r.next(" @spec", false, true, &t),
            Role::Start(TagKind::Spec, "")
        );
        assert_eq!(
            r.next(" ```wavedrom", false, true, &t),
            Role::Continue("```wavedrom")
        );
        assert_eq!(r.next("", false, true, &t), Role::Continue(""));
        assert_eq!(r.next(" ```", false, true, &t), Role::Continue("```"));
        assert_eq!(r.next("", false, true, &t), Role::Other);
    }
}
