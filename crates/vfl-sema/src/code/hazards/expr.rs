//! Checks of single expressions and assignments: `'-'` compared with `=`,
//! `srl` of a signed value, `resize` and conversions that lose bits,
//! conversions that do nothing, costly operators, and counters that run
//! past their bounds.

use vfl_syntax::green_walk::ChildTokens;
use vfl_syntax::{SyntaxKind, SyntaxNode, TextRange};

use super::{Hazards, Num, call_args, callee, op};
use crate::decl::{DeclId, DeclKind};
use crate::diagnostic::{Edit, Fix, codes};

/// Whether a literal holds a `'-'` among `std_ulogic` values:
/// `"1--0"`, `b"1-0"`.
fn has_dont_care(literal: &SyntaxNode) -> bool {
    let Some(token) = literal.first_token() else {
        return false;
    };
    let text = token.text();
    let inner = match token.kind() {
        SyntaxKind::STRING_LITERAL => text.trim_matches('"'),
        SyntaxKind::BIT_STRING_LITERAL => match text.find('"') {
            Some(q) => text[q..].trim_matches('"'),
            None => return false,
        },
        SyntaxKind::CHAR_LITERAL => &text[1..text.len().saturating_sub(1)],
        _ => return false,
    };
    inner.contains('-')
        && inner.chars().all(|c| {
            matches!(
                c.to_ascii_uppercase(),
                '0' | '1' | '-' | 'U' | 'X' | 'Z' | 'W' | 'L' | 'H' | '_'
            )
        })
}

impl Hazards<'_, '_, '_> {
    pub(super) fn binary(&mut self, node: &SyntaxNode) {
        use SyntaxKind::*;
        let Some(operator) = op(node) else {
            return;
        };
        let mut operands = node.children();
        let (Some(left), Some(right)) = (operands.next(), operands.next()) else {
            return;
        };
        match operator {
            EQ | NEQ if self.h.dont_care_compare => self.dont_care_compare(node, &left, &right),
            SRL_KW if self.h.signed_logical_shift => {
                if self.num(&left) == Some(Num::Signed) {
                    let fix = Fix {
                        title: "Use `shift_right`".into(),
                        edits: vec![Edit {
                            range: node.text_range(),
                            text: format!(
                                "shift_right({}, {})",
                                self.text(&left),
                                self.text(&right)
                            ),
                        }],
                        safe: false,
                    };
                    self.report_fix(
                        codes::SIGNED_LOGICAL_SHIFT,
                        node.text_range(),
                        format!(
                            "`srl` shifts zeros into the sign bit of signed `{}`, so negative \
                             values turn positive: `shift_right` keeps the sign",
                            self.text(&left)
                        ),
                        fix,
                    );
                }
            }
            SLASH | MOD_KW | REM_KW | STAR2 if self.h.expensive_operator => {
                self.expensive(node, operator, &left, &right)
            }
            _ => {}
        }
    }

    // ---- sema/dont-care-compare ---------------------------------------------

    fn dont_care_compare(&mut self, node: &SyntaxNode, left: &SyntaxNode, right: &SyntaxNode) {
        let (value, literal) = if right.kind() == SyntaxKind::LITERAL && has_dont_care(right) {
            (left, right)
        } else if left.kind() == SyntaxKind::LITERAL && has_dont_care(left) {
            (right, left)
        } else {
            return;
        };
        // A string, not a vector of `std_ulogic`.
        if let Some((_, decl)) = self.object(value)
            && self
                .type_names(decl)
                .iter()
                .any(|n| matches!(*n, "string" | "character"))
        {
            return;
        }
        let negated = op(node) == Some(SyntaxKind::NEQ);
        let call = format!("std_match({}, {})", self.text(value), self.text(literal));
        let fix = Fix {
            title: "Use `std_match`".into(),
            edits: vec![Edit {
                range: node.text_range(),
                text: if negated {
                    format!("not {call}")
                } else {
                    call.clone()
                },
            }],
            safe: false,
        };
        self.report_fix(
            codes::DONT_CARE_COMPARE,
            node.text_range(),
            format!(
                "`{}` compares `'-'` as a value, so it {} only when `{}` holds `'-'` there: \
                 use `{call}` (or `?=` in VHDL-2008)",
                if negated { "/=" } else { "=" },
                if negated { "is false" } else { "is true" },
                self.text(value)
            ),
            fix,
        );
    }

    /// Choices of an ordinary `case` or selected assignment that hold
    /// `'-'`, which only `case?` and `select?` read as "any value".
    pub(super) fn dont_care_choices(&mut self, node: &SyntaxNode) {
        // `case?` and `select?`.
        if node.has_child(|k| k == SyntaxKind::QUESTION) {
            return;
        }
        let choices: Vec<SyntaxNode> = match node.kind() {
            SyntaxKind::CASE_STMT => node
                .children()
                .filter(|n| n.kind() == SyntaxKind::CASE_ALTERNATIVE)
                .filter_map(|a| a.children().find(|n| n.kind() == SyntaxKind::CHOICES))
                .collect(),
            _ => node
                .children()
                .filter(|n| n.kind() == SyntaxKind::SELECTED_WAVEFORMS)
                .flat_map(|w| w.children().filter(|n| n.kind() == SyntaxKind::CHOICES))
                .collect(),
        };
        // `'-'` is a plain value of `character` and other enumerations.
        let selector = node.children().find(|n| n.kind() != SyntaxKind::LABEL);
        if let Some((_, decl)) = selector.and_then(|s| self.object(&s))
            && !self.type_names(decl).iter().any(|n| {
                n.contains("std_ulogic") || n.contains("std_logic") || n.ends_with("signed")
            })
        {
            return;
        }
        let keyword = if node.kind() == SyntaxKind::CASE_STMT {
            "case"
        } else {
            "select"
        };
        for choice in choices.iter().flat_map(|c| c.children()) {
            if choice.kind() == SyntaxKind::LITERAL && has_dont_care(&choice) {
                self.report(
                    codes::DONT_CARE_COMPARE,
                    choice.text_range(),
                    format!(
                        "an ordinary `{keyword}` matches `'-'` only against `'-'`, so this \
                         choice is never taken: use `{keyword}?` (VHDL-2008) for \"any value\""
                    ),
                );
            }
        }
    }

    // ---- Calls ---------------------------------------------------------------

    pub(super) fn call(&mut self, node: &SyntaxNode) {
        let Some(name) = callee(node) else {
            return;
        };
        // An index or a slice, not a call.
        if node
            .children()
            .next()
            .is_some_and(|p| self.object(&p).is_some())
        {
            return;
        }
        let args = call_args(node);
        match name.as_str() {
            "resize" if self.h.signed_resize => self.signed_resize(node, &args),
            "to_integer" | "to_unsigned" | "to_signed" => {
                if self.h.conversion_overflow {
                    self.conversion_overflow(node, &name, &args);
                }
                if self.h.redundant_conversion {
                    self.round_trip(node, &name, &args);
                }
            }
            _ if self.h.redundant_conversion => self.same_type_conversion(node, &args),
            _ => {}
        }
    }

    // ---- sema/signed-resize --------------------------------------------------

    fn signed_resize(&mut self, node: &SyntaxNode, args: &[SyntaxNode]) {
        let [value, size] = args else {
            return;
        };
        if self.num(value) != Some(Num::Signed) {
            return;
        }
        let (Some(from), Some(to)) = (self.width(value), self.c.int(size)) else {
            return;
        };
        if to >= from || to < 1 {
            return;
        }
        self.report(
            codes::SIGNED_RESIZE,
            node.text_range(),
            format!(
                "`resize` of a signed value from {from} to {to} bits keeps the sign bit and the \
                 {} low bits: it neither truncates nor saturates; slice `{}({} downto 0)` to \
                 truncate, or saturate explicitly",
                to - 1,
                self.text(value),
                to - 1
            ),
        );
    }

    // ---- sema/conversion-overflow ----------------------------------------------

    fn conversion_overflow(&mut self, node: &SyntaxNode, name: &str, args: &[SyntaxNode]) {
        match (name, args) {
            ("to_integer", [value]) => {
                let (Some(kind), Some(width)) = (self.num(value), self.width(value)) else {
                    return;
                };
                let max = match kind {
                    Num::Unsigned => 31,
                    Num::Signed => 32,
                    _ => return,
                };
                if width > max {
                    self.report(
                        codes::CONVERSION_OVERFLOW,
                        node.text_range(),
                        format!(
                            "`to_integer` of a {width}-bit {} value: `integer` holds {max} \
                             bits, so larger values stop the simulation and are cut in synthesis",
                            if kind == Num::Signed {
                                "signed"
                            } else {
                                "unsigned"
                            }
                        ),
                    );
                }
            }
            ("to_unsigned" | "to_signed", [value, size]) => {
                let (Some(v), Some(n)) = (self.c.int(value), self.c.int(size)) else {
                    return;
                };
                if !(1..63).contains(&n) {
                    return;
                }
                let (lo, hi) = if name == "to_unsigned" {
                    (0, (1i64 << n) - 1)
                } else {
                    (-(1i64 << (n - 1)), (1i64 << (n - 1)) - 1)
                };
                if v < lo || v > hi {
                    self.report(
                        codes::CONVERSION_OVERFLOW,
                        node.text_range(),
                        format!(
                            "{v} does not fit in {n} bits ({lo} to {hi}): `{name}` warns in \
                             simulation and keeps the low bits in synthesis"
                        ),
                    );
                }
            }
            _ => {}
        }
    }

    // ---- sema/redundant-conversion ---------------------------------------------

    /// `unsigned(x)` where `x` is already `unsigned`.
    fn same_type_conversion(&mut self, node: &SyntaxNode, args: &[SyntaxNode]) {
        let [value] = args else {
            return;
        };
        let Some(prefix) = node.children().next() else {
            return;
        };
        let Some((mark, decl)) = self.named(&prefix) else {
            return;
        };
        if !matches!(decl.kind, DeclKind::Type | DeclKind::Subtype) {
            return;
        }
        let Some((_, object)) = self.object(value) else {
            return;
        };
        // A slice or an index has its own type; only whole objects.
        if !matches!(
            super::super::unparen(value).kind(),
            SyntaxKind::NAME | SyntaxKind::SELECTED_NAME
        ) {
            return;
        }
        let Some(object_mark) = object.subtype else {
            return;
        };
        let analysis = self.c.analysis;
        // Only the same mark: `std_ulogic(x)` on a `std_logic` is needed in
        // VHDL-93, and a subtype conversion can constrain.
        if analysis.base_type(mark).is_none() || mark != object_mark {
            return;
        }
        let fix = Fix {
            title: "Remove the conversion".into(),
            edits: vec![Edit {
                range: node.text_range(),
                text: self.text(value).to_string(),
            }],
            safe: true,
        };
        self.report_fix(
            codes::REDUNDANT_CONVERSION,
            node.text_range(),
            format!(
                "`{}` is already of type `{}`: the conversion does nothing",
                self.text(value),
                self.text(&prefix).trim()
            ),
            fix,
        );
    }

    /// `to_unsigned(to_integer(x), n)` where `x` is `unsigned`: a
    /// `resize` with the 31-bit limit of `integer`.
    fn round_trip(&mut self, node: &SyntaxNode, name: &str, args: &[SyntaxNode]) {
        let ([inner, size], kind) = (args, name) else {
            return;
        };
        let want = match kind {
            "to_unsigned" => Num::Unsigned,
            "to_signed" => Num::Signed,
            _ => return,
        };
        let inner = super::super::unparen(inner);
        if inner.kind() != SyntaxKind::CALL_EXPR || callee(&inner).as_deref() != Some("to_integer")
        {
            return;
        }
        let [value] = call_args(&inner)
            .try_into()
            .unwrap_or_else(|_| [inner.clone()]);
        if value == inner || self.num(&value) != Some(want) {
            return;
        }
        let fix = Fix {
            title: "Use `resize`".into(),
            edits: vec![Edit {
                range: node.text_range(),
                text: format!("resize({}, {})", self.text(&value), self.text(size)),
            }],
            safe: false,
        };
        self.report_fix(
            codes::REDUNDANT_CONVERSION,
            node.text_range(),
            format!(
                "`{name}(to_integer(...))` converts `{}` to its own type through `integer`, \
                 which limits it to 31 bits: `resize({}, {})` does the same without the limit",
                self.text(&value),
                self.text(&value),
                self.text(size)
            ),
            fix,
        );
    }

    // ---- sema/expensive-operator -------------------------------------------------

    fn expensive(
        &mut self,
        node: &SyntaxNode,
        operator: SyntaxKind,
        left: &SyntaxNode,
        right: &SyntaxNode,
    ) {
        if !self.rtl() || !in_rtl_statement(node) || self.in_simulation_only(node) {
            return;
        }
        if self.is_static(left) && self.is_static(right) {
            return;
        }
        let power_of_two = |v: Option<i64>| v.is_some_and(|v| v > 0 && v & (v - 1) == 0);
        let symbol = match operator {
            SyntaxKind::SLASH => "/",
            SyntaxKind::MOD_KW => "mod",
            SyntaxKind::REM_KW => "rem",
            _ => "**",
        };
        if operator == SyntaxKind::STAR2 {
            // `2 ** n` is a shift, `x ** 2` a multiplication.
            if self.c.int(left) == Some(2) || self.is_static(right) {
                return;
            }
        } else if self.is_static(right) && power_of_two(self.c.int(right)) {
            return;
        }
        let what = if self.is_static(right) {
            "a constant that is not a power of two"
        } else {
            "a value that is not constant"
        };
        self.report(
            codes::EXPENSIVE_OPERATOR,
            node.text_range(),
            format!(
                "`{symbol}` by {what} builds a large combinational divider in one clock cycle: \
                 use a power of two, a pipelined or iterative divider, or a vendor IP"
            ),
        );
    }

    // ---- sema/integer-overflow, sema/power-of-two-wrap -----------------------------

    /// `x <= x + k` and `x := x - k` in a process.
    pub(super) fn counter(&mut self, node: &SyntaxNode) {
        let Some(target) = node.children().next() else {
            return;
        };
        let Some((id, decl)) = self.named(&target) else {
            return;
        };
        if !matches!(
            decl.kind,
            DeclKind::Signal | DeclKind::Variable | DeclKind::Port { .. }
        ) {
            return;
        }
        let value = if node.kind() == SyntaxKind::VARIABLE_ASSIGN_STMT {
            node.children().nth(1)
        } else {
            node.children()
                .find(|n| n.kind() == SyntaxKind::WAVEFORM)
                .and_then(|w| w.children().next())
                .and_then(|e| e.children().next())
        };
        let Some(value) = value.map(|v| super::super::unparen(&v)) else {
            return;
        };
        if value.kind() != SyntaxKind::BINARY_EXPR {
            return;
        }
        let up = match op(&value) {
            Some(SyntaxKind::PLUS) => true,
            Some(SyntaxKind::MINUS) => false,
            _ => return,
        };
        let mut operands = value.children();
        let (Some(a), Some(b)) = (operands.next(), operands.next()) else {
            return;
        };
        let names =
            |n: &SyntaxNode| self.named(&super::super::unparen(n)).map(|(i, _)| i) == Some(id);
        let step = if names(&a) {
            self.c.int(&b)
        } else if up && names(&b) {
            self.c.int(&a)
        } else {
            None
        };
        let Some(step) = step.filter(|s| *s > 0) else {
            return;
        };
        // Subprograms are software more often than hardware: processes only.
        let Some(process) = node
            .ancestors()
            .find(|a| {
                matches!(
                    a.kind(),
                    SyntaxKind::PROCESS_STMT | SyntaxKind::SUBPROGRAM_BODY
                )
            })
            .filter(|a| a.kind() == SyntaxKind::PROCESS_STMT)
        else {
            return;
        };
        if self.guarded(node, &process, id) {
            return;
        }
        if self.h.integer_overflow && self.num_of(decl) == Some(Num::Integer) {
            self.integer_overflow(node, &process, id, &decl.spelling, up, step);
        }
        if self.h.power_of_two_wrap && up && self.num_of(decl) == Some(Num::Unsigned) {
            self.power_of_two_wrap(node, id, &decl.spelling);
        }
    }

    /// Whether a statement of `process` runs under a test of `id`: an
    /// `if`, `elsif`, `case` or loop condition that reads it.
    fn guarded(&self, node: &SyntaxNode, process: &SyntaxNode, id: DeclId) -> bool {
        node.ancestors()
            .take_while(|a| a != process)
            .filter(|a| {
                matches!(
                    a.kind(),
                    SyntaxKind::IF_STMT | SyntaxKind::CASE_STMT | SyntaxKind::LOOP_STMT
                )
            })
            .any(|statement| {
                statement
                    .children()
                    .filter(|c| !super::super::is_statement(c.kind()))
                    .filter(|c| {
                        !matches!(c.kind(), SyntaxKind::CASE_ALTERNATIVE | SyntaxKind::LABEL)
                    })
                    .any(|condition| {
                        self.c
                            .refs_in(condition.text_range())
                            .iter()
                            .any(|r| super::super::single_decl(r) == Some(id))
                    })
            })
    }

    fn integer_overflow(
        &mut self,
        node: &SyntaxNode,
        process: &SyntaxNode,
        id: DeclId,
        name: &str,
        up: bool,
        step: i64,
    ) {
        let Some((lo, hi)) = self.c.values.integer_bounds(id) else {
            return;
        };
        let bound = if up { hi } else { lo };
        let Some(bound) = bound else {
            return;
        };
        // `integer` itself: its bound is not a design choice.
        if bound.unsigned_abs() >= i32::MAX as u64 {
            return;
        }
        let counter = super::counter::Counter {
            node,
            process,
            id,
            lo,
            hi,
            up,
            step,
        };
        if self.cannot_overflow(&counter) {
            return;
        }
        let range = |lo: Option<i64>, hi: Option<i64>| match (lo, hi) {
            (Some(lo), Some(hi)) => format!(" ({lo} to {hi})"),
            _ => String::new(),
        };
        self.report(
            codes::INTEGER_OVERFLOW,
            node.text_range(),
            format!(
                "`{name}` is counted {} by {step} with no test of its value around it: past {bound}{} \
                 the simulation stops with a range error, while the hardware wraps or counts on",
                if up { "up" } else { "down" },
                range(lo, hi)
            ),
        );
    }

    fn power_of_two_wrap(&mut self, node: &SyntaxNode, id: DeclId, name: &str) {
        let Some(decl) = self.c.decl(id) else {
            return;
        };
        let Some(declaration) = self.c.values.declaration(decl) else {
            return;
        };
        // `unsigned(clog2(DEPTH) - 1 downto 0)`.
        let Some(generic) = declaration
            .children()
            .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)
            .and_then(|s| {
                s.descendants()
                    .filter(|n| n.kind() == SyntaxKind::CALL_EXPR)
                    .find(|c| callee(c).is_some_and(|f| f.contains("log2")))
            })
            .and_then(|call| call_args(&call).into_iter().next())
            .and_then(|arg| {
                self.c
                    .values
                    .target(id.unit, crate::resolver::mark_token(&arg)?.text_range())
            })
        else {
            return;
        };
        let Some(g) = self.c.decl(generic) else {
            return;
        };
        if !matches!(g.kind, DeclKind::Generic { .. } | DeclKind::Constant) {
            return;
        }
        if self.asserted(generic) {
            return;
        }
        let value = self.c.values.value(generic, 0);
        let detail = match value {
            Some(v) if v > 0 && v & (v - 1) != 0 => format!(
                ": with `{}` = {v}, `{name}` reaches {} past the last entry {}",
                g.spelling,
                (v as u64).next_power_of_two() - 1,
                v - 1
            ),
            _ => String::new(),
        };
        self.report(
            codes::POWER_OF_TWO_WRAP,
            node.text_range(),
            format!(
                "`{name}` wraps at a power of two, which is `{}` only when `{}` is one, and \
                 nothing checks that it is{detail}; assert it or wrap explicitly",
                g.spelling, g.spelling
            ),
        );
    }

    /// Whether an `assert` in the design units of a generic's entity reads
    /// it.
    fn asserted(&self, generic: DeclId) -> bool {
        let analysis = self.c.analysis;
        let units = [generic.unit, self.c.unit];
        units.iter().any(|&unit| {
            let Some(info) = analysis.units().get(unit as usize) else {
                return false;
            };
            let Some(root) = self.c.values.root(info.file) else {
                return false;
            };
            let Some(design_unit) = root
                .children()
                .filter(|n| n.kind() == SyntaxKind::DESIGN_UNIT)
                .nth(info.index)
            else {
                return false;
            };
            let refs = analysis.unit_references(unit);
            vfl_syntax::green_walk::descendants_where(&design_unit, |k| {
                k == SyntaxKind::ASSERT_STMT
            })
            .iter()
            .any(|a| {
                let range: TextRange = a.text_range();
                refs.iter()
                    .filter(|r| range.contains_range(r.range))
                    .any(|r| super::super::single_decl(r) == Some(generic))
            })
        })
    }
}

/// Whether an expression is in a statement of an architecture's
/// processes or concurrent assignments, rather than in a declaration, a
/// subprogram, an assertion or a generate's scheme.
fn in_rtl_statement(node: &SyntaxNode) -> bool {
    use SyntaxKind::*;
    for a in node.ancestors() {
        match a.kind() {
            SIGNAL_ASSIGN_STMT | VARIABLE_ASSIGN_STMT | SELECTED_ASSIGN_STMT | IF_STMT
            | CASE_STMT => {
                return !a
                    .ancestors()
                    .any(|b| matches!(b.kind(), SUBPROGRAM_BODY | ASSERT_STMT | REPORT_STMT));
            }
            ASSERT_STMT | REPORT_STMT | SUBPROGRAM_BODY | SIGNAL_DECL | VARIABLE_DECL
            | CONSTANT_DECL | FOR_GENERATE_STMT | IF_GENERATE_STMT | LOOP_STMT | WAIT_STMT
            | ARCHITECTURE_BODY => return false,
            _ => {}
        }
    }
    false
}
