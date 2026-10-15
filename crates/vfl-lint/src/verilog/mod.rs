//! Lint rules for Verilog and SystemVerilog files.
//!
//! The rules read the syntax tree of one file, and what the other Verilog
//! files of the project declare: packages, compilation-unit declarations,
//! included files and modules with their ports (a [`Project`]). Rules that
//! mean the same in both languages share the VHDL rules' ids
//! (`sema/inferred-latch`, `style/positional-association`); the others
//! are `verilog/*`.

mod fsm;
mod instances;
mod names;
mod procedures;
mod scope;
mod width;

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use vfl_sema::{Analysis, ForeignModule};
use vfl_verilog_syntax::{SyntaxKind as K, SyntaxNode, SyntaxToken, TextRange};

use crate::Finding;
use crate::naming::Convention;
use scope::{Access, Scope, Use, is_unit, name_token};

/// What a file declares for other files to see, and what it uses of them.
#[derive(Debug, Default, Hash)]
pub struct Exports {
    /// Its modules, interfaces, programs and packages.
    units: Vec<String>,
    /// Its packages, with the names each declares.
    packages: Vec<(String, Vec<String>)>,
    /// What it declares outside units, in the compilation unit.
    globals: Vec<String>,
    /// Every name it declares, for files that include it.
    names: Vec<String>,
    /// The packages it imports.
    imports: Vec<String>,
    /// The modules it instantiates.
    instantiated: Vec<String>,
    /// The files it includes.
    includes: Vec<String>,
    /// Its units with a macro as an item, which may declare ports and
    /// parameters: `` `NESTED_TEST_SUITE ``.
    open: Vec<String>,
}

impl Exports {
    pub fn new(root: &SyntaxNode, includes: &[String]) -> Exports {
        let mut out = Exports {
            includes: includes.to_vec(),
            ..Exports::default()
        };
        let file = Scope::collect(root);
        out.globals = file.decls.iter().map(|d| d.name.clone()).collect();
        out.names.extend(out.globals.iter().cloned());
        out.imports.extend(file.imports.iter().cloned());
        for unit in root.descendants().filter(|n| is_unit(n.kind())) {
            let Some(name) = name_token(&unit) else {
                continue;
            };
            let name = scope::ident(&name);
            let scope = Scope::collect(&unit);
            let names: Vec<String> = scope.decls.iter().map(|d| d.name.clone()).collect();
            out.names.extend(names.iter().cloned());
            out.imports.extend(scope.imports.iter().cloned());
            if unit.kind() == K::PACKAGE_DECL {
                out.packages.push((name.clone(), names));
            }
            if scope.open {
                out.open.push(name.clone());
            }
            out.units.push(name);
        }
        for instantiation in root.descendants().filter(|n| n.kind() == K::INSTANTIATION) {
            if let Some(module) = instantiation
                .first_child()
                .filter(|n| n.kind() == K::NAME_REF)
                .and_then(|n| scope::ref_name(&n))
            {
                out.instantiated.push(scope::ident(&module));
            }
        }
        for list in [&mut out.imports, &mut out.instantiated, &mut out.names] {
            list.sort();
            list.dedup();
        }
        out
    }
}

/// What the Verilog files of a project declare, for the rules of each.
pub struct Project<'a> {
    files: Vec<(PathBuf, Arc<Exports>)>,
    packages: FxHashMap<String, Vec<String>>,
    globals: FxHashSet<String>,
    units: FxHashSet<String>,
    modules: FxHashMap<&'a str, Option<&'a ForeignModule>>,
    /// A hash of what every file sees: the globals and the units.
    shared: u64,
}

impl<'a> Project<'a> {
    pub fn new(analysis: &'a Analysis, files: Vec<(PathBuf, Arc<Exports>)>) -> Project<'a> {
        let mut packages: FxHashMap<String, Vec<String>> = FxHashMap::default();
        let mut globals = FxHashSet::default();
        let mut units = FxHashSet::default();
        let mut open = FxHashSet::default();
        for (_, exports) in &files {
            open.extend(exports.open.iter().map(String::as_str));
            for (name, names) in &exports.packages {
                packages
                    .entry(name.clone())
                    .or_default()
                    .extend(names.iter().cloned());
            }
            globals.extend(exports.globals.iter().cloned());
            units.extend(exports.units.iter().cloned());
        }
        // A module defined twice (two test benches), or whose macros may
        // declare more ports and parameters, is not looked up.
        let mut modules: FxHashMap<&str, Option<&ForeignModule>> = FxHashMap::default();
        for id in 0..analysis.files().len() {
            for module in analysis.foreign_modules(id as u32).unwrap_or(&[]) {
                let known = !open.contains(module.name.as_str());
                modules
                    .entry(module.name.as_str())
                    .and_modify(|m| *m = None)
                    .or_insert(known.then_some(module));
            }
        }
        let mut sorted_globals: Vec<&String> = globals.iter().collect();
        sorted_globals.sort();
        let mut sorted_units: Vec<&String> = units.iter().collect();
        sorted_units.sort();
        let mut hasher = DefaultHasher::new();
        sorted_globals.hash(&mut hasher);
        sorted_units.hash(&mut hasher);
        Project {
            files,
            packages,
            globals,
            units,
            modules,
            shared: hasher.finish(),
        }
    }

    /// The file `name` (as written after `` `include ``) names.
    fn include(&self, name: &str) -> Option<&Exports> {
        let name = name.trim_matches(['"', '<', '>']);
        if name.is_empty() {
            return None;
        }
        let wanted = Path::new(name);
        self.files
            .iter()
            .find(|(path, _)| path.ends_with(wanted))
            .map(|(_, e)| &**e)
    }

    pub fn module(&self, name: &str) -> Option<&'a ForeignModule> {
        self.modules.get(name).copied().flatten()
    }

    /// A hash of what a file's findings depend on beyond its own text.
    pub fn dependencies(&self, exports: &Exports) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.shared.hash(&mut hasher);
        for package in &exports.imports {
            self.packages.get(package).hash(&mut hasher);
        }
        for include in &exports.includes {
            self.include(include).map(|e| &e.names).hash(&mut hasher);
        }
        for module in &exports.instantiated {
            self.module(module).hash(&mut hasher);
        }
        hasher.finish()
    }
}

/// What the rules need to know about one file.
pub struct Context<'a> {
    pub text: &'a str,
    pub root: &'a SyntaxNode,
    pub path: &'a Path,
    pub includes: &'a [String],
    pub project: &'a Project<'a>,
    pub max_line_length: usize,
    pub indent: usize,
    pub conventions: &'a [(&'static str, Convention)],
    /// Whether the file parses: the rules that need its meaning are
    /// skipped while it does not.
    pub parses: bool,
}

impl Context<'_> {
    fn line(&self, offset: rowan::TextSize) -> usize {
        self.text[..usize::from(offset)].matches('\n').count() + 1
    }
}

/// A unit being checked, with what the rules share about it.
pub struct Unit<'a> {
    pub ctx: &'a Context<'a>,
    pub node: SyntaxNode,
    pub scope: Scope,
    pub uses: Vec<Use>,
    /// Names nothing declares, used where they declare an implicit net.
    pub implicit: FxHashSet<String>,
}

impl Unit<'_> {
    /// The declaration of a name used in the unit, if it is one of its
    /// own.
    pub fn decl(&self, name: &str) -> Option<&scope::Decl> {
        self.scope.lookup(name)
    }
}

pub fn check(ctx: &Context, enabled: impl Fn(&str) -> bool, out: &mut Vec<Finding>) {
    if enabled("style/line-length") || enabled("style/trailing-whitespace") || enabled("style/tabs")
    {
        crate::style::lines(ctx.text, ctx.max_line_length, ctx.indent, &enabled, out);
    }
    let units: Vec<SyntaxNode> = ctx
        .root
        .descendants()
        .filter(|n| is_unit(n.kind()))
        .collect();
    if enabled("style/file-name") {
        file_name(ctx, &units, out);
    }
    if enabled("verilog/default-nettype") {
        default_nettype(ctx, &units, out);
    }
    for node in units {
        if enabled("style/process-label") {
            process_labels(&node, out);
        }
        if !ctx.parses {
            continue;
        }
        let scope = Scope::collect(&node);
        let port_access = |connection: &SyntaxNode| instances::port_access(ctx.project, connection);
        let uses = scope::uses(&node, &port_access);
        let mut unit = Unit {
            ctx,
            node,
            scope,
            uses,
            implicit: FxHashSet::default(),
        };
        names::check(&mut unit, &enabled, out);
        procedures::check(&unit, &enabled, out);
        fsm::check(&unit, &enabled, out);
        width::check(&unit, &enabled, out);
        instances::check(&unit, &enabled, out);
    }
}

/// The non-trivia tokens of `node` and its descendants.
fn all_tokens(node: &SyntaxNode) -> impl Iterator<Item = SyntaxToken> + '_ {
    node.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
}

fn file_name(ctx: &Context, units: &[SyntaxNode], out: &mut Vec<Finding>) {
    let Some(stem) = ctx.path.file_stem().and_then(|s| s.to_str()) else {
        return;
    };
    let names: Vec<(String, TextRange)> = units
        .iter()
        .filter(|u| u.parent().is_some_and(|p| p.kind() == K::SOURCE_FILE))
        .filter_map(name_token)
        .map(|t| (scope::ident(&t), t.text_range()))
        .collect();
    let Some((first, range)) = names.first() else {
        return;
    };
    if names.iter().all(|(name, _)| name != stem) {
        let extension = ctx
            .path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("sv");
        out.push(Finding::new(
            "style/file-name",
            *range,
            format!(
                "file name does not match the unit it declares; expected `{first}.{extension}`"
            ),
        ));
    }
}

/// The `` `default_nettype `` in effect at each offset: `(offset, none)`
/// for each directive that sets it.
fn nettype_directives(root: &SyntaxNode) -> Vec<(rowan::TextSize, bool)> {
    root.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == K::DIRECTIVE)
        .filter_map(|t| {
            let mut words = t.text().split_whitespace();
            match words.next()? {
                "`default_nettype" => Some((t.text_range().start(), words.next() == Some("none"))),
                "`resetall" => Some((t.text_range().start(), false)),
                _ => None,
            }
        })
        .collect()
}

/// Whether `` `default_nettype none `` is in effect at `offset`.
fn nettype_none(directives: &[(rowan::TextSize, bool)], offset: rowan::TextSize) -> bool {
    directives
        .iter()
        .take_while(|(at, _)| *at < offset)
        .last()
        .is_some_and(|(_, none)| *none)
}

fn default_nettype(ctx: &Context, units: &[SyntaxNode], out: &mut Vec<Finding>) {
    let directives = nettype_directives(ctx.root);
    for unit in units.iter().filter(|u| u.kind() == K::MODULE_DECL) {
        if nettype_none(&directives, unit.text_range().start()) {
            continue;
        }
        let Some(keyword) = all_tokens(unit).next() else {
            continue;
        };
        out.push(Finding::new(
            "verilog/default-nettype",
            keyword.text_range(),
            "`` `default_nettype none `` is not in effect here, so a name nothing declares becomes an implicit wire",
        ));
    }
}

/// The body of a procedure: the statement after its keyword and its
/// event control, if any.
fn procedure_body(procedure: &SyntaxNode) -> Option<SyntaxNode> {
    let mut statement = procedure.children().next()?;
    while statement.kind() == K::TIMED_STMT {
        statement = statement
            .children()
            .find(|n| !matches!(n.kind(), K::EVENT_CONTROL | K::DELAY))?;
    }
    Some(statement)
}

/// The label of a block: `begin : name`.
fn block_label(block: &SyntaxNode) -> Option<SyntaxToken> {
    if block.kind() != K::BLOCK_STMT {
        return None;
    }
    let label = block
        .children()
        .find(|n| n.kind() == K::END_LABEL)
        .filter(|l| {
            l.prev_sibling_or_token()
                .into_iter()
                .flat_map(|e| std::iter::successors(Some(e), |e| e.prev_sibling_or_token()))
                .find(|e| !e.kind().is_trivia())
                .is_some_and(|e| e.kind() == K::BEGIN_KW)
        })?;
    scope::tokens(&label).find(|t| scope::is_ident(t.kind()))
}

fn process_labels(unit: &SyntaxNode, out: &mut Vec<Finding>) {
    for procedure in unit.descendants().filter(|n| n.kind() == K::PROCEDURE) {
        let Some(keyword) = scope::tokens(&procedure).next() else {
            continue;
        };
        if !matches!(
            keyword.kind(),
            K::ALWAYS_KW | K::ALWAYS_COMB_KW | K::ALWAYS_FF_KW | K::ALWAYS_LATCH_KW
        ) {
            continue;
        }
        let labeled = procedure_body(&procedure).is_some_and(|b| block_label(&b).is_some());
        if !labeled {
            out.push(Finding::new(
                "style/process-label",
                keyword.text_range(),
                format!("`{}` block without a label", keyword.text()),
            ));
        }
    }
}

/// The ranges of a node's first and last non-trivia tokens, joined.
fn trimmed_range(node: &SyntaxNode) -> TextRange {
    let mut tokens = all_tokens(node);
    let Some(first) = tokens.next() else {
        return node.text_range();
    };
    let last = tokens.last().unwrap_or_else(|| first.clone());
    first.text_range().cover(last.text_range())
}

/// The uses within `range` that read.
fn reads_in(uses: &[Use], range: TextRange) -> impl Iterator<Item = &Use> {
    let start = uses.partition_point(|u| u.range.start() < range.start());
    uses[start..]
        .iter()
        .take_while(move |u| u.range.end() <= range.end())
        .filter(|u| u.access != Access::Write)
}
