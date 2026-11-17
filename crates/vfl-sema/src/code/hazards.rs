//! Checks for hardware hazards no other VHDL linter looks for, turned on
//! rule by rule like the other code checks: counters past their bounds
//! (`sema/integer-overflow`, `sema/power-of-two-wrap`), `'-'` compared as
//! a value (`sema/dont-care-compare`), pipelines whose data and valid
//! disagree (`sema/latency-mismatch`, `sema/handshake`), copy-paste slips
//! (`sema/broken-sequence`, `sema/branch-clone`,
//! `sema/swapped-association`), `numeric_std` traps
//! (`sema/signed-resize`, `sema/signed-logical-shift`,
//! `sema/conversion-overflow`, `sema/redundant-conversion`), bits nobody
//! reads or drives (`sema/unused-bits`), testbench processes that restart
//! or hang (`sema/process-restarts`, `sema/unbounded-wait`), operators
//! that build large logic (`sema/expensive-operator`), what keeps a target
//! device from inferring its RAM, DSP and shift register blocks
//! (`vendor/inference`) and paths with more logic levels than the clock
//! period allows (`sema/logic-depth`).
//!
//! Computed on demand, for the files the rules are on for, so that they
//! cost nothing otherwise; each design unit is walked once.

mod depth;
mod design;
mod expr;
mod graph;
mod ports;
mod stmt;

use rustc_hash::FxHashSet;
use vfl_syntax::green_walk::{ChildTokens, descendants_where_green, for_each_token};
use vfl_syntax::{SyntaxKind, SyntaxNode, TextRange};

use super::{Checker, Values};
use crate::analysis::Analysis;
use crate::decl::{Decl, DeclId, DeclKind, FileId, TypeDef};
use crate::diagnostic::{Diagnostic, Fix};
use crate::rtl::reset::{if_branches, process_body};

/// What a target device builds RAM, multipliers and shift registers from,
/// for `vendor/inference`, and how fast its logic is, for
/// `sema/logic-depth`.
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    /// The family, as messages name it: `AMD 7 series`.
    pub name: &'static str,
    /// The widths of the signed operands of one DSP multiplier.
    pub dsp: (u32, u32),
    /// Whether shift registers map to LUT shift registers (SRL), which
    /// have no reset.
    pub srl: bool,
    /// The largest memory worth reading asynchronously (from LUTs), in
    /// bits.
    pub lutram_bits: u32,
    /// The delay of one level of logic with its routing, in ns.
    pub level_ns: f64,
}

/// The families vfl knows, by the prefixes of their names and parts.
const DEVICES: &[(&[&str], Device)] = &[
    (
        &[
            "ultrascale",
            "xcku",
            "xcvu",
            "xczu",
            "xcau",
            "xcve",
            "xcvc",
            "xcvm",
            "xcvp",
            "versal",
            "zynqmp",
            "zynq ultrascale",
            "kintexu",
            "virtexu",
            "artixuplus",
        ],
        Device {
            name: "AMD UltraScale",
            dsp: (27, 18),
            srl: true,
            lutram_bits: 8192,
            level_ns: 0.35,
        },
    ),
    (
        &[
            "7 series",
            "7series",
            "artix",
            "kintex",
            "virtex7",
            "virtex-7",
            "spartan7",
            "spartan-7",
            "zynq",
            "xc7",
        ],
        Device {
            name: "AMD 7 series",
            dsp: (25, 18),
            srl: true,
            lutram_bits: 4096,
            level_ns: 0.5,
        },
    ),
    (
        &["spartan6", "spartan-6", "virtex6", "virtex-6", "xc6"],
        Device {
            name: "AMD Spartan-6/Virtex-6",
            dsp: (18, 18),
            srl: true,
            lutram_bits: 2048,
            level_ns: 0.7,
        },
    ),
    (
        &[
            "agilex",
            "stratix 10",
            "stratix10",
            "arria 10",
            "arria10",
            "10ax",
            "10gx",
            "1s",
            "ag",
        ],
        Device {
            name: "Intel Arria 10/Stratix 10/Agilex",
            dsp: (27, 27),
            srl: false,
            lutram_bits: 640,
            level_ns: 0.45,
        },
    ),
    (
        &[
            "cyclone v",
            "cyclonev",
            "cyclone 10 gx",
            "arria v",
            "arriav",
            "stratix v",
            "stratixv",
            "5c",
            "5a",
            "5s",
        ],
        Device {
            name: "Intel Cyclone V/Arria V",
            dsp: (27, 27),
            srl: false,
            lutram_bits: 640,
            level_ns: 0.7,
        },
    ),
    (
        &[
            "cyclone iv",
            "cycloneiv",
            "cyclone 10 lp",
            "cyclone10lp",
            "max 10",
            "max10",
            "10m",
            "10cl",
            "ep4c",
        ],
        Device {
            name: "Intel Cyclone IV/MAX 10",
            dsp: (18, 18),
            srl: false,
            lutram_bits: 0,
            level_ns: 0.9,
        },
    ),
    (
        &["ice40", "ice5", "ice40up", "ice40hx", "ice40lp"],
        Device {
            name: "Lattice iCE40",
            dsp: (16, 16),
            srl: false,
            lutram_bits: 0,
            level_ns: 1.2,
        },
    ),
    (
        &[
            "ecp5",
            "lfe5",
            "nexus",
            "certus",
            "crosslink",
            "lifcl",
            "lfcpnx",
            "lfd2nx",
            "ecp3",
            "machxo",
            "lcmxo",
        ],
        Device {
            name: "Lattice ECP5/Nexus",
            dsp: (18, 18),
            srl: false,
            lutram_bits: 1024,
            level_ns: 0.9,
        },
    ),
    (
        &[
            "polarfire",
            "mpf",
            "smartfusion",
            "igloo",
            "m2s",
            "m2gl",
            "rtg4",
            "microchip",
        ],
        Device {
            name: "Microchip PolarFire/SmartFusion2",
            dsp: (18, 18),
            srl: false,
            lutram_bits: 768,
            level_ns: 0.8,
        },
    ),
    (
        &["gowin", "gw1n", "gw2a", "gw5a", "littlebee", "arora"],
        Device {
            name: "Gowin",
            dsp: (18, 18),
            srl: false,
            lutram_bits: 1024,
            level_ns: 1.2,
        },
    ),
];

impl Device {
    /// The device of a family or part name (`artix7`, `xc7a35tcpg236-1`,
    /// `Cyclone V`, `5CSEMA5F31C6`, `LFE5U-85F`, `GW1NR-9`): the first
    /// family one of whose names it starts with, ignoring case, `-` and
    /// `_`.
    pub fn named(name: &str) -> Option<Device> {
        let key: String = name
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|c| !matches!(c, '-' | '_'))
            .collect();
        let squeezed: String = key.chars().filter(|c| *c != ' ').collect();
        DEVICES
            .iter()
            .find(|(names, _)| {
                names.iter().any(|n| {
                    let n: String = n.chars().filter(|c| !matches!(c, '-' | '_')).collect();
                    key.starts_with(&n) || squeezed.starts_with(&n.replace(' ', ""))
                })
            })
            .map(|(_, device)| device.clone())
    }
}

/// The clocks of a design and what `sema/logic-depth` allows between
/// registers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Timing {
    /// The period of each clock in ns, by the lower-case name of the
    /// signal or port that carries it.
    pub clocks: Vec<(String, f64)>,
    /// The delay of one level of logic, in ns.
    pub level_ns: f64,
    /// The family that delay is for, for messages.
    pub device: Option<&'static str>,
    /// Multicycle paths: a lower-case glob on register or clock names,
    /// and how many periods paths to them have.
    pub multicycle: Vec<(String, u32)>,
    /// False paths: lower-case globs on register or clock names.
    pub false_paths: Vec<String>,
}

impl Timing {
    /// The period of a clock, in ns.
    pub fn period(&self, clock: &str) -> Option<f64> {
        self.clocks
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(clock))
            .map(|(_, p)| *p)
    }
}

/// Whether `name` matches a glob (`*`, `?`, `*` matching `/` too),
/// ignoring case.
pub fn glob(pattern: &str, name: &str) -> bool {
    fn matches(p: &[u8], n: &[u8]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some(b'*'), _) => matches(&p[1..], n) || (!n.is_empty() && matches(p, &n[1..])),
            (Some(b'?'), Some(_)) => matches(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => matches(&p[1..], &n[1..]),
            _ => false,
        }
    }
    matches(pattern.as_bytes(), name.as_bytes())
}

/// Whether `name` matches a glob (`*`, `?`), ignoring case. Hierarchical
/// names match by their last part as well: `u0/cnt_reg*` matches `cnt`.
pub fn glob_matches(pattern: &str, name: &str) -> bool {
    let matches =
        |p: &[u8], n: &[u8]| glob(&String::from_utf8_lossy(p), &String::from_utf8_lossy(n));
    let pattern = pattern.trim_matches(|c| matches!(c, '{' | '}' | '"' | ' '));
    let last = pattern.rsplit(['/', '|', '.']).next().unwrap_or(pattern);
    [pattern, last].iter().any(|p| {
        matches(p.as_bytes(), name.as_bytes())
            || matches(p.as_bytes(), format!("{name}_reg").as_bytes())
            || p.strip_suffix("_reg*")
                .or_else(|| p.strip_suffix("_reg"))
                .is_some_and(|p| matches(p.as_bytes(), name.as_bytes()))
    })
}

/// Which hazard checks to run, with their options.
#[derive(Debug, Clone, Default)]
pub struct HazardChecks {
    /// `sema/integer-overflow`.
    pub integer_overflow: bool,
    /// `sema/dont-care-compare`.
    pub dont_care_compare: bool,
    /// `sema/latency-mismatch`.
    pub latency_mismatch: bool,
    /// `sema/handshake`.
    pub handshake: bool,
    /// `sema/power-of-two-wrap`.
    pub power_of_two_wrap: bool,
    /// `sema/init-reset-mismatch`.
    pub init_reset_mismatch: bool,
    /// `sema/broken-sequence`.
    pub broken_sequence: bool,
    /// `sema/branch-clone`.
    pub branch_clone: bool,
    /// `sema/signed-resize`.
    pub signed_resize: bool,
    /// `sema/signed-logical-shift`.
    pub signed_logical_shift: bool,
    /// `sema/conversion-overflow`.
    pub conversion_overflow: bool,
    /// `sema/unused-bits`.
    pub unused_bits: bool,
    /// `sema/process-restarts`.
    pub process_restarts: bool,
    /// `sema/unbounded-wait`.
    pub unbounded_wait: bool,
    /// `sema/redundant-conversion`.
    pub redundant_conversion: bool,
    /// `sema/swapped-association`.
    pub swapped_association: bool,
    /// `sema/expensive-operator`.
    pub expensive_operator: bool,
    /// `vendor/inference`, for a device; the memory reset is checked for
    /// any device.
    pub inference: Option<Option<Device>>,
    /// `sema/logic-depth`, with the clocks.
    pub logic_depth: Option<Timing>,
}

impl HazardChecks {
    fn any(&self) -> bool {
        self.expressions() || self.statements() || self.design()
    }

    /// Checks of single expressions.
    fn expressions(&self) -> bool {
        self.dont_care_compare
            || self.signed_resize
            || self.signed_logical_shift
            || self.conversion_overflow
            || self.redundant_conversion
            || self.expensive_operator
            || self.integer_overflow
            || self.power_of_two_wrap
    }

    /// Checks of statements and their neighbors.
    fn statements(&self) -> bool {
        self.broken_sequence
            || self.branch_clone
            || self.process_restarts
            || self.unbounded_wait
            || self.swapped_association
    }

    /// Checks of a whole architecture.
    fn design(&self) -> bool {
        self.latency_mismatch
            || self.handshake
            || self.init_reset_mismatch
            || self.unused_bits
            || self.inference.is_some()
            || self.logic_depth.is_some()
    }
}

impl Analysis {
    /// The findings of the hazard checks of `checks` in a file.
    pub fn hazard_checks(&self, file: FileId, checks: &HazardChecks) -> Vec<Diagnostic> {
        let Some(analyzed) = self.files().get(file as usize) else {
            return Vec::new();
        };
        if analyzed.builtin || analyzed.foreign.is_some() || !checks.any() {
            return Vec::new();
        }
        let root = analyzed.syntax();
        let design_units: Vec<SyntaxNode> = root
            .children()
            .filter(|n| n.kind() == SyntaxKind::DESIGN_UNIT)
            .collect();
        let values = Values::new(self, true);
        values.roots.borrow_mut().insert(file, root.clone());
        let code = super::CodeChecks::default();
        let mut sink = Vec::new();
        let mut out = Vec::new();
        let mut seen = FxHashSet::default();
        for unit in self.file_units(file) {
            let info = &self.units()[unit as usize];
            if !seen.insert(info.index) {
                continue;
            }
            let Some(node) = design_units.get(info.index) else {
                continue;
            };
            let mut checker = Checker {
                analysis: self,
                file,
                unit,
                text: &analyzed.text,
                refs: self.unit_references(unit),
                checks: &code,
                values: &values,
                rtl: None,
                decls_at: std::cell::OnceCell::new(),
                simulation_only: Vec::new(),
                out: &mut sink,
            };
            if info.kind == DeclKind::Architecture {
                checker.rtl = Some(checker.is_rtl(node));
            }
            let mut h = Hazards {
                c: &checker,
                h: checks,
                kind: info.kind,
                out: Vec::new(),
                simulation_only: std::cell::OnceCell::new(),
            };
            h.run(node);
            out.extend(h.out);
        }
        out.sort_by_key(|d| (d.range.start(), d.code));
        out.dedup_by(|a, b| a.range == b.range && a.code == b.code);
        out
    }
}

/// The hazard checks of one design unit.
struct Hazards<'x, 'a, 'v> {
    c: &'x Checker<'a, 'v>,
    h: &'x HazardChecks,
    kind: DeclKind,
    out: Vec<Diagnostic>,
    /// The `translate_off` regions of the unit, found when first needed.
    simulation_only: std::cell::OnceCell<Vec<TextRange>>,
}

/// What numeric type an expression has, as far as the checks care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Num {
    Unsigned,
    Signed,
    /// `std_logic_vector`, `std_ulogic_vector`, `bit_vector`.
    Vector,
    Integer,
}

impl<'a> Hazards<'_, 'a, '_> {
    fn report(&mut self, code: &'static str, range: TextRange, message: String) {
        self.out.push(Diagnostic {
            code,
            file: self.c.file,
            range,
            message,
            fix: None,
        });
    }

    fn report_fix(&mut self, code: &'static str, range: TextRange, message: String, fix: Fix) {
        self.out.push(Diagnostic {
            code,
            file: self.c.file,
            range,
            message,
            fix: Some(fix),
        });
    }

    fn text(&self, node: &SyntaxNode) -> &'a str {
        let range = node.text_range();
        &self.c.text[usize::from(range.start())..usize::from(range.end())]
    }

    fn line(&self, range: TextRange) -> u32 {
        self.c.line(range)
    }

    fn rtl(&self) -> bool {
        self.c.rtl == Some(true)
    }

    fn in_simulation_only(&self, node: &SyntaxNode) -> bool {
        let range = node.text_range();
        self.simulation_only
            .get_or_init(|| {
                node.ancestors()
                    .find(|a| a.kind() == SyntaxKind::DESIGN_UNIT)
                    .map_or_else(Vec::new, |unit| super::simulation_only(&unit))
            })
            .iter()
            .any(|r| r.contains_range(range))
    }

    fn run(&mut self, unit: &SyntaxNode) {
        // `x <= x + k`, `x := x - k`: a sum or difference assigned.
        fn counts(g: super::Green<'_>) -> bool {
            use SyntaxKind::*;
            let value = if g.kind() == VARIABLE_ASSIGN_STMT {
                g.nodes().nth(1)
            } else {
                g.nodes()
                    .find(|n| n.kind() == WAVEFORM)
                    .and_then(|w| w.nodes().next())
                    .and_then(|e| e.nodes().next())
            };
            value.is_some_and(|v| {
                v.kind() == BINARY_EXPR
                    && v.tokens()
                        .find(|(k, _)| !k.is_trivia())
                        .is_some_and(|(k, _)| matches!(k, PLUS | MINUS))
            })
        }
        use SyntaxKind::*;
        let h = self.h;
        let expressions = h.expressions();
        let c = self.c;
        let wanted = |g: super::Green<'_>| match g.kind() {
            BINARY_EXPR => {
                expressions
                    && g.tokens()
                        .find(|(k, _)| !k.is_trivia())
                        .is_some_and(|(k, _)| match k {
                            EQ | NEQ => h.dont_care_compare,
                            SRL_KW => h.signed_logical_shift,
                            SLASH | MOD_KW | REM_KW | STAR2 => h.expensive_operator,
                            _ => false,
                        })
            }
            // Not an index or a slice of an object.
            CALL_EXPR => {
                (h.signed_resize || h.conversion_overflow || h.redundant_conversion)
                    && !g
                        .nodes()
                        .next()
                        .and_then(|p| c.green_named(p))
                        .is_some_and(|(_, d)| d.kind.is_object())
            }
            SIGNAL_ASSIGN_STMT | VARIABLE_ASSIGN_STMT => {
                (h.integer_overflow || h.power_of_two_wrap) && counts(g)
            }
            CASE_STMT | SELECTED_ASSIGN_STMT => h.dont_care_compare || h.branch_clone,
            IF_STMT => h.branch_clone,
            PROCESS_STMT => h.process_restarts,
            WAIT_STMT => h.unbounded_wait,
            ASSOCIATION_LIST => h.broken_sequence || h.swapped_association,
            ARCHITECTURE_BODY | CASE_ALTERNATIVE | LOOP_STMT | BLOCK_STMT | FOR_GENERATE_STMT
            | IF_GENERATE_STMT | SUBPROGRAM_BODY => h.broken_sequence,
            _ => false,
        };
        let per_node = expressions
            || h.signed_resize
            || h.conversion_overflow
            || h.redundant_conversion
            || h.integer_overflow
            || h.power_of_two_wrap
            || h.branch_clone
            || h.process_restarts
            || h.unbounded_wait
            || h.broken_sequence
            || h.swapped_association;
        let nodes = if per_node {
            descendants_where_green(unit, wanted)
        } else {
            Vec::new()
        };
        for node in &nodes {
            match node.kind() {
                BINARY_EXPR => self.binary(node),
                CALL_EXPR => self.call(node),
                SIGNAL_ASSIGN_STMT | VARIABLE_ASSIGN_STMT => self.counter(node),
                CASE_STMT | SELECTED_ASSIGN_STMT => {
                    if h.dont_care_compare {
                        self.dont_care_choices(node);
                    }
                    if h.branch_clone && node.kind() == CASE_STMT {
                        self.case_clones(node);
                    }
                }
                IF_STMT => self.if_clones(node),
                PROCESS_STMT => self.process_restarts(node),
                WAIT_STMT => self.unbounded_wait(node),
                ASSOCIATION_LIST => {
                    if h.broken_sequence {
                        self.association_sequence(node);
                    }
                    if h.swapped_association {
                        self.swapped(node);
                    }
                }
                _ => {}
            }
            if h.broken_sequence
                && matches!(
                    node.kind(),
                    ARCHITECTURE_BODY
                        | PROCESS_STMT
                        | IF_STMT
                        | CASE_ALTERNATIVE
                        | LOOP_STMT
                        | BLOCK_STMT
                        | FOR_GENERATE_STMT
                        | IF_GENERATE_STMT
                        | SUBPROGRAM_BODY
                )
            {
                self.statement_sequence(node);
            }
        }
        if h.design()
            && self.kind == DeclKind::Architecture
            && let Some(body) = unit.children().find(|n| n.kind() == ARCHITECTURE_BODY)
        {
            self.design(&body);
        }
    }

    // ---- Names and types ----------------------------------------------------

    /// The single declaration a name (`s`, `pkg.s`) refers to.
    fn named(&self, node: &SyntaxNode) -> Option<(DeclId, &'a Decl)> {
        self.c.named(node)
    }

    /// The object a name, an index or a slice of it names: `s`, `s(3)`,
    /// `s(7 downto 0)`.
    fn object(&self, node: &SyntaxNode) -> Option<(DeclId, &'a Decl)> {
        let node = super::unparen(node);
        match node.kind() {
            SyntaxKind::NAME | SyntaxKind::SELECTED_NAME => {
                self.named(&node).filter(|(_, d)| d.kind.is_object())
            }
            SyntaxKind::CALL_EXPR => self.object(&node.children().next()?),
            _ => None,
        }
    }

    /// The names of the type marks from an object's subtype to its type.
    fn type_names(&self, decl: &Decl) -> Vec<&'a str> {
        let mut out = Vec::new();
        let mut mark = decl.subtype;
        for _ in 0..16 {
            let Some(d) = mark.and_then(|m| self.c.decl(m)) else {
                break;
            };
            out.push(d.name.as_str());
            if d.kind == DeclKind::Type {
                break;
            }
            mark = d.subtype;
        }
        out
    }

    /// The numeric kind of a type mark's names.
    fn num_of_names(names: &[&str]) -> Option<Num> {
        names.iter().find_map(|n| match *n {
            "unsigned" | "unresolved_unsigned" | "u_unsigned" => Some(Num::Unsigned),
            "signed" | "unresolved_signed" | "u_signed" => Some(Num::Signed),
            "std_logic_vector" | "std_ulogic_vector" | "bit_vector" => Some(Num::Vector),
            "integer" | "natural" | "positive" => Some(Num::Integer),
            _ => None,
        })
    }

    /// The numeric kind of an object.
    fn num_of(&self, decl: &Decl) -> Option<Num> {
        Self::num_of_names(&self.type_names(decl)).or_else(|| {
            decl.subtype
                .is_some_and(|s| self.c.values.is_integer(s))
                .then_some(Num::Integer)
        })
    }

    /// The numeric kind of an expression: of a name, an index or slice,
    /// a conversion or a `numeric_std` function.
    fn num(&self, expr: &SyntaxNode) -> Option<Num> {
        use SyntaxKind::*;
        let expr = super::unparen(expr);
        match expr.kind() {
            NAME | SELECTED_NAME => self.object(&expr).and_then(|(_, d)| self.num_of(d)),
            CALL_EXPR => {
                let prefix = expr.children().next()?;
                if let Some((_, d)) = self.object(&prefix) {
                    // A slice keeps the type, an index gives an element.
                    let args = call_args(&expr);
                    return (args.len() == 1 && args[0].kind() == RANGE_EXPR)
                        .then(|| self.num_of(d))
                        .flatten();
                }
                let name = callee(&expr)?;
                match name.as_str() {
                    "unsigned" | "to_unsigned" => Some(Num::Unsigned),
                    "signed" | "to_signed" => Some(Num::Signed),
                    "std_logic_vector" | "std_ulogic_vector" | "to_slv" => Some(Num::Vector),
                    "to_integer" => Some(Num::Integer),
                    "resize" | "shift_left" | "shift_right" | "rotate_left" | "rotate_right" => {
                        self.num(call_args(&expr).first()?)
                    }
                    _ => None,
                }
            }
            BINARY_EXPR => {
                let mut operands = expr.children();
                let left = operands.next()?;
                let right = operands.next()?;
                self.num(&left).or_else(|| self.num(&right))
            }
            _ => None,
        }
    }

    /// The static number of elements of a vector expression.
    fn width(&self, expr: &SyntaxNode) -> Option<i64> {
        use SyntaxKind::*;
        let expr = super::unparen(expr);
        match expr.kind() {
            NAME | SELECTED_NAME => {
                let (id, d) = self.object(&expr)?;
                d.width
                    .map(i64::from)
                    .or_else(|| self.c.values.object_bounds(id).map(|b| b.hi() - b.lo() + 1))
            }
            LITERAL => crate::rtl::literal_width(&expr.first_token()?).map(i64::from),
            CALL_EXPR => {
                let args = call_args(&expr);
                let prefix = expr.children().next()?;
                if self.object(&prefix).is_some() {
                    let [index] = args.as_slice() else {
                        return None;
                    };
                    if index.kind() == RANGE_EXPR {
                        let b = self.c.values.range(index, self.c.unit, 0)?;
                        return Some((b.hi() - b.lo() + 1).max(0));
                    }
                    return None;
                }
                match callee(&expr)?.as_str() {
                    "resize" | "to_unsigned" | "to_signed" => self.c.int(args.get(1)?),
                    "unsigned" | "signed" | "std_logic_vector" | "std_ulogic_vector"
                    | "shift_left" | "shift_right" | "rotate_left" | "rotate_right" => {
                        self.width(args.first()?)
                    }
                    _ => None,
                }
            }
            BINARY_EXPR => {
                let mut operands = expr.children();
                let left = operands.next()?;
                let right = operands.next();
                match op(&expr)? {
                    AMP => Some(self.width(&left)? + self.width(&right?)?),
                    PLUS | MINUS => {
                        let l = self.width(&left);
                        let r = right.and_then(|r| self.width(&r));
                        l.max(r)
                    }
                    STAR => Some(self.width(&left)? + self.width(&right?)?),
                    AND_KW | OR_KW | XOR_KW | NAND_KW | NOR_KW | XNOR_KW => self
                        .width(&left)
                        .or_else(|| right.and_then(|r| self.width(&r))),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Whether an expression is a static value: no signals or variables.
    fn is_static(&self, expr: &SyntaxNode) -> bool {
        self.c.refs_in(expr.text_range()).iter().all(|r| {
            super::single_decl(r)
                .and_then(|id| self.c.decl(id))
                .is_none_or(|d| {
                    !matches!(
                        d.kind,
                        DeclKind::Signal | DeclKind::Variable | DeclKind::Port { .. }
                    )
                })
        })
    }

    /// The signals, ports and variables an expression reads.
    fn reads(&self, range: TextRange, out: &mut Vec<DeclId>) {
        for r in self.c.refs_in(range) {
            if !r.access.reads() {
                continue;
            }
            if let Some(id) = super::single_decl(r)
                && self.c.decl(id).is_some_and(|d| {
                    matches!(
                        d.kind,
                        DeclKind::Signal | DeclKind::Variable | DeclKind::Port { .. }
                    )
                })
                && !out.contains(&id)
            {
                out.push(id);
            }
        }
    }

    /// The architecture's entity.
    fn entity(&self, body: &SyntaxNode) -> Option<&'a Decl> {
        body.children()
            .find(|n| n.kind() == SyntaxKind::NAME)
            .and_then(|n| self.named(&n))
            .map(|(_, d)| d)
            .filter(|d| d.kind == DeclKind::Entity)
    }

    /// The ports of an entity.
    fn ports(&self, entity: &Decl) -> Vec<(DeclId, &'a Decl)> {
        entity
            .members
            .as_ref()
            .map(|m| {
                m.iter()
                    .filter_map(|id| Some((id, self.c.decl(id)?)))
                    .filter(|(_, d)| matches!(d.kind, DeclKind::Port { .. }))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The depth and width of a memory: a signal of an array type whose
    /// elements are vectors, both with static bounds.
    fn memory(&self, id: DeclId, decl: &Decl) -> Option<(i64, i64)> {
        let Some(TypeDef::Array {
            element: Some(_), ..
        }) = self.c.values.object_type(decl)
        else {
            return None;
        };
        let depth = self.c.values.object_bounds(id)?;
        let depth = depth.hi() - depth.lo() + 1;
        // The element's width, from the array type's declaration.
        let mut mark = decl.subtype?;
        for _ in 0..8 {
            let d = self.c.decl(mark)?;
            if d.kind == DeclKind::Type {
                let node = self.c.values.declaration(d)?;
                let def = node
                    .children()
                    .find(|n| n.kind() == SyntaxKind::ARRAY_TYPE_DEF)?;
                let element = def
                    .children()
                    .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)?;
                let width = self.indication_width(&element, mark.unit)?;
                return (depth > 0 && width > 1).then_some((depth, width));
            }
            mark = d.subtype?;
        }
        None
    }

    /// The width of a constrained vector subtype indication:
    /// `std_logic_vector(7 downto 0)`, or a subtype with a static width.
    fn indication_width(&self, indication: &SyntaxNode, unit: crate::decl::UnitId) -> Option<i64> {
        let mark = indication.children().find(|n| {
            matches!(
                n.kind(),
                SyntaxKind::CALL_EXPR | SyntaxKind::NAME | SyntaxKind::SELECTED_NAME
            )
        })?;
        if mark.kind() == SyntaxKind::CALL_EXPR {
            let [range] = call_args(&mark).try_into().ok()?;
            let b = self.c.values.range(&range, unit, 0)?;
            return Some(b.hi() - b.lo() + 1);
        }
        let token = crate::resolver::mark_token(&mark)?;
        let id = self.c.values.target(unit, token.text_range())?;
        self.c.decl(id)?.width.map(i64::from)
    }
}

// ---- Syntax helpers ---------------------------------------------------------

/// The arguments of a call, positional or named.
fn call_args(call: &SyntaxNode) -> Vec<SyntaxNode> {
    call.children()
        .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)
        .map(|list| {
            list.children()
                .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
                .filter_map(|e| e.children().last())
                .collect()
        })
        .unwrap_or_default()
}

/// The lower-case simple name a call's prefix ends with: `resize` for
/// `ieee.numeric_std.resize(...)`.
fn callee(call: &SyntaxNode) -> Option<String> {
    let prefix = call.children().next()?;
    let token = crate::resolver::mark_token(&prefix)?;
    Some(token.text().to_ascii_lowercase())
}

/// A hash of what [`canonical`] gives for `nodes` one after the other,
/// without building the string.
fn fingerprint<'n>(nodes: impl IntoIterator<Item = &'n SyntaxNode>) -> u64 {
    use std::hash::Hasher;
    let mut hasher = rustc_hash::FxHasher::default();
    for node in nodes {
        for_each_token(node, |kind, _, text| {
            if kind.is_trivia() {
                return;
            }
            hasher.write_u16(kind as u16);
            if matches!(
                kind,
                SyntaxKind::STRING_LITERAL | SyntaxKind::CHAR_LITERAL | SyntaxKind::EXT_IDENT
            ) {
                hasher.write(text.as_bytes());
            } else {
                for b in text.bytes() {
                    hasher.write_u8(b.to_ascii_lowercase());
                }
            }
            hasher.write_u8(0xff);
        });
    }
    hasher.finish()
}

/// A node's text without trivia, its words in lower case, for comparing
/// code: `A  and b` and `a AND b` are the same.
fn canonical(node: &SyntaxNode) -> String {
    let mut out = String::new();
    for_each_token(node, |kind, _, text| {
        if kind.is_trivia() {
            return;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        if matches!(
            kind,
            SyntaxKind::STRING_LITERAL | SyntaxKind::CHAR_LITERAL | SyntaxKind::EXT_IDENT
        ) {
            out.push_str(text);
        } else {
            out.push_str(&text.to_ascii_lowercase());
        }
    });
    out
}

/// Whether a condition is a clock edge: `rising_edge(clk)`,
/// `clk'event and clk = '1'`.
fn is_edge(cond: &SyntaxNode) -> bool {
    let cond = super::unparen(cond);
    crate::resolver::is_clock_edge(&cond)
        || (cond.kind() == SyntaxKind::BINARY_EXPR && cond.children().any(|c| is_edge(&c)))
}

/// Whether a condition tests a reset signal: `rst = '1'`, `not rst_n`,
/// `rst`.
fn is_reset_test(cond: &SyntaxNode) -> bool {
    use SyntaxKind::*;
    let cond = super::unparen(cond);
    match cond.kind() {
        NAME => crate::resolver::mark_token(&cond)
            .is_some_and(|t| crate::resets::is_reset_name(t.text())),
        UNARY_EXPR => cond.children().next().is_some_and(|c| is_reset_test(&c)),
        BINARY_EXPR => {
            matches!(op(&cond), Some(EQ | NEQ))
                && cond
                    .children()
                    .any(|c| c.kind() == NAME && is_reset_test(&c))
        }
        _ => false,
    }
}

/// The parts of a clocked process: the statements that run on a reset,
/// those that run on the clock edge, and the edge's condition.
struct Clocked {
    reset: Vec<SyntaxNode>,
    clocked: Vec<SyntaxNode>,
    edge: SyntaxNode,
}

/// The parts of a process, if it is clocked: `if rst then ... elsif
/// rising_edge(clk) then ... end if;`, `if rising_edge(clk) then if rst
/// then ... else ... end if; end if;` or `wait until rising_edge(clk);`
/// first.
fn clocked(process: &SyntaxNode) -> Option<Clocked> {
    let body = process_body(process);
    if let Some(wait) = body.first().filter(|s| s.kind() == SyntaxKind::WAIT_STMT) {
        let until = wait
            .children()
            .find(|n| !matches!(n.kind(), SyntaxKind::LABEL))?;
        if !is_edge(&until) {
            return None;
        }
        return Some(Clocked {
            reset: Vec::new(),
            clocked: body[1..].to_vec(),
            edge: until,
        });
    }
    let [statement] = body.as_slice() else {
        return None;
    };
    if statement.kind() != SyntaxKind::IF_STMT {
        return None;
    }
    let branches = if_branches(statement);
    match branches.as_slice() {
        [(Some(edge), statements)] if is_edge(edge) => {
            if let [inner] = statements.as_slice()
                && inner.kind() == SyntaxKind::IF_STMT
            {
                let inner_branches = if_branches(inner);
                if let Some((Some(test), reset)) = inner_branches.first()
                    && is_reset_test(test)
                {
                    return Some(Clocked {
                        reset: reset.clone(),
                        clocked: inner_branches[1..]
                            .iter()
                            .flat_map(|(_, s)| s.iter().cloned())
                            .collect(),
                        edge: edge.clone(),
                    });
                }
            }
            Some(Clocked {
                reset: Vec::new(),
                clocked: statements.clone(),
                edge: edge.clone(),
            })
        }
        [(Some(test), reset), (Some(edge), statements)] if is_reset_test(test) && is_edge(edge) => {
            Some(Clocked {
                reset: reset.clone(),
                clocked: statements.clone(),
                edge: edge.clone(),
            })
        }
        _ => None,
    }
}

/// The signal a clock edge is of: `clk` in `rising_edge(clk)`.
fn edge_signal(edge: &SyntaxNode) -> Option<SyntaxNode> {
    let edge = super::unparen(edge);
    match edge.kind() {
        SyntaxKind::CALL_EXPR => call_args(&edge).into_iter().next(),
        SyntaxKind::ATTRIBUTE_NAME => edge.children().next(),
        SyntaxKind::BINARY_EXPR => edge.children().find_map(|c| {
            let c = super::unparen(&c);
            (crate::resolver::is_clock_edge(&c))
                .then(|| edge_signal(&c))
                .flatten()
        }),
        _ => None,
    }
}

/// The operator of a unary or binary expression.
fn op(expr: &SyntaxNode) -> Option<SyntaxKind> {
    expr.child_token(|k| !k.is_trivia()).map(|t| t.kind())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_by_family_and_part() {
        let name = |s: &str| Device::named(s).map(|d| d.name);
        assert_eq!(name("xc7a35tcpg236-1"), Some("AMD 7 series"));
        assert_eq!(name("Artix-7"), Some("AMD 7 series"));
        assert_eq!(name("xczu9eg-ffvb1156-2-e"), Some("AMD UltraScale"));
        assert_eq!(name("Cyclone V"), Some("Intel Cyclone V/Arria V"));
        assert_eq!(name("5CSEMA5F31C6"), Some("Intel Cyclone V/Arria V"));
        assert_eq!(name("10M50DAF484C7G"), Some("Intel Cyclone IV/MAX 10"));
        assert_eq!(name("LFE5U-85F"), Some("Lattice ECP5/Nexus"));
        assert_eq!(name("GW1NR-9"), Some("Gowin"));
        assert_eq!(name("MPF300T"), Some("Microchip PolarFire/SmartFusion2"));
        assert_eq!(name("nothing"), None);
    }

    #[test]
    fn globs() {
        assert!(glob_matches("cnt*", "cnt_q"));
        assert!(glob_matches("u0/cnt_reg*", "cnt"));
        assert!(glob_matches("{slow_*}", "SLOW_SUM"));
        assert!(!glob_matches("cnt", "count"));
    }
}
