//! `case` statements and selected assignments whose choices leave out
//! values of the selector's subtype (`sema/incomplete-case`), or name a
//! value twice (`sema/duplicate-choice`).
//!
//! Selectors are objects (or slices and elements of them) of an
//! enumeration type, an integer subtype with static bounds, or a
//! one-dimensional array of character literals (`std_logic_vector`,
//! `bit_vector`, `unsigned`) of static width. A `case` with a choice that
//! is not a literal or a static value (a constant of an unknown value, a
//! subtype name, an attribute) is left alone, as is a matching `case?`.

use rustc_hash::FxHashMap;
use vfl_syntax::green_walk::{ChildTokens, descendants_where};
use vfl_syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange, TextSize};

use super::{Checker, static_int};
use crate::decl::{Decl, DeclId, DeclKind, Target, TypeDef};
use crate::diagnostic::{Diagnostic, Edit, Fix, codes};
use crate::resolver::mark_token;

/// The most missing values a fix spells out, one alternative each; more,
/// and it adds `when others` instead.
const MAX_LISTED: usize = 8;

/// The values a selector can take.
enum Domain<'a> {
    /// The literals of an enumeration type, by position.
    Enum { ty: DeclId, literals: &'a [String] },
    /// The integers from `lo` to `hi`.
    Int { lo: i64, hi: i64 },
    /// Arrays of `width` elements of an enumeration of characters.
    Vector { width: u32, literals: &'a [String] },
}

/// A choice's values.
enum Values {
    /// Positions (or integers) from `.0` to `.1`.
    Span(i64, i64),
    /// An array value, one literal key per element.
    Word(String),
}

impl<'a> Checker<'a> {
    /// Checks the choices of a `case` statement or a selected assignment.
    pub(super) fn case_choices(&mut self, node: &SyntaxNode) {
        use SyntaxKind::*;
        // `case?` and `select?` match don't-care values.
        if node.has_child(|k| k == QUESTION) {
            return;
        }
        let (selector, keyword) = match node.kind() {
            CASE_STMT => (
                node.children().find(|n| n.kind() != LABEL),
                node.child_token(|k| k == CASE_KW),
            ),
            _ => (
                node.children().find(|n| n.kind() != LABEL),
                node.child_token(|k| k == WITH_KW),
            ),
        };
        let (Some(selector), Some(keyword)) = (selector, keyword) else {
            return;
        };
        let choices: Vec<SyntaxNode> = match node.kind() {
            CASE_STMT => node
                .children()
                .filter(|n| n.kind() == CASE_ALTERNATIVE)
                .filter_map(|a| a.children().find(|n| n.kind() == CHOICES))
                .collect(),
            _ => node
                .children()
                .filter(|n| n.kind() == SELECTED_WAVEFORMS)
                .flat_map(|w| w.children().filter(|n| n.kind() == CHOICES))
                .collect(),
        };
        if choices.is_empty() {
            return;
        }
        let Some(domain) = self.selector_domain(&selector) else {
            return;
        };
        // A syntax error may hide alternatives.
        if !descendants_where(node, |k| k == ERROR).is_empty() {
            return;
        }
        // Every choice, in source order.
        let mut values: Vec<(Values, TextRange)> = Vec::new();
        let mut others = false;
        for list in &choices {
            for element in list.children_with_tokens() {
                match element {
                    rowan::NodeOrToken::Token(t) if t.kind() == OTHERS_KW => others = true,
                    rowan::NodeOrToken::Token(_) => {}
                    rowan::NodeOrToken::Node(n) => {
                        let Some(v) = self.choice_values(&n, &domain) else {
                            return;
                        };
                        values.push((v, n.text_range()));
                    }
                }
            }
        }
        self.duplicate_choices(&values);
        if !others {
            self.incomplete_case(node, &keyword, &values, &domain);
        }
    }

    /// The values a selector can take, when known.
    fn selector_domain(&self, selector: &SyntaxNode) -> Option<Domain<'a>> {
        use SyntaxKind::*;
        match selector.kind() {
            NAME | SELECTED_NAME => {
                let Some(decl) = self.named(selector) else {
                    // An element of a record: `r.state`.
                    let mark = self.element_subtype(selector)?;
                    return self.domain(self.decl(mark)?, mark);
                };
                if !decl.kind.is_object() || decl.kind == DeclKind::LoopParameter {
                    return None;
                }
                self.domain(decl, decl.subtype?)
            }
            // An element or a slice of an array object.
            CALL_EXPR => {
                let prefix = selector.children().next()?;
                let decl = self.named(&prefix)?;
                if !decl.kind.is_object() {
                    return None;
                }
                let Some(TypeDef::Array {
                    indexes,
                    element: Some(element),
                }) = self.object_type(decl)
                else {
                    return None;
                };
                if indexes.len() != 1 {
                    return None;
                }
                let list = selector.children().find(|n| n.kind() == ASSOCIATION_LIST)?;
                let mut elements = list.children();
                let index = elements.next()?.children().next()?;
                if elements.next().is_some() {
                    return None;
                }
                let (ty, literals) = self.enum_type(*element)?;
                if index.kind() == RANGE_EXPR {
                    let width = self.width(selector)?;
                    character_enum(literals).then_some(Domain::Vector { width, literals })
                } else {
                    Some(Domain::Enum { ty, literals })
                }
            }
            _ => None,
        }
    }

    /// The subtype of the record element a selected name denotes.
    fn element_subtype(&self, name: &SyntaxNode) -> Option<DeclId> {
        if name.kind() != SyntaxKind::SELECTED_NAME {
            return None;
        }
        let range = mark_token(name)?.text_range();
        let element = self.elements.iter().find(|e| e.range == range)?;
        match self.decl(element.record)?.type_def.as_deref()? {
            TypeDef::Record(elements) => elements.iter().find(|e| e.name == element.name)?.subtype,
            _ => None,
        }
    }

    /// The values an object, or a subtype, can take: those of `mark`, its
    /// subtype, as `decl` constrains it.
    fn domain(&self, decl: &Decl, mark: DeclId) -> Option<Domain<'a>> {
        match self.type_def(mark)? {
            TypeDef::Enum(_) => {
                // Enumeration subtypes with a range are not followed.
                if self.indication_range(decl).is_some() {
                    return None;
                }
                let (ty, literals) = self.enum_type(mark)?;
                Some(Domain::Enum { ty, literals })
            }
            TypeDef::Integer => {
                let (lo, hi) = self.integer_bounds(decl)?;
                Some(Domain::Int { lo, hi })
            }
            TypeDef::Array {
                indexes,
                element: Some(element),
            } if indexes.len() == 1 => {
                let (_, literals) = self.enum_type(*element)?;
                let width = decl.width?;
                character_enum(literals).then_some(Domain::Vector { width, literals })
            }
            _ => None,
        }
    }

    /// The enumeration type a type or subtype mark denotes, when no subtype
    /// on the way restricts it with a range.
    fn enum_type(&self, mut mark: DeclId) -> Option<(DeclId, &'a [String])> {
        for _ in 0..32 {
            let decl = self.decl(mark)?;
            match decl.kind {
                DeclKind::Type => match decl.type_def.as_deref()? {
                    TypeDef::Enum(literals) => return Some((mark, literals)),
                    _ => return None,
                },
                DeclKind::Subtype => {
                    if self.indication_range(decl).is_some() {
                        return None;
                    }
                    mark = decl.subtype?;
                }
                _ => return None,
            }
        }
        None
    }

    /// The bounds of an integer object's subtype, or of a subtype: its own
    /// range constraint, or that of the subtypes and the type it names.
    fn integer_bounds(&self, decl: &Decl) -> Option<(i64, i64)> {
        let mut decl = decl;
        for _ in 0..32 {
            if decl.kind == DeclKind::Type {
                // `type t is range 0 to 15`.
                let root = self.ctx.files.get(decl.file as usize)?.syntax();
                let node = declaration_node(&root, decl.range)?;
                let range = node
                    .descendants()
                    .find(|n| n.kind() == SyntaxKind::RANGE_EXPR)?;
                return self.static_bounds(decl, &range);
            }
            if let Some(range) = self.indication_range(decl) {
                return self.static_bounds(decl, &range);
            }
            decl = self.decl(decl.subtype?)?;
        }
        None
    }

    /// The range of the range constraint of a declaration's subtype
    /// indication: `integer range 0 to 3`.
    fn indication_range(&self, decl: &Decl) -> Option<SyntaxNode> {
        let root = self.ctx.files.get(decl.file as usize)?.syntax();
        let node = declaration_node(&root, decl.range)?;
        node.children()
            .find(|n| n.kind() == SyntaxKind::SUBTYPE_INDICATION)?
            .children()
            .find(|n| n.kind() == SyntaxKind::RANGE_CONSTRAINT)?
            .children()
            .find(|n| n.kind() == SyntaxKind::RANGE_EXPR)
    }

    /// The low and high bound of a static range of a declaration. Names are
    /// followed only in this unit's file, whose references are known.
    fn static_bounds(&self, decl: &Decl, range: &SyntaxNode) -> Option<(i64, i64)> {
        let local = decl.file == self.file;
        let value = |n: &SyntaxNode| if local { self.named_value(n) } else { None };
        let mut bounds = range.children();
        let left = static_int(&bounds.next()?, &value)?;
        let right = static_int(&bounds.next()?, &value)?;
        let (lo, hi) = if range.has_child(|k| k == SyntaxKind::TO_KW) {
            (left, right)
        } else {
            (right, left)
        };
        (lo <= hi).then_some((lo, hi))
    }

    /// The values of one choice.
    fn choice_values(&self, choice: &SyntaxNode, domain: &Domain) -> Option<Values> {
        use SyntaxKind::*;
        match domain {
            Domain::Enum { ty, literals } => {
                let position = |n: &SyntaxNode| self.enum_position(n, *ty, literals);
                if choice.kind() == RANGE_EXPR {
                    let mut bounds = choice.children();
                    let left = position(&bounds.next()?)?;
                    let right = position(&bounds.next()?)?;
                    let (lo, hi) = if choice.has_child(|k| k == TO_KW) {
                        (left, right)
                    } else {
                        (right, left)
                    };
                    return (lo <= hi).then_some(Values::Span(lo, hi));
                }
                let p = position(choice)?;
                Some(Values::Span(p, p))
            }
            Domain::Int { .. } => {
                let value = |n: &SyntaxNode| self.named_value(n);
                if choice.kind() == RANGE_EXPR {
                    let mut bounds = choice.children();
                    let left = static_int(&bounds.next()?, &value)?;
                    let right = static_int(&bounds.next()?, &value)?;
                    let (lo, hi) = if choice.has_child(|k| k == TO_KW) {
                        (left, right)
                    } else {
                        (right, left)
                    };
                    return (lo <= hi).then_some(Values::Span(lo, hi));
                }
                let v = static_int(choice, &value)?;
                Some(Values::Span(v, v))
            }
            Domain::Vector { width, literals } => {
                if choice.kind() != LITERAL {
                    return None;
                }
                let word = word(&choice.first_token()?, literals)?;
                (word.chars().count() == *width as usize).then_some(Values::Word(word))
            }
        }
    }

    /// The position of the literal of enumeration `ty` a choice names.
    fn enum_position(&self, choice: &SyntaxNode, ty: DeclId, literals: &[String]) -> Option<i64> {
        let key = match choice.kind() {
            SyntaxKind::LITERAL => {
                let token = choice.first_token()?;
                if token.kind() != SyntaxKind::CHAR_LITERAL {
                    return None;
                }
                token.text().to_owned()
            }
            SyntaxKind::NAME | SyntaxKind::SELECTED_NAME => {
                let token = mark_token(choice)?;
                let r = &self.result.refs[*self.refs_at.get(&token.text_range())?];
                r.targets.iter().find_map(|t| match t {
                    Target::Decl(id) => self
                        .decl(*id)
                        .filter(|d| d.kind == DeclKind::EnumLiteral && d.subtype == Some(ty))
                        .map(|d| d.name.clone()),
                    Target::Library(_) => None,
                })?
            }
            _ => return None,
        };
        literals.iter().position(|l| *l == key).map(|p| p as i64)
    }

    /// The literal of `ty` at `position`, as declared.
    fn literal_spelling(&self, ty: DeclId, literals: &[String], position: usize) -> String {
        self.decl(ty)
            .and_then(|d| d.members.as_ref())
            .and_then(|m| m.get(&literals[position]).first().copied())
            .and_then(|id| self.decl(id))
            .map_or_else(|| literals[position].clone(), |d| d.spelling.clone())
    }

    fn duplicate_choices(&mut self, values: &[(Values, TextRange)]) {
        let mut spans: Vec<(i64, i64, TextRange)> = Vec::new();
        let mut words: FxHashMap<&str, TextRange> = FxHashMap::default();
        let mut diags = Vec::new();
        for (v, range) in values {
            let earlier = match v {
                Values::Span(lo, hi) => {
                    let earlier = spans
                        .iter()
                        .find(|(l, h, _)| l <= hi && lo <= h)
                        .map(|(.., r)| *r);
                    spans.push((*lo, *hi, *range));
                    earlier
                }
                Values::Word(w) => words.get(w.as_str()).copied().or_else(|| {
                    words.insert(w, *range);
                    None
                }),
            };
            let Some(earlier) = earlier else {
                continue;
            };
            let text = &self.ctx.files[self.file as usize].text;
            let choice = text[std::ops::Range::<usize>::from(*range)].trim();
            diags.push(Diagnostic {
                code: codes::DUPLICATE_CHOICE,
                file: self.file,
                range: *range,
                message: format!(
                    "`{choice}` is already covered by the choice at line {}",
                    self.ctx.line_of(self.file, earlier.start())
                ),
                fix: None,
            });
        }
        self.diags.extend(diags);
    }

    fn incomplete_case(
        &mut self,
        node: &SyntaxNode,
        keyword: &SyntaxToken,
        values: &[(Values, TextRange)],
        domain: &Domain,
    ) {
        // The values left out, as spans, or their number for arrays.
        let (missing, names): (Option<Vec<(i64, i64)>>, Vec<String>) = match domain {
            Domain::Enum { ty, literals } => {
                let spans = uncovered(values, 0, literals.len() as i64 - 1);
                let names = spans
                    .iter()
                    .flat_map(|&(lo, hi)| lo..=hi)
                    .map(|p| self.literal_spelling(*ty, literals, p as usize))
                    .collect();
                (Some(spans), names)
            }
            Domain::Int { lo, hi } => {
                let spans = uncovered(values, *lo, *hi);
                let names = spans
                    .iter()
                    .map(|&(lo, hi)| {
                        if lo == hi {
                            lo.to_string()
                        } else {
                            format!("{lo} to {hi}")
                        }
                    })
                    .collect();
                (Some(spans), names)
            }
            Domain::Vector { width, literals } => {
                let distinct = values
                    .iter()
                    .filter_map(|(v, _)| match v {
                        Values::Word(w) => Some(w.as_str()),
                        Values::Span(..) => None,
                    })
                    .collect::<rustc_hash::FxHashSet<_>>()
                    .len() as u128;
                let total = (literals.len() as u128).checked_pow(*width);
                if total.is_some_and(|t| distinct >= t) {
                    return;
                }
                (None, Vec::new())
            }
        };
        if missing.as_ref().is_some_and(|m| m.is_empty()) {
            return;
        }
        let message = match domain {
            Domain::Vector { width, literals } => format!(
                "the choices do not cover every value of this {width}-element selector, whose elements have {} values; add `when others`",
                literals.len()
            ),
            _ => {
                let shown: Vec<String> = names.iter().take(5).map(|n| format!("`{n}`")).collect();
                let more = names.len().saturating_sub(5);
                let list = if more > 0 {
                    format!("{} and {more} more", shown.join(", "))
                } else {
                    shown.join(", ")
                };
                format!("the choices do not cover {list}")
            }
        };
        let fix = if node.kind() == SyntaxKind::CASE_STMT {
            let listed = match domain {
                Domain::Enum { literals, .. } if !character_enum(literals) => {
                    (names.len() <= MAX_LISTED).then_some(names.clone())
                }
                Domain::Int { .. } => (names.len() <= MAX_LISTED).then_some(names.clone()),
                _ => None,
            };
            self.missing_alternatives(node, listed)
        } else {
            None
        };
        self.diags.push(Diagnostic {
            code: codes::INCOMPLETE_CASE,
            file: self.file,
            range: keyword.text_range(),
            message,
            fix,
        });
    }

    /// Alternatives for `choices` (or `when others`) before `end case`,
    /// indented as the others, with a `null` body.
    fn missing_alternatives(&self, node: &SyntaxNode, choices: Option<Vec<String>>) -> Option<Fix> {
        use SyntaxKind::*;
        let text = &self.ctx.files[self.file as usize].text;
        let line_start = |offset: usize| text[..offset].rfind('\n').map_or(0, |i| i + 1);
        let end = usize::from(node.child_token(|k| k == END_KW)?.text_range().start());
        let start = line_start(end);
        let when = usize::from(
            node.children()
                .find(|n| n.kind() == CASE_ALTERNATIVE)?
                .text_range()
                .start(),
        );
        let when_start = line_start(when);
        if !text[start..end].trim().is_empty() || !text[when_start..when].trim().is_empty() {
            return None;
        }
        let newline = if text[..start].ends_with("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let indent = &text[when_start..when];
        let (title, choices) = match choices {
            Some(choices) => {
                let title = if choices.len() == 1 {
                    format!("Add a `when {}` alternative", choices[0])
                } else {
                    format!(
                        "Add `when` alternatives for the {} missing choices",
                        choices.len()
                    )
                };
                (title, choices)
            }
            None => (
                "Add `when others => null;`".to_owned(),
                vec!["others".to_owned()],
            ),
        };
        let inserted: String = choices
            .iter()
            .map(|c| format!("{indent}when {c} => null;{newline}"))
            .collect();
        Some(Fix {
            title,
            edits: vec![Edit {
                range: TextRange::empty(TextSize::from(start as u32)),
                text: inserted,
            }],
            // The new alternatives do nothing, which may not be what the
            // design needs.
            safe: false,
        })
    }
}

/// The declaration statement (`signal`, `subtype`, ...) that declares the
/// name at `range`.
pub(crate) fn declaration_node(root: &SyntaxNode, range: TextRange) -> Option<SyntaxNode> {
    use SyntaxKind::*;
    // A Verilog declaration has no VHDL syntax: its file's tree is empty.
    if !root.text_range().contains_range(range) {
        return None;
    }
    let element = root.covering_element(range);
    let node = match element {
        rowan::NodeOrToken::Node(n) => n,
        rowan::NodeOrToken::Token(t) => t.parent()?,
    };
    node.ancestors().find(|n| {
        matches!(
            n.kind(),
            SIGNAL_DECL
                | VARIABLE_DECL
                | CONSTANT_DECL
                | INTERFACE_OBJECT_DECL
                | SUBTYPE_DECL
                | TYPE_DECL
                | ALIAS_DECL
        )
    })
}

/// Whether every literal of an enumeration is a character: `std_ulogic`,
/// `bit`.
fn character_enum(literals: &[String]) -> bool {
    !literals.is_empty() && literals.iter().all(|l| l.starts_with('\''))
}

/// The value of a string or bit string literal, as the key of each
/// element's literal joined: `"01"` for `"01"`, `b"01"` and `x"1"` (with
/// 4 elements). `None` for elements that are not literals of the type.
fn word(token: &SyntaxToken, literals: &[String]) -> Option<String> {
    let text = token.text();
    let digits: String = match token.kind() {
        SyntaxKind::STRING_LITERAL => text.get(1..text.len().checked_sub(1)?)?.to_owned(),
        SyntaxKind::BIT_STRING_LITERAL => {
            let quote = text.find('"')?;
            let (prefix, rest) = text.split_at(quote);
            let content = rest.trim_matches('"').replace('_', "");
            let bits = match prefix.to_ascii_lowercase().as_str() {
                "b" => 1,
                "o" => 3,
                "x" => 4,
                // With a length, signed, unsigned or decimal: not followed.
                _ => return None,
            };
            let mut out = String::new();
            for c in content.chars() {
                let v = c.to_digit(1 << bits)?;
                for i in (0..bits).rev() {
                    out.push(if v >> i & 1 == 1 { '1' } else { '0' });
                }
            }
            out
        }
        _ => return None,
    };
    let mut key = String::with_capacity(digits.len() * 3);
    let mut buf = [0u8; 4];
    for c in digits.chars() {
        let literal = format!("'{}'", c.encode_utf8(&mut buf));
        if !literals.contains(&literal) {
            return None;
        }
        key.push(c);
    }
    Some(key)
}

/// The spans of `lo..=hi` no choice covers.
fn uncovered(values: &[(Values, TextRange)], lo: i64, hi: i64) -> Vec<(i64, i64)> {
    let mut spans: Vec<(i64, i64)> = values
        .iter()
        .filter_map(|(v, _)| match v {
            Values::Span(l, h) => Some((*l, *h)),
            Values::Word(_) => None,
        })
        .collect();
    spans.sort_unstable();
    let mut missing = Vec::new();
    let mut next = lo;
    for (l, h) in spans {
        if next > hi {
            break;
        }
        if l > next {
            missing.push((next, (l - 1).min(hi)));
        }
        next = next.max(h.saturating_add(1));
    }
    if next <= hi {
        missing.push((next, hi));
    }
    missing
}
