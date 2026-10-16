//! The sensitivity list check.

use rustc_hash::{FxHashMap, FxHashSet};
use vfl_syntax::green_walk::ChildTokens;
use vfl_syntax::{SyntaxKind, SyntaxNode, TextRange};

use super::{Resolver, is_clock_edge};
use crate::decl::{DeclId, Reference, Target};
use crate::diagnostic::{Diagnostic, Edit, Fix, codes};

impl<'a> Resolver<'a> {
    /// Missing and unneeded entries of a process's sensitivity list.
    pub(super) fn check_sensitivity(
        &mut self,
        process: &SyntaxNode,
        list: &SyntaxNode,
        sens_refs: std::ops::Range<usize>,
        refs_start: usize,
    ) {
        if self.quiet > 0 {
            return;
        }
        let signal_of = |r: &Reference, me: &Self| -> Option<DeclId> {
            r.targets.iter().find_map(|t| match t {
                Target::Decl(id) => me.decl(*id).filter(|d| d.kind.is_signal()).map(|_| *id),
                _ => None,
            })
        };
        let mut listed: FxHashMap<DeclId, TextRange> = FxHashMap::default();
        for r in &self.refs[sens_refs.clone()] {
            if let Some(id) = signal_of(r, self) {
                listed.insert(id, r.range);
            }
        }
        let body: Vec<&Reference> = self.refs[refs_start..]
            .iter()
            .enumerate()
            .filter(|(i, _)| !sens_refs.contains(&(refs_start + i)))
            .map(|(_, r)| r)
            .collect();
        let required = required_ranges(process);
        let mut missing: Vec<(DeclId, String)> = Vec::new();
        let mut read: FxHashSet<DeclId> = FxHashSet::default();
        for r in &body {
            if !r.access.reads() {
                continue;
            }
            let Some(id) = signal_of(r, self) else {
                continue;
            };
            read.insert(id);
            let needed = match &required {
                Some(ranges) => ranges.iter().any(|range| range.contains_range(r.range)),
                None => false,
            };
            if needed && !listed.contains_key(&id) && !missing.iter().any(|(m, _)| *m == id) {
                let name = self
                    .decl(id)
                    .map(|d| d.spelling.clone())
                    .unwrap_or_default();
                missing.push((id, name));
            }
        }
        if !missing.is_empty() {
            let names: Vec<String> = missing.iter().map(|(_, n)| n.clone()).collect();
            let quoted: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
            let close = list.child_token(|k| k == SyntaxKind::RPAREN);
            let fix = close.map(|close| Fix {
                title: format!("Add {} to the sensitivity list", quoted.join(", ")),
                edits: vec![Edit {
                    range: TextRange::empty(close.text_range().start()),
                    text: format!(", {}", names.join(", ")),
                }],
                safe: false,
            });
            self.diags.push(Diagnostic {
                code: codes::SENSITIVITY_LIST,
                file: self.file,
                range: list.text_range(),
                message: format!(
                    "the process reads {} but its sensitivity list does not name {}",
                    quoted.join(", "),
                    if missing.len() == 1 { "it" } else { "them" }
                ),
                fix,
            });
        }
        let mut unneeded: Vec<(DeclId, TextRange)> = listed
            .into_iter()
            .filter(|(id, _)| !read.contains(id))
            .collect();
        unneeded.sort_by_key(|(_, r)| r.start());
        for (id, range) in unneeded {
            let name = self
                .decl(id)
                .map(|d| d.spelling.clone())
                .unwrap_or_default();
            // A process without its list would never wait: keep the last.
            let fix = crate::diagnostic::list_item_removal(
                &self.ctx.files[self.file as usize].text,
                range,
            )
            .map(|removed| Fix {
                title: format!("Remove `{name}` from the sensitivity list"),
                edits: vec![Edit {
                    range: removed,
                    text: String::new(),
                }],
                safe: false,
            });
            self.diags.push(Diagnostic {
                code: codes::UNNEEDED_SENSITIVITY,
                file: self.file,
                range,
                message: format!(
                    "`{name}` is in the sensitivity list but the process never reads it"
                ),
                fix,
            });
        }
    }
}

/// The parts of a process whose signal reads must be in its sensitivity
/// list, or `None` when the process has a shape the check does not
/// understand (it is then skipped).
///
/// A combinational process needs every signal it reads. A clocked process,
/// a single `if` whose branch tests a clock edge, needs the signals read in
/// the conditions up to and including the edge test, and in the branches
/// before it (an asynchronous reset).
fn required_ranges(process: &SyntaxNode) -> Option<Vec<TextRange>> {
    let begin = process
        .children_with_tokens()
        .position(|e| e.kind() == SyntaxKind::BEGIN_KW)?;
    let statements: Vec<SyntaxNode> = process
        .children_with_tokens()
        .skip(begin + 1)
        .filter_map(|e| e.into_node())
        .collect();
    let has_edge = |node: &SyntaxNode| node.descendants().any(|n| is_clock_edge(&n));
    let clocked = statements.iter().any(has_edge);
    if !clocked {
        return Some(statements.iter().map(SyntaxNode::text_range).collect());
    }
    let [single] = statements.as_slice() else {
        return None;
    };
    if single.kind() != SyntaxKind::IF_STMT {
        return None;
    }
    // Walk the branches: conditions follow `if`/`elsif`, bodies follow `then`/`else`.
    let mut ranges = Vec::new();
    let mut expect_condition = false;
    let mut in_body = false;
    for element in single.children_with_tokens() {
        match element {
            rowan::NodeOrToken::Token(t) => match t.kind() {
                SyntaxKind::IF_KW | SyntaxKind::ELSIF_KW => {
                    expect_condition = true;
                    in_body = false;
                }
                SyntaxKind::THEN_KW | SyntaxKind::ELSE_KW => in_body = true,
                SyntaxKind::END_KW => break,
                _ => {}
            },
            rowan::NodeOrToken::Node(n) => {
                if expect_condition {
                    expect_condition = false;
                    let edges: Vec<TextRange> = n
                        .descendants()
                        .filter(is_clock_edge)
                        .map(|e| e.text_range())
                        .collect();
                    if !edges.is_empty() {
                        // Only the clock itself: other signals tested with
                        // the edge are sampled on it.
                        ranges.extend(edges);
                        return Some(ranges);
                    }
                    ranges.push(n.text_range());
                } else if in_body {
                    ranges.push(n.text_range());
                }
            }
        }
    }
    None
}
