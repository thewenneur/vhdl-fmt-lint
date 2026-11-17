//! Checks of a whole architecture, on the graph of its assignments: data
//! and valid outputs with different latencies, `valid` waiting for
//! `ready`, registers whose initial value is not their reset value, bits
//! of vectors nobody reads or drives, and what keeps the target device
//! from inferring its RAM, DSP and shift register blocks.

use rustc_hash::{FxHashMap, FxHashSet};
use vfl_syntax::green_walk::descendants_where;
use vfl_syntax::{SyntaxKind, SyntaxNode, TextRange};

use super::graph::Graph;
use super::{Hazards, call_args, canonical, op};
use crate::decl::{Access, DeclId, DeclKind, Mode};
use crate::diagnostic::codes;

/// The prefix of a name that ends with one of `suffixes`: `m_` for
/// `m_tvalid`, `` for `valid`.
fn prefix_of<'n>(name: &'n str, suffixes: &[&str]) -> Option<&'n str> {
    let lower = name.to_ascii_lowercase();
    suffixes.iter().find_map(|s| {
        lower
            .ends_with(s)
            .then(|| name[..name.len() - s.len()].trim_end_matches('_'))
    })
}

const VALID: &[&str] = &["tvalid", "valid", "vld", "_val"];
const READY: &[&str] = &["tready", "ready", "rdy"];

/// A value as far as comparing initial and reset values goes: `x"00"`,
/// `"0000"` and `(others => '0')` are all zeros.
fn value_key(h: &Hazards<'_, '_, '_>, expr: &SyntaxNode) -> String {
    use SyntaxKind::*;
    let expr = super::super::unparen(expr);
    match expr.kind() {
        LITERAL => {
            let Some(token) = expr.first_token() else {
                return canonical(&expr);
            };
            let text = token.text().to_ascii_lowercase();
            let digits = match token.kind() {
                STRING_LITERAL => text.trim_matches('"').replace('_', ""),
                BIT_STRING_LITERAL => {
                    let q = text.find('"').unwrap_or(0);
                    let base = text[..q].chars().last().unwrap_or('b');
                    let digits = text[q..].trim_matches('"').replace('_', "");
                    let max = match base {
                        'x' => 'f',
                        'o' => '7',
                        _ => '1',
                    };
                    if !digits.is_empty() && digits.chars().all(|c| c == '0') {
                        "0".into()
                    } else if !digits.is_empty() && digits.chars().all(|c| c == max) {
                        "1".into()
                    } else {
                        return text;
                    }
                }
                DECIMAL_LITERAL => return h.c.int(&expr).map_or(text, |v| v.to_string()),
                _ => return text,
            };
            match digits.chars().next() {
                Some(c) if digits.chars().all(|d| d == c) => format!("others '{c}'"),
                _ => text,
            }
        }
        AGGREGATE => {
            let mut elements = expr.children().filter(|n| n.kind() == ELEMENT_ASSOCIATION);
            if let (Some(element), None) = (elements.next(), elements.next())
                && element
                    .children()
                    .find(|n| n.kind() == CHOICES)
                    .is_some_and(|c| {
                        vfl_syntax::green_walk::ChildTokens::has_child(&c, |k| k == OTHERS_KW)
                    })
                && let Some(inner) = element.children().last()
            {
                return format!("others {}", value_key(h, &inner));
            }
            canonical(&expr)
        }
        _ => {
            h.c.int(&expr)
                .map_or_else(|| canonical(&expr), |v| v.to_string())
        }
    }
}

impl Hazards<'_, '_, '_> {
    pub(super) fn design(&mut self, body: &SyntaxNode) {
        // Testbenches are not built into hardware.
        if !self.rtl() {
            return;
        }
        if self.h.unused_bits {
            self.unused_bits(body);
        }
        let h = self.h;
        if !(h.latency_mismatch || h.handshake || h.init_reset_mismatch)
            && h.inference.is_none()
            && h.logic_depth.is_none()
        {
            return;
        }
        let graph = self.graph(body);
        if let Some(entity) = self.entity(body) {
            let ports = self.ports(entity);
            if self.h.latency_mismatch {
                self.latency(&graph, &ports);
            }
            if self.h.handshake {
                self.handshake(&graph, &ports);
            }
        }
        if self.h.init_reset_mismatch {
            self.init_reset(&graph);
        }
        if let Some(device) = &self.h.inference {
            let device = device.clone();
            self.inference(&graph, device.as_ref());
        }
        if self.h.logic_depth.is_some() {
            self.logic_depth(&graph);
        }
    }

    // ---- sema/latency-mismatch -------------------------------------------------

    /// The number of registers on the longest path from the inputs to a
    /// signal; `None` when a path goes through an instance, a loop or
    /// something undriven.
    fn latency_of(
        &self,
        graph: &Graph,
        id: DeclId,
        memo: &mut FxHashMap<DeclId, Option<i64>>,
        busy: &mut FxHashSet<DeclId>,
    ) -> Option<i64> {
        if let Some(&known) = memo.get(&id) {
            return known;
        }
        let decl = self.c.decl(id)?;
        let result = if matches!(decl.kind, DeclKind::Port { mode: Mode::In, .. }) {
            Some(0)
        } else if graph.opaque.contains(&id) || !busy.insert(id) {
            None
        } else {
            let mut result = None;
            let mut known = true;
            for assign in graph.drivers(id) {
                let mut latency = 0;
                for read in self.assign_reads(assign) {
                    if read == id {
                        continue;
                    }
                    match self.latency_of(graph, read, memo, busy) {
                        Some(l) => latency = latency.max(l),
                        None => known = false,
                    }
                }
                // A register that reads itself (a counter, a state) has no
                // fixed latency.
                if assign.clock.is_some() && self.assign_reads(assign).contains(&id) {
                    known = false;
                }
                let latency = latency + i64::from(assign.clock.is_some());
                result = Some(result.map_or(latency, |r: i64| r.max(latency)));
            }
            busy.remove(&id);
            result.filter(|_| known)
        };
        memo.insert(id, result);
        result
    }

    fn latency(&mut self, graph: &Graph, ports: &[(DeclId, &crate::decl::Decl)]) {
        let outputs: Vec<(DeclId, &crate::decl::Decl)> = ports
            .iter()
            .filter(|(_, d)| {
                matches!(
                    d.kind,
                    DeclKind::Port {
                        mode: Mode::Out | Mode::Buffer,
                        ..
                    }
                )
            })
            .copied()
            .collect();
        let mut memo = FxHashMap::default();
        let mut busy = FxHashSet::default();
        for (valid, v) in &outputs {
            let Some(prefix) = prefix_of(&v.spelling, VALID) else {
                continue;
            };
            let Some(valid_latency) = self.latency_of(graph, *valid, &mut memo, &mut busy) else {
                continue;
            };
            for (data, d) in &outputs {
                if data == valid
                    || prefix_of(&d.spelling, READY).is_some()
                    || prefix_of(&d.spelling, VALID).is_some()
                {
                    continue;
                }
                let lower = d.name.as_str();
                let paired = if prefix.is_empty() {
                    lower.contains("data") || lower == "dout" || lower == "q"
                } else {
                    lower.starts_with(&prefix.to_ascii_lowercase())
                };
                if !paired {
                    continue;
                }
                let Some(data_latency) = self.latency_of(graph, *data, &mut memo, &mut busy) else {
                    continue;
                };
                if data_latency == valid_latency {
                    continue;
                }
                let Some(assign) = graph.drivers(*data).last() else {
                    continue;
                };
                let range = assign.statement.text_range();
                self.report(
                    codes::LATENCY_MISMATCH,
                    range,
                    format!(
                        "`{}` is {data_latency} register{} after the inputs but `{}` is \
                         {valid_latency}: the data and its valid arrive on different cycles",
                        d.spelling,
                        if data_latency == 1 { "" } else { "s" },
                        v.spelling
                    ),
                );
            }
        }
    }

    // ---- sema/handshake -----------------------------------------------------------

    fn handshake(&mut self, graph: &Graph, ports: &[(DeclId, &crate::decl::Decl)]) {
        for (valid, v) in ports {
            if !matches!(
                v.kind,
                DeclKind::Port {
                    mode: Mode::Out | Mode::Buffer,
                    ..
                }
            ) || prefix_of(&v.spelling, VALID).is_none()
            {
                continue;
            }
            // What `valid` depends on without a register in between.
            let mut seen = FxHashSet::default();
            let mut stack = vec![*valid];
            let mut found = None;
            while let Some(id) = stack.pop() {
                if !seen.insert(id) {
                    continue;
                }
                for assign in graph.drivers(id).filter(|a| a.clock.is_none()) {
                    for read in self.assign_reads(assign) {
                        if let Some(d) = self.c.decl(read)
                            && matches!(d.kind, DeclKind::Port { mode: Mode::In, .. })
                            && prefix_of(&d.spelling, READY).is_some()
                        {
                            found.get_or_insert((d.spelling.clone(), read));
                        }
                        stack.push(read);
                    }
                }
            }
            let Some((ready, _)) = found else {
                continue;
            };
            let Some(assign) = graph.drivers(*valid).find(|a| a.clock.is_none()) else {
                continue;
            };
            self.report(
                codes::HANDSHAKE,
                assign.statement.text_range(),
                format!(
                    "`{}` depends on `{ready}` through combinational logic: a valid that waits \
                     for ready breaks the AXI handshake rules and can deadlock with a sink that \
                     waits for valid",
                    v.spelling
                ),
            );
        }
    }

    // ---- sema/init-reset-mismatch -----------------------------------------------

    fn init_reset(&mut self, graph: &Graph) {
        let mut reported = FxHashSet::default();
        for assign in graph.assigns.iter().filter(|a| a.reset) {
            let Some(decl) = self.c.decl(assign.target) else {
                continue;
            };
            if decl.kind != DeclKind::Signal || assign.target.unit != self.c.unit {
                continue;
            }
            let Some(declaration) = self.c.values.declaration(decl) else {
                continue;
            };
            let Some(initial) = super::super::initial_expression(&declaration) else {
                continue;
            };
            let [value] = assign.values.as_slice() else {
                continue;
            };
            // Only a whole signal set to a static value.
            if assign.statement.children().next().map(|t| t.kind()) != Some(SyntaxKind::NAME)
                || !self.is_static(value)
                || !self.is_static(&initial)
            {
                continue;
            }
            let (a, b) = (value_key(self, &initial), value_key(self, value));
            if a == b || !reported.insert(assign.target) {
                continue;
            }
            self.report(
                codes::INIT_RESET_MISMATCH,
                assign.statement.text_range(),
                format!(
                    "`{}` starts as `{}` (its initial value) but resets to `{}`: the design is \
                     in a different state after configuration than after a reset",
                    decl.spelling,
                    self.text(&initial).trim(),
                    self.text(value).trim()
                ),
            );
        }
    }

    // ---- sema/unused-bits ----------------------------------------------------------

    fn unused_bits(&mut self, body: &SyntaxNode) {
        let unit = self.c.unit;
        let Some(root) = self.c.values.root(self.c.file) else {
            return;
        };
        // The vectors this architecture declares, and their bits.
        #[allow(clippy::type_complexity)]
        let mut vectors: FxHashMap<
            DeclId,
            (super::super::Bounds, Vec<bool>, Vec<bool>, bool, bool),
        > = FxHashMap::default();
        for (i, decl) in self.c.analysis.unit_decls(unit).iter().enumerate() {
            if decl.kind != DeclKind::Signal || !body.text_range().contains_range(decl.range) {
                continue;
            }
            let id = DeclId {
                unit,
                index: i as u32,
            };
            if !matches!(
                self.num_of(decl),
                Some(super::Num::Unsigned | super::Num::Signed | super::Num::Vector)
            ) {
                continue;
            }
            let Some(bounds) = self.c.values.object_bounds(id) else {
                continue;
            };
            let n = bounds.hi() - bounds.lo() + 1;
            if !(2..=4096).contains(&n) {
                continue;
            }
            let n = n as usize;
            vectors.insert(id, (bounds, vec![false; n], vec![false; n], false, false));
        }
        if vectors.is_empty() {
            return;
        }
        for r in self.c.refs {
            let Some(id) = super::super::single_decl(r) else {
                continue;
            };
            let Some((bounds, read, written, any_read, any_written)) = vectors.get_mut(&id) else {
                continue;
            };
            if !matches!(r.access, Access::Read | Access::Write | Access::ReadWrite) {
                continue;
            }
            // The bits the name covers: all of them, or a static index or
            // slice.
            let bits: Option<(i64, i64)> = (|| {
                let token = root.covering_element(r.range).into_token()?;
                let name = token.parent()?;
                let call = name.parent()?;
                if call.kind() != SyntaxKind::CALL_EXPR || call.children().next()? != name {
                    return Some((bounds.lo(), bounds.hi()));
                }
                let [index] = call_args(&call).try_into().ok()?;
                if index.kind() == SyntaxKind::RANGE_EXPR {
                    let slice = self.c.values.range(&index, unit, 0)?;
                    Some((slice.lo(), slice.hi()))
                } else {
                    let i = self.c.int(&index)?;
                    Some((i, i))
                }
            })();
            // A dynamic index may touch any bit.
            let (lo, hi) = bits.unwrap_or((bounds.lo(), bounds.hi()));
            let mark = |set: &mut Vec<bool>| {
                for i in lo.max(bounds.lo())..=hi.min(bounds.hi()) {
                    set[(i - bounds.lo()) as usize] = true;
                }
            };
            if r.access.reads() {
                mark(read);
                *any_read = true;
            }
            if matches!(r.access, Access::Write | Access::ReadWrite) {
                mark(written);
                *any_written = true;
            }
        }
        let mut found: Vec<(TextRange, String)> = Vec::new();
        for (id, (bounds, read, written, any_read, any_written)) in &vectors {
            let Some(decl) = self.c.decl(*id) else {
                continue;
            };
            for (set, any, what) in [(read, any_read, "read"), (written, any_written, "assigned")] {
                if !*any || set.iter().all(|b| *b) {
                    continue;
                }
                let missing = describe_bits(bounds, set);
                found.push((
                    decl.range,
                    format!(
                        "{missing} of `{}` {} never {what}",
                        decl.spelling,
                        if set.iter().filter(|b| !**b).count() == 1 {
                            "is"
                        } else {
                            "are"
                        }
                    ),
                ));
            }
        }
        found.sort_by_key(|(r, _)| r.start());
        for (range, message) in found {
            self.report(codes::UNUSED_BITS, range, message);
        }
    }

    // ---- vendor/inference -----------------------------------------------------------

    fn inference(&mut self, graph: &Graph, device: Option<&super::Device>) {
        // A memory reset: never a block RAM.
        let mut reported = FxHashSet::default();
        for assign in graph.assigns.iter().filter(|a| a.reset) {
            let Some(decl) = self.c.decl(assign.target) else {
                continue;
            };
            let Some((depth, width)) = self.memory(assign.target, decl) else {
                continue;
            };
            // Small register files are fine in flip-flops.
            if depth < 16 || depth * width < 1024 || !reported.insert(assign.target) {
                continue;
            }
            self.report(
                codes::INFERENCE,
                assign.statement.text_range(),
                format!(
                    "`{}` ({depth} x {width} bits) is reset: block RAM contents cannot be \
                     reset, so it is built from {} flip-flops{}",
                    decl.spelling,
                    depth * width,
                    device.map_or(String::new(), |d| format!(" on {}", d.name))
                ),
            );
        }
        let Some(device) = device else {
            return;
        };
        // Memories read without a clock.
        let mut memories: FxHashMap<DeclId, (i64, i64)> = FxHashMap::default();
        for assign in graph
            .assigns
            .iter()
            .filter(|a| a.clock.is_some() && !a.reset)
        {
            if let Some(decl) = self.c.decl(assign.target)
                && decl.kind == DeclKind::Signal
                && let Some(shape) = self.memory(assign.target, decl)
            {
                memories.insert(assign.target, shape);
            }
        }
        let mut async_reported = FxHashSet::default();
        for assign in graph.assigns.iter().filter(|a| a.clock.is_none()) {
            for value in &assign.values {
                for call in descendants_where(value, |k| k == SyntaxKind::CALL_EXPR) {
                    let Some(prefix) = call.children().next() else {
                        continue;
                    };
                    let Some((id, decl)) = self.object(&prefix) else {
                        continue;
                    };
                    let Some(&(depth, width)) = memories.get(&id) else {
                        continue;
                    };
                    let [index] = call_args(&call)
                        .try_into()
                        .unwrap_or_else(|_| [call.clone()]);
                    if index == call || self.is_static(&index) {
                        continue;
                    }
                    let bits = depth * width;
                    if bits <= i64::from(device.lutram_bits) || !async_reported.insert(id) {
                        continue;
                    }
                    self.report(
                        codes::INFERENCE,
                        call.text_range(),
                        format!(
                            "`{}` ({depth} x {width} bits) is read without a clock: {} block \
                             RAM reads on a clock edge, so it is built from {}; register the \
                             address or the data",
                            decl.spelling,
                            device.name,
                            if device.lutram_bits > 0 {
                                "LUTs (distributed RAM), a lot of them at this size"
                            } else {
                                "flip-flops and multiplexers"
                            }
                        ),
                    );
                }
            }
        }
        // Multiplications wider than one DSP block.
        let mut seen = FxHashSet::default();
        for assign in &graph.assigns {
            for value in &assign.values {
                for product in descendants_where(value, |k| k == SyntaxKind::BINARY_EXPR) {
                    if op(&product) != Some(SyntaxKind::STAR) || !seen.insert(product.text_range())
                    {
                        continue;
                    }
                    let mut operands = product.children();
                    let (Some(a), Some(b)) = (operands.next(), operands.next()) else {
                        continue;
                    };
                    if self.is_static(&a) || self.is_static(&b) {
                        continue;
                    }
                    let (Some(wa), Some(wb)) = (self.width(&a), self.width(&b)) else {
                        continue;
                    };
                    // An unsigned operand needs a sign bit in the DSP.
                    let signed = |e: &SyntaxNode, w: i64| {
                        if self.num(e) == Some(super::Num::Signed) {
                            w
                        } else {
                            w + 1
                        }
                    };
                    let (sa, sb) = (signed(&a, wa), signed(&b, wb));
                    let (big, small) = (sa.max(sb), sa.min(sb));
                    let (d1, d2) = device.dsp;
                    let (dbig, dsmall) = (i64::from(d1.max(d2)), i64::from(d1.min(d2)));
                    if big <= dbig && small <= dsmall {
                        continue;
                    }
                    let blocks = ((big + dbig - 2) / (dbig - 1)).max(1)
                        * ((small + dsmall - 2) / (dsmall - 1)).max(1);
                    self.report(
                        codes::INFERENCE,
                        product.text_range(),
                        format!(
                            "this {wa} x {wb}-bit multiplication is wider than one {} DSP block \
                             ({d1} x {d2} signed): it takes about {blocks} blocks and the logic \
                             to add their results",
                            device.name
                        ),
                    );
                }
            }
        }
        // Shift registers with a reset: no SRL.
        if !device.srl {
            return;
        }
        let reset_targets: FxHashMap<DeclId, TextRange> = graph
            .assigns
            .iter()
            .filter(|a| a.reset)
            .map(|a| (a.target, a.statement.text_range()))
            .collect();
        let mut shift_reported = FxHashSet::default();
        for assign in graph
            .assigns
            .iter()
            .filter(|a| a.clock.is_some() && !a.reset)
        {
            let Some(&reset) = reset_targets.get(&assign.target) else {
                continue;
            };
            let shifts = assign.values.iter().any(|v| {
                let v = super::super::unparen(v);
                v.kind() == SyntaxKind::BINARY_EXPR
                    && op(&v) == Some(SyntaxKind::AMP)
                    && v.children().any(|o| {
                        super::super::unparen(&o).kind() == SyntaxKind::CALL_EXPR
                            && self.object(&o).map(|(id, _)| id) == Some(assign.target)
                    })
            });
            let Some(decl) = self.c.decl(assign.target) else {
                continue;
            };
            // Up to 4 stages is a synchronizer, which belongs in flip-flops.
            if !shifts || decl.width.is_none_or(|w| w <= 4) || !shift_reported.insert(assign.target)
            {
                continue;
            }
            self.report(
                codes::INFERENCE,
                reset,
                format!(
                    "`{}` is a shift register with a reset: {} shift register LUTs (SRL) have no \
                     reset, so its {} stages are built from flip-flops; reset only the first \
                     stage, or none",
                    decl.spelling,
                    device.name,
                    decl.width.unwrap_or_default()
                ),
            );
        }
    }
}

/// The bits of `set` that are not set, as `16`, `3 downto 0` or `4 to 7`,
/// in the declared direction.
fn describe_bits(bounds: &super::super::Bounds, set: &[bool]) -> String {
    let mut ranges: Vec<(i64, i64)> = Vec::new();
    for (i, b) in set.iter().enumerate() {
        if *b {
            continue;
        }
        let i = bounds.lo() + i as i64;
        match ranges.last_mut() {
            Some((_, hi)) if *hi + 1 == i => *hi = i,
            _ => ranges.push((i, i)),
        }
    }
    if !bounds.ascending {
        ranges.reverse();
    }
    let parts: Vec<String> = ranges
        .iter()
        .map(|&(lo, hi)| {
            if lo == hi {
                lo.to_string()
            } else if bounds.ascending {
                format!("{lo} to {hi}")
            } else {
                format!("{hi} downto {lo}")
            }
        })
        .collect();
    let count = set.iter().filter(|b| !**b).count();
    format!(
        "bit{} {}",
        if count == 1 { "" } else { "s" },
        parts.join(", ")
    )
}
