//! `project/constant-port`: input ports that every instance in the
//! project ties to the same constant.

use rustc_hash::FxHashMap;
use vfl_syntax::green_walk::ChildTokens;
use vfl_syntax::{SyntaxKind, SyntaxNode};

use super::canonical;
use crate::analysis::Analysis;
use crate::decl::{Access, DeclId, DeclKind, FileId, Mode, Target};
use crate::diagnostic::{Diagnostic, codes};

impl Analysis {
    /// The `project/constant-port` findings of the whole project: one pass
    /// over its references, for the formals of named port associations.
    pub fn constant_ports(&self) -> Vec<Diagnostic> {
        let mut roots: FxHashMap<FileId, SyntaxNode> = FxHashMap::default();
        let mut actuals: FxHashMap<DeclId, Vec<Option<String>>> = FxHashMap::default();
        for (file, r) in self.references() {
            if r.access != Access::Other {
                continue;
            }
            let mut targets = r.targets.iter().filter_map(|t| match t {
                Target::Decl(id) => Some(*id),
                Target::Library(_) => None,
            });
            let (Some(id), None) = (targets.next(), targets.next()) else {
                continue;
            };
            let Some(decl) = self.decl(id) else {
                continue;
            };
            if !matches!(decl.kind, DeclKind::Port { mode: Mode::In, .. })
                || self.files()[decl.file as usize].builtin
            {
                continue;
            }
            let root = roots
                .entry(file)
                .or_insert_with(|| self.files()[file as usize].syntax());
            let Some(token) = root.covering_element(r.range).into_token() else {
                continue;
            };
            let Some(element) = token
                .parent_ancestors()
                .find(|a| a.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
            else {
                continue;
            };
            let in_port_map = element
                .parent()
                .and_then(|list| list.parent())
                .is_some_and(|m| m.kind() == SyntaxKind::PORT_MAP_ASPECT);
            let formal = element.children().next();
            if !in_port_map
                || !element.has_child(|k| k == SyntaxKind::ARROW)
                || formal.is_none_or(|f| !f.text_range().contains_range(r.range))
            {
                continue;
            }
            let actual = element.children().last().filter(|a| {
                matches!(a.kind(), SyntaxKind::LITERAL | SyntaxKind::AGGREGATE)
                    || (a.kind() == SyntaxKind::NAME
                        && matches!(canonical(a).as_str(), "true" | "false"))
            });
            // `open`, a signal or anything else: not a constant.
            let key = actual
                .filter(|a| {
                    let mut names = false;
                    vfl_syntax::green_walk::for_each_token(a, |k, _, t| {
                        names |= k == SyntaxKind::IDENT
                            && !matches!(
                                t.to_ascii_lowercase().as_str(),
                                "others" | "true" | "false"
                            );
                    });
                    !names
                })
                .map(|a| canonical(&a));
            actuals.entry(id).or_default().push(key);
        }
        let mut out = Vec::new();
        for (id, values) in actuals {
            if values.len() < 2 {
                continue;
            }
            let Some(Some(first)) = values.first() else {
                continue;
            };
            if !values.iter().all(|v| v.as_ref() == Some(first)) {
                continue;
            }
            let Some(decl) = self.decl(id) else {
                continue;
            };
            out.push(Diagnostic {
                code: codes::CONSTANT_PORT,
                file: decl.file,
                range: decl.range,
                message: format!(
                    "every instance ({}) ties `{}` to `{first}`: the logic that handles its \
                     other values is dead; make it a constant or a generic",
                    values.len(),
                    decl.spelling
                ),
                fix: None,
            });
        }
        out.sort_by_key(|d| (d.file, d.range.start()));
        out
    }
}
