//! Fixture values: an initial value for the selected constant, signal and
//! variable declarations that fits their type and size, for tests.
//!
//! Vector widths, integer ranges and array bounds come from the static
//! values the code checks use, with generics at their default values.
//! Values are pseudo-random but seeded by the object's name, so asking
//! again gives the same value.

use vfl_syntax::{SyntaxKind, SyntaxNode, TextRange, TextSize};

use super::{Bounds, MAX_DEPTH, Values, initial_expression, single_association};
use crate::analysis::Analysis;
use crate::decl::{DeclId, FileId, TypeDef, UnitId};
use crate::diagnostic::Edit;
use crate::resolver::mark_token;

/// Arrays with more elements than this get `(others => value)`.
const MAX_ELEMENTS: i64 = 64;

fn is_object_decl(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::CONSTANT_DECL | SyntaxKind::SIGNAL_DECL | SyntaxKind::VARIABLE_DECL
    )
}

/// A xorshift generator.
struct Rng(u64);

impl Rng {
    fn seeded(name: &str) -> Rng {
        // FNV-1a: stable across runs and platforms.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in name.to_ascii_lowercase().bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        Rng(h | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
}

impl Analysis {
    /// Edits giving each constant, signal and variable declaration of
    /// `file` that `range` touches (or that holds the caret, for an empty
    /// range) a fixture value of its type: an initial value replaced, or
    /// one added. Declarations whose type or size is not known statically
    /// are left out.
    pub fn fixture_edits(&self, file: FileId, range: TextRange) -> Vec<Edit> {
        let Some(analyzed) = self.files().get(file as usize) else {
            return Vec::new();
        };
        let ranges = decl_ranges(&analyzed.green, range);
        if ranges.is_empty() {
            return Vec::new();
        }
        let root = analyzed.syntax();
        let values = Values::new(self, true);
        ranges
            .into_iter()
            .filter_map(|r| {
                let decl = match root.covering_element(r) {
                    rowan::NodeOrToken::Node(n) => n,
                    rowan::NodeOrToken::Token(t) => t.parent()?,
                };
                let decl = decl.ancestors().find(|n| is_object_decl(n.kind()))?;
                values.fixture_edit(self, file, &decl)
            })
            .collect()
    }

    /// How many constant, signal and variable declarations of `file` a
    /// selection touches, for [`Analysis::fixture_edits`]. Code actions are
    /// asked for at every caret move: a caret alone (an empty `range`) is
    /// answered with 0 at once.
    pub fn fixture_targets(&self, file: FileId, range: TextRange) -> usize {
        match self.files().get(file as usize) {
            Some(f) if !range.is_empty() => decl_ranges(&f.green, range).len(),
            _ => 0,
        }
    }
}

/// The ranges of the object declarations `range` overlaps, or of the one
/// holding an empty `range`. Found on the green tree, going down only into
/// the nodes that overlap `range`, so that no syntax node is built: code
/// actions are asked for at every selection change.
fn decl_ranges(root: &rowan::GreenNodeData, range: TextRange) -> Vec<TextRange> {
    fn walk(node: &rowan::GreenNodeData, at: TextSize, range: TextRange, out: &mut Vec<TextRange>) {
        let mut at = at;
        for child in node.children() {
            let r = TextRange::at(at, child.text_len());
            at = r.end();
            // A caret is in the child that holds it; a selection overlaps.
            let (before, after) = if range.is_empty() {
                (r.end() <= range.start(), r.start() > range.start())
            } else {
                (r.end() <= range.start(), r.start() >= range.end())
            };
            if before {
                continue;
            }
            if after {
                break;
            }
            if let rowan::NodeOrToken::Node(n) = child {
                if is_object_decl(SyntaxKind::from_raw(n.kind().0)) {
                    out.push(r);
                } else {
                    walk(n, r.start(), range, out);
                }
            }
            if range.is_empty() {
                break;
            }
        }
    }
    let mut out = Vec::new();
    walk(root, TextSize::from(0), range, &mut out);
    out
}

impl Values<'_> {
    fn fixture_edit(&self, analysis: &Analysis, file: FileId, decl: &SyntaxNode) -> Option<Edit> {
        let name = decl
            .children()
            .find(|n| n.kind() == SyntaxKind::IDENT_LIST)?
            .first_token()?;
        let at = name.text_range().start();
        let unit = analysis.file_units(file).find(|&u| {
            analysis
                .unit_decls(u)
                .first()
                .is_some_and(|d| d.full_range.contains(at))
        })?;
        let indication = decl
            .children()
            .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)?;
        let mut rng = Rng::seeded(name.text());
        let value = self.indication_value(&indication, unit, None, None, &mut rng, 0)?;
        Some(match initial_expression(decl) {
            Some(old) => Edit {
                range: old.text_range(),
                text: value,
            },
            None => {
                let semi = decl.last_token().filter(|t| t.kind() == SyntaxKind::SEMI)?;
                Edit {
                    range: TextRange::empty(semi.text_range().start()),
                    text: format!(" := {value}"),
                }
            }
        })
    }

    /// A value for a subtype indication of `unit`; `index` and `element`
    /// are constraints given from outside, which win over its own.
    fn indication_value(
        &self,
        indication: &SyntaxNode,
        unit: UnitId,
        index: Option<Bounds>,
        element: Option<Bounds>,
        rng: &mut Rng,
        depth: u32,
    ) -> Option<String> {
        use SyntaxKind::*;
        if depth > MAX_DEPTH {
            return None;
        }
        // A resolution function comes before the type mark.
        let mut mark = indication
            .children()
            .filter(|n| matches!(n.kind(), CALL_EXPR | NAME | SELECTED_NAME))
            .last()?;
        // `t(0 to 3)(7 downto 0)`: the index constraint, then the element's.
        let mut constraints = Vec::new();
        while mark.kind() == CALL_EXPR {
            let list = mark.children().find(|n| n.kind() == ASSOCIATION_LIST)?;
            constraints.push(single_association(&list)?);
            mark = mark.children().next()?;
        }
        constraints.reverse();
        if let Some(range) = indication
            .children()
            .find(|n| n.kind() == RANGE_CONSTRAINT)
            .and_then(|c| c.children().find(|n| n.kind() == RANGE_EXPR))
        {
            constraints.push(range);
        }
        let mut own = constraints.iter().map(|c| self.range(c, unit, depth + 1));
        let own_index = own.next().flatten();
        let own_element = own.next().flatten();
        let mark = self.target(unit, mark_token(&mark)?.text_range())?;
        self.mark_value(
            mark,
            index.or(own_index),
            element.or(own_element),
            rng,
            depth + 1,
        )
    }

    /// A value of type or subtype `mark`, with these constraints if given.
    fn mark_value(
        &self,
        mark: DeclId,
        index: Option<Bounds>,
        element: Option<Bounds>,
        rng: &mut Rng,
        depth: u32,
    ) -> Option<String> {
        let base = self.analysis.base_type(mark)?;
        let base_decl = self.analysis.decl(base)?;
        match base_decl.type_def.as_deref()? {
            TypeDef::Enum(literals) => {
                let node = self.declaration(base_decl)?;
                enum_value(&node, literals, rng)
            }
            TypeDef::Integer => {
                let (lo, hi) = match index {
                    Some(b) => (Some(b.lo()), Some(b.hi())),
                    None => self.integer_bounds(mark).unwrap_or((None, None)),
                };
                Some(integer_value(lo, hi, rng).to_string())
            }
            TypeDef::Real => Some(format!("{}.{}", rng.below(100), rng.below(10))),
            TypeDef::Physical if base_decl.name == "time" => {
                Some(format!("{} ns", 1 + rng.below(1000)))
            }
            TypeDef::Array {
                indexes,
                element: e,
            } if indexes.len() == 1 => {
                let bounds = index.or_else(|| self.mark_bounds(mark, depth))?;
                let width = (bounds.hi() - bounds.lo() + 1).max(0);
                let scalar = e
                    .and_then(|e| self.analysis.base_type(e))
                    .and_then(|e| self.analysis.decl(e));
                if let Some(TypeDef::Enum(literals)) = scalar.and_then(|d| d.type_def.as_deref()) {
                    if literals.iter().any(|l| l == "'a'") {
                        let text: String = (0..width)
                            .map(|_| char::from(b'a' + rng.below(26) as u8))
                            .collect();
                        return Some(format!("\"{text}\""));
                    }
                    if literals.iter().any(|l| l == "'0'") && literals.iter().any(|l| l == "'1'") {
                        return Some(bit_string(width, rng));
                    }
                }
                let element_indication = self
                    .declaration(base_decl)?
                    .children()
                    .find(|n| n.kind() == SyntaxKind::ARRAY_TYPE_DEF)?
                    .children()
                    .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)?;
                let mut value = || {
                    self.indication_value(&element_indication, base.unit, element, None, rng, depth)
                };
                if width == 0 || width > MAX_ELEMENTS {
                    return Some(format!("(others => {})", value()?));
                }
                let values = (0..width).map(|_| value()).collect::<Option<Vec<_>>>()?;
                // A positional aggregate needs two elements at least.
                Some(if width == 1 {
                    format!("({} => {})", bounds.left, values[0])
                } else {
                    format!("({})", values.join(", "))
                })
            }
            TypeDef::Record(_) => {
                let node = self.declaration(base_decl)?;
                let def = node
                    .children()
                    .find(|n| n.kind() == SyntaxKind::RECORD_TYPE_DEF)?;
                let mut parts = Vec::new();
                for element in def
                    .children()
                    .filter(|n| n.kind() == SyntaxKind::ELEMENT_DECL)
                {
                    let indication = element
                        .children()
                        .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)?;
                    let names = element
                        .children()
                        .find(|n| n.kind() == SyntaxKind::IDENT_LIST)?;
                    for name in names
                        .children_with_tokens()
                        .filter_map(|t| t.into_token())
                        .filter(|t| t.kind() == SyntaxKind::IDENT)
                    {
                        let value =
                            self.indication_value(&indication, base.unit, None, None, rng, depth)?;
                        parts.push(format!("{} => {value}", name.text()));
                    }
                }
                Some(format!("({})", parts.join(", ")))
            }
            _ => None,
        }
    }
}

/// A literal of an enumeration type, as declared in `node`.
fn enum_value(node: &SyntaxNode, literals: &[String], rng: &mut Rng) -> Option<String> {
    // Of `std_ulogic` and `character`, the values tests use.
    let usual: &[&str] = if literals.iter().any(|l| l == "'a'") {
        &["'a'", "'b'", "'c'", "'x'", "'y'", "'z'", "'0'", "'1'"]
    } else if literals.iter().any(|l| l == "'0'") && literals.iter().any(|l| l == "'1'") {
        &["'0'", "'1'"]
    } else {
        &[]
    };
    if !usual.is_empty() {
        return Some(usual[rng.below(usual.len() as u64) as usize].to_owned());
    }
    let def = node
        .children()
        .find(|n| n.kind() == SyntaxKind::ENUM_TYPE_DEF)?;
    let spelled: Vec<_> = def
        .children_with_tokens()
        .filter_map(|t| t.into_token())
        .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::CHAR_LITERAL))
        .collect();
    let i = rng.below(spelled.len() as u64) as usize;
    spelled.get(i).map(|t| t.text().to_owned())
}

/// An integer in `lo..=hi`, either bound possibly unknown.
fn integer_value(lo: Option<i64>, hi: Option<i64>, rng: &mut Rng) -> i64 {
    match (lo, hi) {
        (Some(lo), Some(hi)) if lo <= hi => {
            let span = (i128::from(hi) - i128::from(lo) + 1).min(i128::from(u32::MAX)) as u64;
            lo.saturating_add(rng.below(span) as i64)
        }
        (Some(lo), Some(_)) => lo,
        (Some(lo), None) => lo.saturating_add(rng.below(256) as i64),
        (None, Some(hi)) => hi.saturating_sub(rng.below(256) as i64),
        (None, None) => rng.below(256) as i64,
    }
}

/// A bit string literal of `width` bits: hexadecimal when it divides by 4.
fn bit_string(width: i64, rng: &mut Rng) -> String {
    if width > 0 && width % 4 == 0 {
        let digits: String = (0..width / 4)
            .map(|_| char::from(b"0123456789ABCDEF"[rng.below(16) as usize]))
            .collect();
        format!("x\"{digits}\"")
    } else {
        let bits: String = (0..width)
            .map(|_| if rng.below(2) == 0 { '0' } else { '1' })
            .collect();
        format!("\"{bits}\"")
    }
}
