//! What a file declares and which library units it refers to: the input to
//! library inference, read straight off the syntax tree.

use rustc_hash::FxHashSet;
use vfl_syntax::green_walk::{ChildTokens, descendants_where};
use vfl_syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange, identifier_key};

/// The kind of a design unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnitKind {
    Entity,
    Architecture,
    Package,
    PackageBody,
    PackageInstance,
    Context,
    Configuration,
}

impl UnitKind {
    /// Primary units are the ones named in a library; secondary units
    /// (architectures and package bodies) attach to a primary unit.
    pub fn is_primary(self) -> bool {
        !matches!(self, UnitKind::Architecture | UnitKind::PackageBody)
    }

    pub fn from_node(kind: SyntaxKind) -> Option<UnitKind> {
        Some(match kind {
            SyntaxKind::ENTITY_DECL => UnitKind::Entity,
            SyntaxKind::ARCHITECTURE_BODY => UnitKind::Architecture,
            SyntaxKind::PACKAGE_DECL => UnitKind::Package,
            SyntaxKind::PACKAGE_BODY => UnitKind::PackageBody,
            SyntaxKind::PACKAGE_INSTANTIATION_DECL => UnitKind::PackageInstance,
            SyntaxKind::CONTEXT_DECL => UnitKind::Context,
            SyntaxKind::CONFIGURATION_DECL => UnitKind::Configuration,
            _ => return None,
        })
    }

    pub fn describe(self) -> &'static str {
        match self {
            UnitKind::Entity => "entity",
            UnitKind::Architecture => "architecture",
            UnitKind::Package => "package",
            UnitKind::PackageBody => "package body",
            UnitKind::PackageInstance => "package",
            UnitKind::Context => "context",
            UnitKind::Configuration => "configuration",
        }
    }
}

/// One design unit of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitSummary {
    pub kind: UnitKind,
    /// The unit's name, as an identifier key.
    pub name: String,
    /// For an architecture or configuration, its entity; for a package
    /// body, its package.
    pub primary: Option<String>,
    /// The range of the unit's name.
    pub name_range: TextRange,
}

/// A reference to a library unit: `lib.unit` in a use clause, an
/// instantiation or an expression, or an implied `work` reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitRef {
    /// The library as written (`work` for references to the file's own
    /// library), as an identifier key.
    pub library: String,
    pub unit: String,
    /// Implied rather than written: the entity bound to a component
    /// instantiation by default. Used for inference only when it names
    /// a unit that exists.
    pub implicit: bool,
}

/// The design units a file declares and the units it refers to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileSummary {
    pub units: Vec<UnitSummary>,
    pub refs: Vec<UnitRef>,
}

/// The first identifier token directly under `node`.
pub fn name_token(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.child_token(|k| matches!(k, SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
}

/// The identifier of a simple `NAME` node.
pub fn simple_name(node: &SyntaxNode) -> Option<SyntaxToken> {
    if node.kind() == SyntaxKind::NAME {
        name_token(node)
    } else {
        None
    }
}

/// Summarizes a parsed file.
pub fn summarize(root: &SyntaxNode) -> FileSummary {
    let mut summary = FileSummary::default();

    let mut libraries: FxHashSet<String> = ["work".to_owned()].into_iter().collect();
    for clause in descendants_where(root, |k| k == SyntaxKind::LIBRARY_CLAUSE) {
        for list in clause.children() {
            for token in list.children_with_tokens().filter_map(|e| e.into_token()) {
                if matches!(token.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT) {
                    libraries.insert(identifier_key(token.text()));
                }
            }
        }
    }

    for unit in root
        .children()
        .filter(|n| n.kind() == SyntaxKind::DESIGN_UNIT)
        .flat_map(|n| n.children())
    {
        let Some(kind) = UnitKind::from_node(unit.kind()) else {
            continue;
        };
        let Some(name) = name_token(&unit) else {
            continue;
        };
        let primary = match kind {
            UnitKind::Architecture | UnitKind::Configuration => unit
                .children()
                .find(|n| n.kind() == SyntaxKind::NAME)
                .and_then(|n| simple_name(&n))
                .map(|t| identifier_key(t.text())),
            UnitKind::PackageBody => Some(identifier_key(name.text())),
            _ => None,
        };
        if let Some(primary) = &primary {
            summary.refs.push(UnitRef {
                library: "work".into(),
                unit: primary.clone(),
                implicit: false,
            });
        }
        summary.units.push(UnitSummary {
            kind,
            name: identifier_key(name.text()),
            primary,
            name_range: name.text_range(),
        });
    }

    // The kinds the arms below look at: only these become syntax nodes.
    let wanted = |k| {
        matches!(
            k,
            SyntaxKind::SELECTED_NAME | SyntaxKind::COMPONENT_INSTANTIATION_STMT
        )
    };
    for node in descendants_where(root, wanted) {
        match node.kind() {
            SyntaxKind::SELECTED_NAME => {
                let mut children = node.children_with_tokens();
                let Some(prefix) = children.next().and_then(|e| e.into_node()) else {
                    continue;
                };
                let Some(library) = simple_name(&prefix).map(|t| identifier_key(t.text())) else {
                    continue;
                };
                if !libraries.contains(&library) {
                    continue;
                }
                let suffix = children
                    .filter_map(|e| e.into_token())
                    .find(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT));
                if let Some(suffix) = suffix {
                    summary.refs.push(UnitRef {
                        library,
                        unit: identifier_key(suffix.text()),
                        implicit: false,
                    });
                }
            }
            SyntaxKind::COMPONENT_INSTANTIATION_STMT => {
                // `u : comp port map (...)` binds to entity `work.comp` by default.
                let is_component = !node.has_child(|k| {
                    matches!(k, SyntaxKind::ENTITY_KW | SyntaxKind::CONFIGURATION_KW)
                });
                let name = node
                    .children()
                    .find(|n| n.kind() == SyntaxKind::NAME)
                    .and_then(|n| simple_name(&n));
                if let (true, Some(name)) = (is_component, name) {
                    summary.refs.push(UnitRef {
                        library: "work".into(),
                        unit: identifier_key(name.text()),
                        implicit: true,
                    });
                }
            }
            _ => {}
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use vfl_syntax::{Standard, parse};

    #[test]
    fn units_and_references() {
        let text = "library ieee, osvvm; use ieee.std_logic_1164.all; use osvvm.AlertLogPkg.all;\n\
                    entity E is end;\n\
                    architecture rtl of e is begin\n\
                      u0 : entity work.sub port map (a => b);\n\
                      u1 : comp port map (a => rec.field);\n\
                    end;\n\
                    package body p is end package body;";
        let parsed = parse(text, Standard::Vhdl2008);
        let summary = summarize(&parsed.syntax());
        let units: Vec<_> = summary
            .units
            .iter()
            .map(|u| (u.kind, u.name.as_str(), u.primary.as_deref()))
            .collect();
        assert_eq!(
            units,
            [
                (UnitKind::Entity, "e", None),
                (UnitKind::Architecture, "rtl", Some("e")),
                (UnitKind::PackageBody, "p", Some("p")),
            ]
        );
        let refs: Vec<_> = summary
            .refs
            .iter()
            .map(|r| (r.library.as_str(), r.unit.as_str(), r.implicit))
            .collect();
        assert_eq!(
            refs,
            [
                ("work", "e", false),
                ("work", "p", false),
                ("ieee", "std_logic_1164", false),
                ("osvvm", "alertlogpkg", false),
                ("work", "sub", false),
                ("work", "comp", true),
            ]
        );
    }
}
