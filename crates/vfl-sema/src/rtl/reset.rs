//! Resets of clocked processes: registers a reset leaves out, reset
//! signals used both synchronously and asynchronously, and reset signals
//! tested with both polarities.

use rustc_hash::FxHashSet;
use vfl_syntax::{SyntaxKind, SyntaxNode, TextRange};

use super::Checker;
use crate::decl::DeclId;
use crate::diagnostic::{Diagnostic, codes};
use crate::resets::{consistency, is_reset_name, named_active_low};

pub(super) type ResetUse = crate::resets::ResetUse<DeclId>;

/// The branches of an `if` statement: each condition (`None` for `else`)
/// with its statements.
pub(crate) fn if_branches(node: &SyntaxNode) -> Vec<(Option<SyntaxNode>, Vec<SyntaxNode>)> {
    use SyntaxKind::*;
    let mut branches: Vec<(Option<SyntaxNode>, Vec<SyntaxNode>)> = Vec::new();
    let mut in_condition = false;
    for element in node.children_with_tokens() {
        match element {
            rowan::NodeOrToken::Token(t) => match t.kind() {
                IF_KW | ELSIF_KW => in_condition = true,
                ELSE_KW => branches.push((None, Vec::new())),
                END_KW => break,
                _ => {}
            },
            rowan::NodeOrToken::Node(n) => {
                if n.kind() == LABEL {
                    continue;
                }
                if in_condition {
                    in_condition = false;
                    branches.push((Some(n), Vec::new()));
                } else if let Some(branch) = branches.last_mut() {
                    branch.1.push(n);
                }
            }
        }
    }
    branches
}

/// The statements of a process body.
pub(crate) fn process_body(process: &SyntaxNode) -> Vec<SyntaxNode> {
    process
        .children_with_tokens()
        .skip_while(|e| e.kind() != SyntaxKind::BEGIN_KW)
        .filter_map(|e| e.into_node())
        .collect()
}

impl Checker<'_> {
    /// A test of a single signal: `rst = '1'`, `rst_n = '0'`, `rst /= '1'`,
    /// `rst`, `not rst_n`, `rst = RESET_LEVEL`. Returns the signal, the
    /// range of its name and whether the test is true when it is high.
    fn signal_test(&self, cond: &SyntaxNode) -> Option<(DeclId, TextRange, Option<bool>)> {
        use SyntaxKind::*;
        match cond.kind() {
            PAREN_EXPR => self.signal_test(&cond.children().next()?),
            NAME => {
                let token = crate::resolver::mark_token(cond)?;
                let id = self.target(&token)?;
                self.decl(id)?.kind.is_signal().then_some(())?;
                Some((id, token.text_range(), Some(true)))
            }
            UNARY_EXPR if super::operator(cond)? == NOT_KW => {
                let (id, range, high) = self.signal_test(&cond.children().next()?)?;
                Some((id, range, high.map(|h| !h)))
            }
            BINARY_EXPR => {
                let negated = match super::operator(cond)? {
                    EQ => false,
                    NEQ => true,
                    _ => return None,
                };
                let mut operands = cond.children();
                let (left, right) = (operands.next()?, operands.next()?);
                let (name, level) = if left.kind() == LITERAL {
                    (right, left)
                } else {
                    (left, right)
                };
                if name.kind() != NAME {
                    return None;
                }
                let token = crate::resolver::mark_token(&name)?;
                let id = self.target(&token)?;
                self.decl(id)?.kind.is_signal().then_some(())?;
                let high = match level.kind() {
                    LITERAL => match level.first_token()?.text() {
                        "'1'" => Some(true),
                        "'0'" => Some(false),
                        _ => return None,
                    },
                    NAME => {
                        let text = crate::resolver::mark_token(&level)?
                            .text()
                            .to_ascii_lowercase();
                        match text.as_str() {
                            "true" => Some(true),
                            "false" => Some(false),
                            // A constant or generic level.
                            _ => {
                                let decl = self.named(&level)?;
                                if !matches!(
                                    decl.kind,
                                    crate::decl::DeclKind::Constant
                                        | crate::decl::DeclKind::Generic { .. }
                                ) {
                                    return None;
                                }
                                None
                            }
                        }
                    }
                    _ => return None,
                };
                Some((id, token.text_range(), high.map(|h| h != negated)))
            }
            _ => None,
        }
    }

    /// Finds the reset of a clocked process without `wait` whose clock edge
    /// is `edge`, and reports the registers it leaves out.
    pub(super) fn resets(&mut self, process: &SyntaxNode, edge: &SyntaxNode, clock: DeclId) {
        let body = process_body(process);
        let [statement] = body.as_slice() else {
            return;
        };
        if statement.kind() != SyntaxKind::IF_STMT {
            return;
        }
        let in_generate = process.ancestors().any(|n| {
            matches!(
                n.kind(),
                SyntaxKind::IF_GENERATE_STMT | SyntaxKind::CASE_GENERATE_STMT
            )
        });
        let contains_edge = |n: &Option<SyntaxNode>| {
            n.as_ref()
                .is_some_and(|c| c.text_range().contains_range(edge.text_range()))
        };
        let branches = if_branches(statement);
        let Some(edge_at) = branches.iter().position(|(c, _)| contains_edge(c)) else {
            return;
        };
        // Nothing may follow the clocked branch: `else` would be
        // combinational.
        if edge_at + 1 != branches.len() {
            return;
        }
        let clocked = &branches[edge_at].1;
        if edge_at > 0 {
            // `if rst = '1' then ... elsif rising_edge(clk) then`.
            let Some((signal, range, active_high)) = branches[0]
                .0
                .as_ref()
                .and_then(|c| self.signal_test(c))
                .filter(|(s, ..)| *s != clock)
            else {
                return;
            };
            let reset: Vec<SyntaxNode> = branches[..edge_at]
                .iter()
                .flat_map(|(_, s)| s.iter().cloned())
                .collect();
            self.record_reset(signal, false, active_high, range, in_generate);
            self.unreset(signal, &reset, clocked);
            return;
        }
        // `if rising_edge(clk) then if rst = '1' then ... else ...`.
        let sync_test = |s: &SyntaxNode| {
            if s.kind() != SyntaxKind::IF_STMT {
                return None;
            }
            let branches = if_branches(s);
            let test = self.signal_test(branches.first()?.0.as_ref()?)?;
            let name = &self.decl(test.0)?.spelling;
            (is_reset_name(name) && test.0 != clock).then_some((test, branches))
        };
        if let Some(((signal, range, active_high), inner)) = clocked.first().and_then(sync_test) {
            self.record_reset(signal, true, active_high, range, in_generate);
            let others: Vec<SyntaxNode> = inner[1..]
                .iter()
                .flat_map(|(_, s)| s.iter().cloned())
                .chain(clocked[1..].iter().cloned())
                .collect();
            self.unreset(signal, &inner[0].1, &others);
        } else if clocked.len() > 1
            && let Some(((signal, range, active_high), inner)) = clocked.last().and_then(sync_test)
            && inner.len() == 1
        {
            // A reset that overrides the assignments before it: registers
            // it leaves out are not affected by it.
            self.record_reset(signal, true, active_high, range, in_generate);
            self.reset_ranges
                .extend(inner[0].1.iter().map(|s| s.text_range()));
        }
    }

    fn record_reset(
        &mut self,
        signal: DeclId,
        sync: bool,
        active_high: Option<bool>,
        range: TextRange,
        in_generate: bool,
    ) {
        let name = self
            .decl(signal)
            .map(|d| d.spelling.clone())
            .unwrap_or_default();
        if let Some(message) = named_active_low(&name, active_high, false) {
            self.diags.push(Diagnostic {
                code: codes::RESET_POLARITY,
                file: self.file,
                range,
                message,
                fix: None,
            });
        }
        self.reset_uses.push(ResetUse {
            key: signal,
            name,
            sync,
            active_high,
            range,
            in_generate,
        });
    }

    /// Reports the signals `clocked` assigns that `reset` does not.
    fn unreset(&mut self, signal: DeclId, reset: &[SyntaxNode], clocked: &[SyntaxNode]) {
        self.reset_ranges
            .extend(reset.iter().map(|s| s.text_range()));
        let mut reset_signals = FxHashSet::default();
        for statement in reset {
            for r in self.refs_in(statement.text_range()) {
                if r.access.writes() {
                    reset_signals.extend(self.signals(r));
                }
            }
        }
        let mut reported = FxHashSet::default();
        let mut found = Vec::new();
        for statement in clocked {
            for r in self.refs_in(statement.text_range()) {
                if !r.access.writes() {
                    continue;
                }
                for id in self.signals(r) {
                    if reset_signals.contains(&id) || !reported.insert(id) {
                        continue;
                    }
                    // Memories cannot be reset.
                    if self.decl(id).is_none_or(|d| self.is_memory(d)) {
                        continue;
                    }
                    found.push((id, r.range));
                }
            }
        }
        let reset_name = self
            .decl(signal)
            .map(|d| d.spelling.clone())
            .unwrap_or_default();
        for (id, range) in found {
            let name = self
                .decl(id)
                .map(|d| d.spelling.clone())
                .unwrap_or_default();
            self.diags.push(Diagnostic {
                code: codes::MISSING_RESET,
                file: self.file,
                range,
                message: format!(
                    "`{name}` is not reset by `{reset_name}` in this process, so the reset acts as an enable on it"
                ),
                fix: None,
            });
        }
    }

    /// Reset signals used both synchronously and asynchronously, or tested
    /// with both polarities, within the unit.
    pub(super) fn reset_consistency(&mut self) {
        let uses = std::mem::take(&mut self.reset_uses);
        let line = |at| self.ctx.line_of(self.file, at) as usize;
        let found = consistency(&uses, "process", line, true, true);
        self.diags.extend(found.into_iter().map(|i| Diagnostic {
            code: i.code,
            file: self.file,
            range: i.range,
            message: i.message,
            fix: None,
        }));
    }
}
