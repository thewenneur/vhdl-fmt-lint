//! Inlay hints: the formal before each positional actual of a port map, a
//! generic map or a subprogram call, and, optionally, the mode and type of
//! the port after each actual of a port map.
//!
//! Only the part of the tree the editor shows is walked, and the formals
//! come from the analysis the server already has: the reference of the
//! instantiated unit or of the called subprogram. `[lsp.inlay_hints]` in
//! `vfl.toml` says which hints a project wants; the editor's settings can
//! override each.

use lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, InlayHintParams};
use serde::Deserialize;
use serde_json::Value;
use vfl_config::InlayHintsConfig;
use vfl_sema::{Analysis, DeclId, DeclKind, FileId, Target};
use vfl_syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange, TextSize};

use crate::types::reference_at;
use crate::world::World;

/// The editor's choice for each kind of hint; `None` keeps the project's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Overrides {
    ports: Option<bool>,
    generics: Option<bool>,
    parameters: Option<bool>,
    #[serde(alias = "portTypes")]
    port_types: Option<bool>,
}

impl Overrides {
    fn apply(self, config: InlayHintsConfig) -> InlayHintsConfig {
        InlayHintsConfig {
            ports: self.ports.unwrap_or(config.ports),
            generics: self.generics.unwrap_or(config.generics),
            parameters: self.parameters.unwrap_or(config.parameters),
            port_types: self.port_types.unwrap_or(config.port_types),
        }
    }
}

/// Takes the editor's `inlayHints` settings, from its initialization
/// options or a configuration change, when it sends some.
pub fn editor_settings(world: &mut World, settings: &Value) {
    if let Some(hints) = settings.get("inlayHints").filter(|h| !h.is_null()) {
        world.inlay_overrides = Overrides::deserialize(hints).unwrap_or_default();
    }
}

/// A hint, before converting its offset to a position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hint {
    pub offset: TextSize,
    pub label: String,
    /// A parameter name, before its actual; else a type, after it.
    pub parameter: bool,
}

/// `textDocument/inlayHint`.
pub fn inlay_hints(world: &mut World, params: InlayHintParams) -> Option<Vec<InlayHint>> {
    let path = params.text_document.uri.to_file_path().ok()?;
    let file = world.file_id(&path)?;
    let config = world
        .inlay_overrides
        .apply(world.config(&path).lsp.inlay_hints);
    let doc = world.doc(file);
    let range = TextRange::new(
        doc.offset(params.range.start),
        doc.offset(params.range.end)
            .max(doc.offset(params.range.start)),
    );
    Some(
        hints(world, file, range, config)
            .into_iter()
            .map(|hint| InlayHint {
                position: doc.position(hint.offset),
                label: InlayHintLabel::String(hint.label),
                kind: Some(if hint.parameter {
                    InlayHintKind::PARAMETER
                } else {
                    InlayHintKind::TYPE
                }),
                text_edits: None,
                tooltip: None,
                padding_left: None,
                padding_right: hint.parameter.then_some(true),
                data: None,
            })
            .collect(),
    )
}

/// The hints of `file` within `range`.
pub(crate) fn hints(
    world: &World,
    file: FileId,
    range: TextRange,
    config: InlayHintsConfig,
) -> Vec<Hint> {
    let mut out = Vec::new();
    if !(config.ports || config.generics || config.parameters || config.port_types) {
        return out;
    }
    let Some(f) = world.analysis.files().get(file as usize) else {
        return out;
    };
    let walk = Walk {
        analysis: &world.analysis,
        file,
        range,
        config,
    };
    walk.node(&f.syntax(), &mut out);
    out
}

struct Walk<'a> {
    analysis: &'a Analysis,
    file: FileId,
    range: TextRange,
    config: InlayHintsConfig,
}

impl Walk<'_> {
    /// Visits the association lists under `node` that overlap the range.
    fn node(&self, node: &SyntaxNode, out: &mut Vec<Hint>) {
        for child in node.children() {
            let r = child.text_range();
            if r.end() < self.range.start() {
                continue;
            }
            if r.start() > self.range.end() {
                break;
            }
            if child.kind() == SyntaxKind::ASSOCIATION_LIST {
                self.list(&child, out);
            }
            self.node(&child, out);
        }
    }

    fn list(&self, list: &SyntaxNode, out: &mut Vec<Hint>) {
        let Some(parent) = list.parent() else { return };
        let (formals, ports) = match parent.kind() {
            SyntaxKind::PORT_MAP_ASPECT | SyntaxKind::GENERIC_MAP_ASPECT => {
                let ports = parent.kind() == SyntaxKind::PORT_MAP_ASPECT;
                let wanted = if ports {
                    self.config.ports || self.config.port_types
                } else {
                    self.config.generics
                };
                if !wanted {
                    return;
                }
                let Some(formals) = parent.parent().and_then(|s| self.map_formals(&s, ports))
                else {
                    return;
                };
                (formals, ports)
            }
            SyntaxKind::CALL_EXPR if self.config.parameters => {
                let Some(formals) = self.call_formals(&parent) else {
                    return;
                };
                (formals, false)
            }
            _ => return,
        };
        let names = if ports { self.config.ports } else { true };
        let mut positional = true;
        for (i, element) in list
            .children()
            .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
            .enumerate()
        {
            let Some((first, last)) = significant_ends(&element) else {
                continue;
            };
            positional &= !element
                .children_with_tokens()
                .any(|e| e.kind() == SyntaxKind::ARROW);
            let formal = if positional {
                formals.get(i).copied().flatten()
            } else if ports && self.config.port_types {
                self.named_formal(&element)
            } else {
                break;
            };
            let Some(formal) = formal.and_then(|id| self.analysis.decl(id)) else {
                continue;
            };
            let start = first.text_range().start();
            if positional && names && self.in_range(start) {
                let actual = element.text().to_string();
                if vfl_syntax::identifier_key(actual.trim()) != formal.name {
                    out.push(Hint {
                        offset: start,
                        label: format!("{}:", formal.spelling),
                        parameter: true,
                    });
                }
            }
            let end = last.text_range().end();
            if ports && self.config.port_types && self.in_range(end) {
                let mut label = ":".to_owned();
                if let DeclKind::Port { mode, .. } = formal.kind {
                    label.push(' ');
                    label.push_str(mode.as_str());
                }
                if let Some(ty) = formal.subtype.and_then(|t| self.analysis.decl(t)) {
                    label.push(' ');
                    label.push_str(&ty.spelling);
                }
                if label.len() > 1 {
                    out.push(Hint {
                        offset: end,
                        label,
                        parameter: false,
                    });
                }
            }
        }
    }

    fn in_range(&self, offset: TextSize) -> bool {
        self.range.contains_inclusive(offset)
    }

    /// The generics or ports, in order, of what a statement or declaration
    /// with a map aspect instantiates: an entity, a component, a package.
    fn map_formals(&self, stmt: &SyntaxNode, ports: bool) -> Option<Vec<Option<DeclId>>> {
        if !matches!(
            stmt.kind(),
            SyntaxKind::COMPONENT_INSTANTIATION_STMT | SyntaxKind::PACKAGE_INSTANTIATION_DECL
        ) {
            return None;
        }
        // `entity work.e(rtl)`: the name is the call's first part.
        let mut unit = stmt.children().find(|n| {
            matches!(
                n.kind(),
                SyntaxKind::NAME | SyntaxKind::SELECTED_NAME | SyntaxKind::CALL_EXPR
            )
        })?;
        if unit.kind() == SyntaxKind::CALL_EXPR {
            unit = unit.first_child()?;
        }
        let name = last_significant(&unit)?;
        let reference = reference_at(self.analysis, self.file, name.text_range())?;
        let members = reference.targets.iter().find_map(|t| match t {
            Target::Decl(id) => self.analysis.decl(*id)?.members.clone(),
            Target::Library(_) => None,
        })?;
        if members.opaque {
            return None;
        }
        Some(
            members
                .iter()
                .filter(|&id| {
                    self.analysis.decl(id).is_some_and(|d| match d.kind {
                        DeclKind::Port { .. } => ports,
                        DeclKind::Generic { .. }
                        | DeclKind::GenericType
                        | DeclKind::GenericSubprogram
                        | DeclKind::GenericPackage => !ports,
                        _ => false,
                    })
                })
                .map(Some)
                .collect(),
        )
    }

    /// The parameters of the subprogram a call names, by position: `None`
    /// where its possible overloads name them differently. Nothing for an
    /// indexed name or a type conversion, and nothing for the calls of
    /// the bundled libraries' functions of one parameter, such as
    /// `rising_edge(clk)`, whose name says it all.
    fn call_formals(&self, call: &SyntaxNode) -> Option<Vec<Option<DeclId>>> {
        let prefix = call.first_child()?;
        if !matches!(prefix.kind(), SyntaxKind::NAME | SyntaxKind::SELECTED_NAME) {
            return None;
        }
        let name = last_significant(&prefix)?;
        if !matches!(name.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT) {
            return None;
        }
        let reference = reference_at(self.analysis, self.file, name.text_range())?;
        let mut formals: Option<Vec<Option<DeclId>>> = None;
        for target in &reference.targets {
            let Target::Decl(id) = target else {
                return None;
            };
            let decl = self.analysis.decl(*id)?;
            if !matches!(decl.kind, DeclKind::Function | DeclKind::Procedure) {
                return None;
            }
            let params = decl.params.as_deref().unwrap_or_default();
            if params.len() == 1 && self.analysis.files()[decl.file as usize].builtin {
                return None;
            }
            formals = Some(match formals {
                None => params.iter().copied().map(Some).collect(),
                Some(previous) => previous
                    .into_iter()
                    .zip(params)
                    .map(|(a, b)| {
                        let same = a.zip(self.analysis.decl(*b)).is_some_and(|(a, b)| {
                            self.analysis.decl(a).is_some_and(|a| a.name == b.name)
                        });
                        a.filter(|_| same)
                    })
                    .collect(),
            });
        }
        formals
    }

    /// The port a named association's formal part names.
    fn named_formal(&self, element: &SyntaxNode) -> Option<DeclId> {
        let formal = element.first_child()?;
        formal
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::EXT_IDENT))
            .find_map(|t| {
                let reference = reference_at(self.analysis, self.file, t.text_range())?;
                reference.targets.iter().find_map(|target| match target {
                    Target::Decl(id)
                        if self
                            .analysis
                            .decl(*id)
                            .is_some_and(|d| matches!(d.kind, DeclKind::Port { .. })) =>
                    {
                        Some(*id)
                    }
                    _ => None,
                })
            })
    }
}

fn last_significant(node: &SyntaxNode) -> Option<SyntaxToken> {
    let mut token = node.last_token()?;
    while token.kind().is_trivia() {
        token = token.prev_token()?;
    }
    Some(token)
}

/// The first and last tokens of a node that are not trivia.
fn significant_ends(node: &SyntaxNode) -> Option<(SyntaxToken, SyntaxToken)> {
    let mut first = node.first_token()?;
    while first.kind().is_trivia() {
        first = first.next_token()?;
    }
    Some((first, last_significant(node)?))
}
