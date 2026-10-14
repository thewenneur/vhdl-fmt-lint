//! Instances of the project's modules: connections by position, ports and
//! parameters that do not exist, inputs left unconnected, and widths that
//! differ from the port's.

use vfl_sema::{Edit, Fix, ForeignModule, Mode, codes};
use vfl_verilog_syntax::{SyntaxKind as K, SyntaxNode, TextRange};

use super::scope::{self, Access, has_token, ident};
use super::{Project, Unit, width};
use crate::Finding;

/// A connection of an instance or of a parameter value list.
struct Connection {
    node: SyntaxNode,
    /// `.name(...)` or `.name`; `None` by position.
    name: Option<String>,
    /// The connected expression; `None` for `.name()` or an empty
    /// position.
    expr: Option<SyntaxNode>,
}

fn connections(list: &SyntaxNode) -> Vec<Connection> {
    list.children()
        .filter(|n| n.kind() == K::CONNECTION && !has_token(n, K::DOT_STAR))
        .map(|node| {
            let name = has_token(&node, K::DOT)
                .then(|| scope::name_token(&node))
                .flatten()
                .map(|t| ident(&t));
            let expr = node.children().find(|n| n.kind() != K::NAME);
            Connection { node, name, expr }
        })
        .collect()
}

/// The module an instance or connection instantiates, through its
/// `INSTANTIATION`.
fn module_of<'a>(project: &Project<'a>, node: &SyntaxNode) -> Option<&'a ForeignModule> {
    let instantiation = node.ancestors().find(|n| n.kind() == K::INSTANTIATION)?;
    let module = instantiation
        .first_child()
        .filter(|n| n.kind() == K::NAME_REF && scope::is_simple_ref(n))?;
    project.module(&ident(&scope::ref_name(&module)?))
}

fn access(mode: Option<Mode>) -> Access {
    match mode {
        Some(Mode::In) => Access::Read,
        Some(Mode::Out | Mode::Buffer) => Access::Write,
        _ => Access::Both,
    }
}

/// How a port connection uses what it connects: read for an input, written
/// for an output. `None` when that is not known.
pub fn port_access(project: &Project, connection: &SyntaxNode) -> Option<Access> {
    let list = connection.parent()?;
    if list.kind() == K::PARAM_VALUES {
        return Some(Access::Read);
    }
    let instantiation = list.parent()?;
    // A gate: `and g (y, a, b)` drives its first terminal.
    let gate = scope::tokens(&instantiation)
        .next()
        .is_some_and(|t| t.kind().is_keyword());
    let position = list
        .children()
        .filter(|n| n.kind() == K::CONNECTION)
        .position(|n| n == *connection)?;
    if gate {
        return Some(if position == 0 {
            Access::Write
        } else {
            Access::Read
        });
    }
    let module = module_of(project, connection)?;
    let port = match scope::name_token(connection).filter(|_| has_token(connection, K::DOT)) {
        Some(name) => {
            let name = ident(&name);
            module.ports.iter().find(|p| p.name == name)?
        }
        None => module.ports.get(position)?,
    };
    Some(access(port.mode))
}

/// The ports `.*` connects, with how each is used; `None` when the module
/// is not known.
pub fn wildcard_ports<'a>(
    project: &Project<'a>,
    connection: &SyntaxNode,
) -> Option<Vec<(&'a str, Access)>> {
    let module = module_of(project, connection)?;
    Some(
        module
            .ports
            .iter()
            .map(|p| (p.name.as_str(), access(p.mode)))
            .collect(),
    )
}

/// An input left unconnected floats; Verilog allows it, so it has its own
/// rule rather than VHDL's `sema/unassociated-port` and `sema/open-input`.
const UNCONNECTED: &str = "verilog/unconnected-input";

pub fn check(unit: &Unit, enabled: &impl Fn(&str) -> bool, out: &mut Vec<Finding>) {
    let project = unit.ctx.project;
    for instantiation in unit
        .node
        .descendants()
        .filter(|n| n.kind() == K::INSTANTIATION)
    {
        // Gates and primitives are positional by definition.
        let Some(type_ref) = instantiation
            .first_child()
            .filter(|n| n.kind() == K::NAME_REF && scope::is_simple_ref(n))
        else {
            continue;
        };
        let module = scope::ref_name(&type_ref).and_then(|t| project.module(&ident(&t)));
        let lists = instantiation
            .children()
            .filter(|n| n.kind() == K::PARAM_VALUES || n.kind() == K::INSTANCE);
        for list in lists {
            let parameters = list.kind() == K::PARAM_VALUES;
            let conns = connections(&list);
            let wildcard = list
                .children()
                .any(|n| n.kind() == K::CONNECTION && has_token(&n, K::DOT_STAR));
            if enabled("style/positional-association") {
                positional(&list, &conns, module, parameters, out);
            }
            let Some(module) = module else {
                continue;
            };
            if parameters {
                if enabled(codes::UNRESOLVED_NAME) {
                    for c in &conns {
                        if let Some(name) = &c.name
                            && !module.generics.iter().any(|g| g.name == *name)
                        {
                            out.push(Finding::new(
                                codes::UNRESOLVED_NAME,
                                name_range(c),
                                format!("`{}` has no parameter `{name}`", module.name),
                            ));
                        }
                    }
                }
                continue;
            }
            ports(unit, &list, &conns, module, wildcard, enabled, out);
        }
    }
}

fn name_range(c: &Connection) -> TextRange {
    scope::name_token(&c.node).map_or(c.node.text_range(), |t| t.text_range())
}

fn ports(
    unit: &Unit,
    instance: &SyntaxNode,
    conns: &[Connection],
    module: &ForeignModule,
    wildcard: bool,
    enabled: &impl Fn(&str) -> bool,
    out: &mut Vec<Finding>,
) {
    let by_position = conns.iter().all(|c| c.name.is_none());
    // An instance with no connections at all: `sub u();` connects nothing
    // on purpose or is an interface.
    if conns.is_empty() && !wildcard {
        return;
    }
    let mut connected = vec![false; module.ports.len()];
    for (i, c) in conns.iter().enumerate() {
        let port = match &c.name {
            Some(name) => module.ports.iter().position(|p| p.name == *name),
            None if by_position => (i < module.ports.len()).then_some(i),
            None => None,
        };
        let Some(index) = port else {
            if let Some(name) = &c.name
                && enabled(codes::UNRESOLVED_NAME)
            {
                out.push(Finding::new(
                    codes::UNRESOLVED_NAME,
                    name_range(c),
                    format!("`{}` has no port `{name}`", module.name),
                ));
            }
            continue;
        };
        let port = &module.ports[index];
        connected[index] = true;
        let input = port.mode == Some(Mode::In);
        match &c.expr {
            None if c.name.is_some() && has_token(&c.node, K::LPAREN) => {
                if input && !port.has_default && enabled(UNCONNECTED) {
                    out.push(Finding::new(
                        UNCONNECTED,
                        c.node.text_range(),
                        format!(
                            "input port `{}` of `{}` is connected to nothing",
                            port.name, module.name
                        ),
                    ));
                }
            }
            Some(expr) if enabled(codes::WIDTH_MISMATCH) => {
                let (Some(port_width), Some((value, arithmetic))) =
                    (port.width, width::value_width(unit, expr))
                else {
                    continue;
                };
                let port_width = width::Linear::from(port_width);
                // An input takes the value; an output gives it.
                let found = if port.mode == Some(Mode::Out) {
                    width::mismatch(&value, &port_width, false).map(|(v, p)| (p, v))
                } else {
                    width::mismatch(&port_width, &value, arithmetic)
                };
                if let Some((p, v)) = found {
                    out.push(Finding::new(
                        codes::WIDTH_MISMATCH,
                        c.node.text_range(),
                        format!(
                            "port `{}` of `{}` has {p} bits but is connected to {v}",
                            port.name, module.name
                        ),
                    ));
                }
            }
            _ => {}
        }
    }
    if wildcard || !enabled(UNCONNECTED) {
        return;
    }
    let missing: Vec<&str> = module
        .ports
        .iter()
        .zip(&connected)
        .filter(|(p, c)| !**c && p.mode == Some(Mode::In) && !p.has_default)
        .map(|(p, _)| p.name.as_str())
        .collect();
    if missing.is_empty() {
        return;
    }
    let range = scope::name_token(instance).map_or(instance.text_range(), |t| t.text_range());
    let quoted: Vec<String> = missing.iter().map(|n| format!("`{n}`")).collect();
    out.push(Finding::new(
        UNCONNECTED,
        range,
        format!(
            "input {} {} of `{}` {} not connected",
            if missing.len() == 1 { "port" } else { "ports" },
            quoted.join(", "),
            module.name,
            if missing.len() == 1 { "is" } else { "are" }
        ),
    ));
}

/// Connections by position, with a fix naming them from the module.
fn positional(
    list: &SyntaxNode,
    conns: &[Connection],
    module: Option<&ForeignModule>,
    parameters: bool,
    out: &mut Vec<Finding>,
) {
    if !conns.iter().any(|c| c.name.is_none() && c.expr.is_some()) {
        return;
    }
    let what = if parameters { "parameter" } else { "port" };
    let names: Option<Vec<&str>> = module.map(|m| {
        if parameters {
            m.generics.iter().map(|g| g.name.as_str()).collect()
        } else {
            m.ports.iter().map(|p| p.name.as_str()).collect()
        }
    });
    let fix = names.and_then(|names| {
        let mut edits = Vec::new();
        for (i, c) in conns.iter().enumerate() {
            if c.name.is_some() {
                continue;
            }
            let name = names.get(i)?;
            let range = match &c.expr {
                Some(expr) => super::trimmed_range(expr),
                None => continue,
            };
            edits.push(Edit {
                range: TextRange::empty(range.start()),
                text: format!(".{name}("),
            });
            edits.push(Edit {
                range: TextRange::empty(range.end()),
                text: ")".into(),
            });
        }
        Some(Fix {
            title: format!("Name the {what}s"),
            edits,
            safe: true,
        })
    });
    let range = match (conns.first(), conns.last()) {
        (Some(first), Some(last)) => first.node.text_range().cover(last.node.text_range()),
        _ => list.text_range(),
    };
    let mut finding = Finding::new(
        "style/positional-association",
        range,
        format!("{what}s are connected by position; name them"),
    );
    finding.fix = fix;
    out.push(finding);
}
