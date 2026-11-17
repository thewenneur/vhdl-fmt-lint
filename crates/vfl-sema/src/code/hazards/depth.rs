//! `sema/logic-depth`: an estimate of the levels of logic between
//! registers, against what the clock period allows.
//!
//! The cost model is coarse, in levels of 6-input LUTs: logic operators
//! pack six inputs a level, an `=` of `w` bits compares three bit pairs a
//! LUT then combines them, adders and `<` ride the carry chain (one level
//! plus one per 16 bits), a multiplier costs six levels, a divider one per
//! bit, a multiplexer by a dynamic index two inputs a level, and each pair
//! of conditions an assignment is under one level of multiplexing.
//! Combinational signals are followed back to the registers and inputs
//! they come from.

use rustc_hash::{FxHashMap, FxHashSet};
use vfl_syntax::{SyntaxKind, SyntaxNode};

use super::graph::Graph;
use super::{Hazards, call_args, callee, op};
use crate::decl::{DeclId, DeclKind};
use crate::diagnostic::codes;

/// The levels of a path, and the combinational signals it goes through.
#[derive(Clone, Default)]
struct Path {
    levels: u32,
    through: Vec<DeclId>,
}

impl Path {
    fn max(self, other: Path) -> Path {
        if other.levels > self.levels {
            other
        } else {
            self
        }
    }

    fn plus(mut self, levels: u32) -> Path {
        self.levels += levels;
        self
    }
}

/// `ceil(log_base(n))`, at least 1 for `n > 1`.
fn log_ceil(n: i64, base: i64) -> u32 {
    let mut levels = 0;
    let mut reach = 1;
    while reach < n {
        reach *= base;
        levels += 1;
    }
    levels
}

struct Depth<'g> {
    graph: &'g Graph,
    memo: FxHashMap<DeclId, Path>,
    busy: FxHashSet<DeclId>,
}

impl Hazards<'_, '_, '_> {
    pub(super) fn logic_depth(&mut self, graph: &Graph) {
        let Some(timing) = self.h.logic_depth.clone() else {
            return;
        };
        if timing.level_ns <= 0.0 {
            return;
        }
        let mut depth = Depth {
            graph,
            memo: FxHashMap::default(),
            busy: FxHashSet::default(),
        };
        // The deepest assignment of each register.
        let mut deepest: FxHashMap<DeclId, (Path, usize)> = FxHashMap::default();
        for (i, assign) in graph.assigns.iter().enumerate() {
            if assign.reset || assign.clock.is_none() {
                continue;
            }
            let path = self.assign_depth(assign, &mut depth);
            let entry = deepest.entry(assign.target).or_insert((Path::default(), i));
            if path.levels > entry.0.levels {
                *entry = (path, i);
            }
        }
        let mut found: Vec<_> = deepest.into_iter().collect();
        found.sort_by_key(|(_, (_, i))| *i);
        for (target, (path, i)) in found {
            let assign = &graph.assigns[i];
            let (Some(target_decl), Some(clock)) = (
                self.c.decl(target),
                assign.clock.and_then(|c| self.c.decl(c)),
            ) else {
                continue;
            };
            let Some(period) = timing.period(&clock.name) else {
                continue;
            };
            let matches = |pattern: &str| {
                super::glob_matches(pattern, &target_decl.name)
                    || super::glob_matches(pattern, &clock.name)
            };
            if timing.false_paths.iter().any(|p| matches(p)) {
                continue;
            }
            let cycles = timing
                .multicycle
                .iter()
                .filter(|(p, _)| matches(p))
                .map(|(_, n)| *n)
                .max()
                .unwrap_or(1)
                .max(1);
            let budget = (period * f64::from(cycles) / timing.level_ns).floor() as u32;
            if path.levels <= budget.max(1) {
                continue;
            }
            let through: Vec<String> = path
                .through
                .iter()
                .filter_map(|id| self.c.decl(*id))
                .map(|d| format!("`{}`", d.spelling))
                .collect();
            let mhz = 1000.0 / period;
            self.report(
                codes::LOGIC_DEPTH,
                assign.statement.text_range(),
                format!(
                    "about {} levels of logic before register `{}`{}: `{}` at {} MHz{} allows \
                     about {budget} ({period} ns / {} ns a level{}); an estimate, the timing \
                     report has the last word",
                    path.levels,
                    target_decl.spelling,
                    if through.is_empty() {
                        String::new()
                    } else {
                        format!(" (through {})", through.join(", "))
                    },
                    clock.spelling,
                    trim(mhz),
                    if cycles > 1 {
                        format!(" over {cycles} cycles")
                    } else {
                        String::new()
                    },
                    trim(timing.level_ns),
                    timing.device.map_or(String::new(), |d| format!(" on {d}"))
                ),
            );
        }
    }

    fn assign_depth(&self, assign: &super::graph::Assign, depth: &mut Depth<'_>) -> Path {
        let mut path = Path::default();
        for value in &assign.values {
            path = path.max(self.expr_depth(value, depth));
        }
        for condition in &assign.conditions {
            path = path.max(self.expr_depth(condition, depth));
        }
        let muxes = (assign.conditions.len() as u32).div_ceil(2);
        path.plus(muxes)
    }

    /// The depth of a combinational signal: of its deepest assignment.
    fn signal_depth(&self, id: DeclId, depth: &mut Depth<'_>) -> Path {
        if let Some(known) = depth.memo.get(&id) {
            return known.clone();
        }
        let graph = depth.graph;
        let drivers: Vec<&super::graph::Assign> = graph.drivers(id).collect();
        if drivers.is_empty() || drivers.iter().any(|a| a.clock.is_some()) || !depth.busy.insert(id)
        {
            return Path::default();
        }
        let mut path = Path::default();
        for assign in drivers {
            path = path.max(self.assign_depth(assign, depth));
        }
        depth.busy.remove(&id);
        if path.levels > 0 {
            path.through.insert(0, id);
        }
        depth.memo.insert(id, path.clone());
        path
    }

    fn expr_depth(&self, expr: &SyntaxNode, depth: &mut Depth<'_>) -> Path {
        use SyntaxKind::*;
        let expr = super::super::unparen(expr);
        match expr.kind() {
            NAME | SELECTED_NAME => match self.object(&expr) {
                Some((id, d)) if matches!(d.kind, DeclKind::Signal | DeclKind::Variable) => {
                    self.signal_depth(id, depth)
                }
                _ => Path::default(),
            },
            UNARY_EXPR | QUALIFIED_EXPR => expr
                .children()
                .last()
                .map(|c| self.expr_depth(&c, depth))
                .unwrap_or_default(),
            AGGREGATE | ELEMENT_ASSOCIATION => expr
                .children()
                .map(|c| self.expr_depth(&c, depth))
                .fold(Path::default(), Path::max),
            CALL_EXPR => self.call_depth(&expr, depth),
            BINARY_EXPR => self.binary_depth(&expr, depth),
            _ => Path::default(),
        }
    }

    fn call_depth(&self, call: &SyntaxNode, depth: &mut Depth<'_>) -> Path {
        let args = call_args(call);
        let deepest = |h: &Self, depth: &mut Depth<'_>| {
            args.iter()
                .map(|a| h.expr_depth(a, depth))
                .fold(Path::default(), Path::max)
        };
        let Some(prefix) = call.children().next() else {
            return Path::default();
        };
        if let Some((id, _)) = self.object(&prefix) {
            let base = self.expr_depth(&prefix, depth);
            let [index] = args.as_slice() else {
                return base;
            };
            if index.kind() == SyntaxKind::RANGE_EXPR || self.is_static(index) {
                return base;
            }
            // A multiplexer by a dynamic index.
            let n = self
                .c
                .values
                .object_bounds(id)
                .map_or(16, |b| b.hi() - b.lo() + 1);
            return base
                .max(self.expr_depth(index, depth))
                .plus(log_ceil(n, 4).max(1));
        }
        let free = matches!(
            callee(call).as_deref(),
            Some(
                "unsigned"
                    | "signed"
                    | "std_logic_vector"
                    | "std_ulogic_vector"
                    | "to_integer"
                    | "to_unsigned"
                    | "to_signed"
                    | "resize"
                    | "to_slv"
                    | "std_logic"
                    | "std_ulogic"
                    | "integer"
                    | "natural"
                    | "rising_edge"
                    | "falling_edge"
            )
        );
        let path = deepest(self, depth);
        if free { path } else { path.plus(2) }
    }

    fn binary_depth(&self, expr: &SyntaxNode, depth: &mut Depth<'_>) -> Path {
        use SyntaxKind::*;
        let Some(operator) = op(expr) else {
            return Path::default();
        };
        let logical =
            |k: SyntaxKind| matches!(k, AND_KW | OR_KW | XOR_KW | NAND_KW | NOR_KW | XNOR_KW);
        if logical(operator) {
            // A tree of logic operators packs six inputs a level.
            let mut leaves = Vec::new();
            let mut stack = vec![expr.clone()];
            while let Some(node) = stack.pop() {
                let node = super::super::unparen(&node);
                if (node.kind() == BINARY_EXPR && op(&node).is_some_and(logical))
                    || (node.kind() == UNARY_EXPR && op(&node) == Some(NOT_KW))
                {
                    stack.extend(node.children());
                } else {
                    leaves.push(node);
                }
            }
            let n = leaves.len() as i64;
            return leaves
                .iter()
                .map(|l| self.expr_depth(l, depth))
                .fold(Path::default(), Path::max)
                .plus(log_ceil(n, 6).max(1));
        }
        let operands: Vec<SyntaxNode> = expr.children().collect();
        let mut path = operands
            .iter()
            .map(|o| self.expr_depth(o, depth))
            .fold(Path::default(), Path::max);
        let width = operands
            .iter()
            .filter_map(|o| self.width(o))
            .max()
            .unwrap_or(1)
            .max(1);
        let statics = operands.iter().filter(|o| self.is_static(o)).count();
        let carry = 1 + (width as u32).div_ceil(16);
        let levels = match operator {
            EQ | NEQ | QEQ | QNEQ => 1 + log_ceil((width + 2) / 3, 6),
            LT | LTE | GT | GTE | PLUS | MINUS => carry,
            STAR if statics > 0 => carry,
            STAR => 6,
            SLASH | MOD_KW | REM_KW => {
                let power = operands
                    .get(1)
                    .and_then(|o| self.c.int(o))
                    .is_some_and(|v| v > 0 && v & (v - 1) == 0);
                if power { 0 } else { width.max(8) as u32 }
            }
            SLL_KW | SRL_KW | SLA_KW | SRA_KW | ROL_KW | ROR_KW => {
                if operands.get(1).is_some_and(|o| self.is_static(o)) {
                    0
                } else {
                    log_ceil(width, 2).div_ceil(2)
                }
            }
            _ => 0,
        };
        if statics == operands.len() {
            return Path::default();
        }
        path.levels += levels;
        path
    }
}

/// A number without trailing zeros: `100`, `2.5`.
fn trim(value: f64) -> String {
    let text = format!("{value:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}
