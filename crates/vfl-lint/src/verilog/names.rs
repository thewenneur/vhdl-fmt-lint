//! Declarations and uses: names nothing declares, implicit nets, names
//! declared twice, signals never used, read or driven, and naming
//! conventions.

use std::collections::{HashMap, HashSet};

use vfl_sema::{Edit, Fix, codes};
use vfl_verilog_syntax::summary::Direction;
use vfl_verilog_syntax::{SyntaxKind as K, SyntaxNode, TextRange, TextSize};

use super::scope::{self, Access, Decl, Kind, is_unit};
use super::{Unit, instances, nettype_directives, nettype_none};
use crate::Finding;
use crate::naming::Convention;

pub fn check(unit: &mut Unit, enabled: &impl Fn(&str) -> bool, out: &mut Vec<Finding>) {
    implicit_nets(unit, enabled, out);
    if enabled(codes::UNRESOLVED_NAME) {
        unresolved(unit, out);
    }
    if enabled(codes::DUPLICATE_DECLARATION) {
        duplicates(unit, out);
    }
    usage(unit, enabled, out);
    if !unit.ctx.conventions.is_empty() {
        naming(unit, out);
    }
}

/// Whether the unit sees a declaration of `name` beyond its own: from the
/// compilation unit, a package, an included file or another unit.
struct Visible<'a> {
    unit: &'a Unit<'a>,
    file: HashSet<String>,
    /// Some names may come from where the rules cannot see.
    open: bool,
}

impl<'a> Visible<'a> {
    fn new(unit: &'a Unit<'a>) -> Visible<'a> {
        let project = unit.ctx.project;
        let mut file = HashSet::new();
        let mut open = unit.scope.open;
        let file_scope = scope::Scope::collect(unit.ctx.root);
        file.extend(file_scope.decls.iter().map(|d| d.name.clone()));
        file.extend(file_scope.hidden.iter().cloned());
        for package in file_scope.imports.iter().chain(&unit.scope.imports) {
            match project.packages.get(package) {
                Some(names) => file.extend(names.iter().cloned()),
                None => open = true,
            }
        }
        for include in unit.ctx.includes {
            match project.include(include) {
                Some(exports) => file.extend(exports.names.iter().cloned()),
                None => open = true,
            }
        }
        Visible { unit, file, open }
    }

    fn declared(&self, name: &str) -> bool {
        let project = self.unit.ctx.project;
        self.unit.scope.by_name.contains_key(name)
            || self.unit.scope.hidden.contains(name)
            || self.unit.implicit.contains(name)
            || self.file.contains(name)
            || project.globals.contains(name)
            || project.units.contains(name)
    }
}

/// Names nothing declares where they declare an implicit net: connected
/// to a port, or the target of a continuous assignment.
fn implicit_nets(unit: &mut Unit, enabled: &impl Fn(&str) -> bool, out: &mut Vec<Finding>) {
    let directives = nettype_directives(unit.ctx.root);
    if nettype_none(&directives, unit.node.text_range().start()) {
        return;
    }
    let mut found: Vec<(String, TextRange)> = Vec::new();
    {
        let visible = Visible::new(unit);
        if visible.open {
            return;
        }
        for u in &unit.uses {
            if !u.checked || visible.declared(&u.name) || found.iter().any(|(n, _)| *n == u.name) {
                continue;
            }
            let Some(parent) = u.node.parent() else {
                continue;
            };
            let declares = match parent.kind() {
                K::CONNECTION => parent.parent().is_some_and(|p| p.kind() == K::INSTANCE),
                K::ASSIGN_EXPR => {
                    parent.first_child().as_ref() == Some(&u.node)
                        && parent
                            .parent()
                            .is_some_and(|p| p.kind() == K::CONTINUOUS_ASSIGN)
                }
                _ => false,
            };
            if declares {
                found.push((u.name.clone(), u.range));
            }
        }
    }
    for (name, range) in found {
        if enabled("verilog/implicit-net") {
            out.push(Finding::new(
                "verilog/implicit-net",
                range,
                format!("`{name}` is not declared, so it is an implicit 1-bit wire"),
            ));
        }
        unit.implicit.insert(name);
    }
}

fn unresolved(unit: &Unit, out: &mut Vec<Finding>) {
    let visible = Visible::new(unit);
    if visible.open {
        return;
    }
    for u in &unit.uses {
        // A SystemVerilog keyword in a Verilog file (`assert` for a tool
        // that knows it) is not a name the file forgot to declare.
        let keyword = K::keyword(&u.name, vfl_verilog_syntax::Standard::Sv2023).is_some();
        // `a.find(x) with (item > 0)`: the iterator an array method names.
        let iterator = u.name == "item" && u.node.ancestors().any(|a| a.kind() == K::WITH_CLAUSE);
        if u.checked && !keyword && !iterator && !visible.declared(&u.name) {
            out.push(Finding::new(
                codes::UNRESOLVED_NAME,
                u.range,
                format!("`{}` is not declared", u.name),
            ));
        }
    }
}

fn duplicates(unit: &Unit, out: &mut Vec<Finding>) {
    let mut groups: HashMap<(&SyntaxNode, &str), Vec<&Decl>> = HashMap::new();
    for decl in &unit.scope.decls {
        if matches!(
            decl.kind,
            Kind::Label | Kind::Other | Kind::Import | Kind::HeaderPort
        ) {
            continue;
        }
        groups
            .entry((&decl.region, decl.name.as_str()))
            .or_default()
            .push(decl);
    }
    let mut found: Vec<(&Decl, &Decl)> = Vec::new();
    for decls in groups.values_mut().filter(|d| d.len() > 1) {
        decls.sort_by_key(|d| d.range.start());
        // `output q; reg q;`: a port declared again with its type.
        let port_decl =
            |d: &Decl| matches!(d.kind, Kind::Port(_)) && d.node.kind() == K::DECLARATOR;
        let data = |d: &Decl| matches!(d.kind, Kind::Net | Kind::Var);
        let redeclared =
            (port_decl(decls[0]) && data(decls[1])) || (data(decls[0]) && port_decl(decls[1]));
        let skip = if redeclared { 2 } else { 1 };
        for decl in &decls[skip..] {
            if !exclusive(&decls[0].node, &decl.node) {
                found.push((decls[0], decl));
            }
        }
    }
    found.sort_by_key(|(_, d)| d.range.start());
    for (first, decl) in found {
        out.push(Finding::new(
            codes::DUPLICATE_DECLARATION,
            decl.range,
            format!(
                "`{}` is already declared at line {}",
                decl.name,
                unit.ctx.line(first.range.start())
            ),
        ));
    }
}

/// Whether two nodes are in different alternatives of an `if` or `case`
/// generate, which never coexist: `if (A) m u (); else n u ();`.
fn exclusive(a: &SyntaxNode, b: &SyntaxNode) -> bool {
    a.ancestors()
        .filter(|g| matches!(g.kind(), K::GENERATE_IF | K::GENERATE_CASE))
        .any(|g| {
            let branch = |n: &SyntaxNode| {
                n.ancestors()
                    .take_while(|x| *x != g)
                    .last()
                    .filter(|x| x.parent().as_ref() == Some(&g))
            };
            g.text_range().contains_range(b.text_range())
                && matches!((branch(a), branch(b)), (Some(x), Some(y)) if x != y)
        })
}

#[derive(Default, Clone, Copy)]
struct Usage {
    reads: bool,
    writes: bool,
}

fn usage(unit: &Unit, enabled: &impl Fn(&str) -> bool, out: &mut Vec<Finding>) {
    let rules = [
        codes::UNUSED_SIGNAL,
        codes::UNREAD_SIGNAL,
        codes::UNDRIVEN_SIGNAL,
        codes::UNUSED_PORT,
        codes::UNUSED_GENERIC,
    ];
    if !rules.iter().any(|r| enabled(r))
        || matches!(unit.node.kind(), K::INTERFACE_DECL | K::PACKAGE_DECL)
    {
        return;
    }
    // Code included into the unit may use anything.
    let includes = unit
        .node
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .any(|t| t.kind() == K::DIRECTIVE && t.text().starts_with("`include"));
    if includes {
        return;
    }
    let mut usage: HashMap<&str, Usage> = HashMap::new();
    let mut all = false;
    for u in &unit.uses {
        let entry = usage.entry(u.name.as_str()).or_default();
        entry.reads |= u.access.reads();
        entry.writes |= u.access.writes();
    }
    for name in &unit.scope.hidden {
        usage.insert(
            name.as_str(),
            Usage {
                reads: true,
                writes: true,
            },
        );
    }
    // `.*` and `.name` connect ports to signals of the same name.
    for connection in unit
        .node
        .descendants()
        .filter(|n| n.kind() == K::CONNECTION)
    {
        if scope::has_token(&connection, K::DOT_STAR) {
            match instances::wildcard_ports(unit.ctx.project, &connection) {
                Some(ports) => {
                    for (name, access) in ports {
                        if let Some(decl) = unit.decl(name) {
                            let entry = usage.entry(decl.name.as_str()).or_default();
                            entry.reads |= access.reads();
                            entry.writes |= access.writes();
                        }
                    }
                }
                None => all = true,
            }
        } else if scope::has_token(&connection, K::DOT)
            && !scope::has_token(&connection, K::LPAREN)
            && let Some(name) = scope::name_token(&connection)
        {
            let access =
                instances::port_access(unit.ctx.project, &connection).unwrap_or(Access::Both);
            if let Some(decl) = unit.decl(&scope::ident(&name)) {
                let entry = usage.entry(decl.name.as_str()).or_default();
                entry.reads |= access.reads();
                entry.writes |= access.writes();
            }
        }
    }
    if all {
        return;
    }
    let names: HashSet<&str> = unit.scope.decls.iter().map(|d| d.name.as_str()).collect();
    let mut reported: HashSet<&str> = HashSet::new();
    for decl in &unit.scope.decls {
        // `unused_x = x;` marks a signal left unused on purpose, as
        // Verilator's `--unused-regexp` does.
        if decl.region != unit.node
            || !names.contains(decl.name.as_str())
            || decl.name.to_ascii_lowercase().contains("unused")
        {
            continue;
        }
        let used = usage.get(decl.name.as_str()).copied().unwrap_or_default();
        let port = unit.scope.named(&decl.name).find_map(|d| match d.kind {
            Kind::Port(dir) => Some(dir),
            _ => None,
        });
        let initialized = unit.scope.named(&decl.name).any(|d| d.init);
        let finding = match (decl.kind, port) {
            (Kind::Net | Kind::Var, None) => {
                // A net with a constant value, or a pulled one.
                let constant = matches!(
                    decl.net_keyword(),
                    Some(K::SUPPLY0_KW | K::SUPPLY1_KW | K::TRI0_KW | K::TRI1_KW)
                );
                if !used.reads && !used.writes && !initialized {
                    let fix = removal(unit, decl);
                    Some((
                        codes::UNUSED_SIGNAL,
                        format!("signal `{}` is never used", decl.name),
                        fix,
                    ))
                } else if !used.reads {
                    Some((
                        codes::UNREAD_SIGNAL,
                        format!("signal `{}` is assigned but never read", decl.name),
                        None,
                    ))
                } else if !used.writes && !initialized && !constant {
                    Some((
                        codes::UNDRIVEN_SIGNAL,
                        format!("signal `{}` is read but never assigned", decl.name),
                        None,
                    ))
                } else {
                    None
                }
            }
            (Kind::Port(Some(Direction::Output)), _) if !used.writes && !initialized => Some((
                codes::UNDRIVEN_SIGNAL,
                format!("output port `{}` is never assigned", decl.name),
                None,
            )),
            (Kind::Port(Some(Direction::Input)), _) if !used.reads => Some((
                codes::UNUSED_PORT,
                format!("port `{}` is never used", decl.name),
                None,
            )),
            (Kind::Param, _) if !used.reads => Some((
                codes::UNUSED_GENERIC,
                format!("parameter `{}` is never used", decl.name),
                None,
            )),
            _ => None,
        };
        if let Some((rule, message, fix)) = finding
            && enabled(rule)
            && reported.insert(decl.name.as_str())
        {
            let mut finding = Finding::new(rule, decl.range, message);
            finding.fix = fix;
            out.push(finding);
        }
    }
}

/// Removing the declaration of an unused signal, when it declares nothing
/// else: the whole line, if nothing else is on it.
fn removal(unit: &Unit, decl: &Decl) -> Option<Fix> {
    let declaration = decl.node.parent()?;
    if !matches!(declaration.kind(), K::NET_DECL | K::DATA_DECL)
        || declaration
            .children()
            .filter(|n| n.kind() == K::DECLARATOR)
            .count()
            != 1
    {
        return None;
    }
    let text = unit.ctx.text;
    let range = super::trimmed_range(&declaration);
    let start = usize::from(range.start());
    let end = usize::from(range.end());
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i + 1);
    let alone = text[line_start..start].trim().is_empty() && text[end..line_end].trim().is_empty();
    let removed = if alone {
        TextRange::new(
            TextSize::from(line_start as u32),
            TextSize::from(line_end as u32),
        )
    } else {
        range
    };
    Some(Fix {
        title: format!("Remove `{}`", decl.name),
        edits: vec![Edit {
            range: removed,
            text: String::new(),
        }],
        safe: false,
    })
}

/// The `naming/<kind>` rules that apply to a declaration, most general
/// first.
fn kinds(unit: &Unit, decl: &Decl) -> Vec<&'static str> {
    match decl.kind {
        Kind::Port(dir) => match dir {
            Some(Direction::Input) => vec!["port", "input-port"],
            Some(Direction::Output) => vec!["port", "output-port"],
            Some(Direction::Inout) => vec!["port", "inout-port"],
            _ => vec!["port"],
        },
        Kind::Param => vec!["parameter"],
        Kind::LocalParam => vec!["constant"],
        Kind::Net | Kind::Var => {
            // `output q; reg q;`: the port's rules apply.
            if unit
                .scope
                .named(&decl.name)
                .any(|d| matches!(d.kind, Kind::Port(_)))
            {
                vec![]
            } else if decl.is_static() {
                vec!["signal"]
            } else {
                vec!["variable"]
            }
        }
        Kind::Type => vec!["type"],
        Kind::Function => vec!["function"],
        Kind::Task => vec!["task"],
        Kind::Instance => vec!["instance"],
        Kind::Label => match decl.node.parent() {
            Some(b) if b.kind() == K::GENERATE_BLOCK => vec!["generate"],
            Some(b)
                if b.kind() == K::BLOCK_STMT
                    && b.ancestors()
                        .find(|a| a.kind() == K::PROCEDURE)
                        .and_then(|p| super::procedure_body(&p))
                        .as_ref()
                        == Some(&b) =>
            {
                vec!["process"]
            }
            _ => vec![],
        },
        _ => vec![],
    }
}

fn naming(unit: &Unit, out: &mut Vec<Finding>) {
    let conventions = unit.ctx.conventions;
    let check =
        |kinds: &[&str], name: &str, range: TextRange, what: &str, out: &mut Vec<Finding>| {
            let applying: Vec<(&str, &Convention)> = kinds
                .iter()
                .filter_map(|kind| {
                    conventions
                        .iter()
                        .find(|(k, _)| k == kind)
                        .map(|(k, c)| (*k, c))
                })
                .collect();
            for (kind, convention) in &applying {
                if let Some(problem) = convention.check(name) {
                    out.push(Finding::new(
                        crate::naming::leak_id(kind),
                        range,
                        format!("{what} `{name}` {problem}"),
                    ));
                    break;
                }
            }
        };
    if let Some(name) = scope::name_token(&unit.node) {
        let (kind, what) = match unit.node.kind() {
            K::PACKAGE_DECL => ("package", "package"),
            K::INTERFACE_DECL => ("module", "interface"),
            K::PROGRAM_DECL => ("module", "program"),
            _ => ("module", "module"),
        };
        check(&[kind], &scope::ident(&name), name.text_range(), what, out);
    }
    for decl in &unit.scope.decls {
        // Names of nested units are checked with them.
        if decl.kind == Kind::Other || is_unit(decl.node.kind()) {
            continue;
        }
        let kinds = kinds(unit, decl);
        if kinds.is_empty() {
            continue;
        }
        let what = match decl.kind {
            Kind::Port(_) => "port",
            Kind::Param => "parameter",
            Kind::LocalParam => "constant",
            Kind::Net | Kind::Var if kinds[0] == "signal" => "signal",
            Kind::Net | Kind::Var => "variable",
            Kind::Type => "type",
            Kind::Function => "function",
            Kind::Task => "task",
            Kind::Instance => "instance",
            _ => "label",
        };
        check(&kinds, &decl.name, decl.range, what, out);
    }
}
