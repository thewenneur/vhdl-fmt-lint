//! The quick fix of `sema/unassociated-port`: adding the ports or generics
//! an instantiation leaves out to its map, laid out like the map around
//! them. Only worked out when the fix is offered (which ports are missing)
//! and when it is chosen (its edits), never while linting.

use vfl_config::{Case, Config, Separator};
use vfl_sema::{DeclId, DeclKind, FileId, Mode, Target};
use vfl_syntax::{SyntaxKind, SyntaxNode, TextRange, TextSize};

use crate::actions::Edits;
use crate::world::World;

/// What one map of an instantiation leaves out.
pub(crate) struct Missing {
    stmt: SyntaxNode,
    /// The association list of the map, when there is one.
    list: Option<SyntaxNode>,
    ports: bool,
    /// Ports of mode `in` and generics without a default value: the ones
    /// the problem is about.
    pub required: Vec<DeclId>,
    /// The others, which may be left out: outputs, and inputs and generics
    /// with a default value.
    pub optional: Vec<DeclId>,
}

/// The ports (or generics) the instantiation whose problem starts at `at`
/// leaves out of its map. `ports` tells a port map problem from a generic
/// map one, which can start at the same label.
pub(crate) fn missing(world: &World, file: FileId, at: TextSize, ports: bool) -> Option<Missing> {
    let analysis = &world.analysis;
    let root = analysis.files()[file as usize].syntax();
    // Down to the statement: each step a binary search among the children,
    // where `token_at_offset` would build every child it passes over.
    let mut stmt = root;
    while stmt.kind() != SyntaxKind::COMPONENT_INSTANTIATION_STMT {
        stmt = stmt
            .child_or_token_at_range(TextRange::at(at, 1.into()))?
            .into_node()?;
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
    let name = unit
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
        .last()?;
    let reference = crate::types::reference_at(analysis, file, name.text_range())?;
    let members = reference.targets.iter().find_map(|t| match t {
        Target::Decl(id) => analysis.decl(*id)?.members.clone(),
        Target::Library(_) => None,
    })?;
    if members.opaque {
        return None;
    }
    // In order, as positional associations count them.
    let formals: Vec<DeclId> = members
        .iter()
        .filter(|&id| {
            analysis.decl(id).is_some_and(|d| {
                if ports {
                    matches!(d.kind, DeclKind::Port { .. })
                } else {
                    matches!(
                        d.kind,
                        DeclKind::Generic { .. }
                            | DeclKind::GenericType
                            | DeclKind::GenericSubprogram
                            | DeclKind::GenericPackage
                    )
                }
            })
        })
        .collect();
    let aspect = if ports {
        SyntaxKind::PORT_MAP_ASPECT
    } else {
        SyntaxKind::GENERIC_MAP_ASPECT
    };
    let list = stmt.children().find(|n| n.kind() == aspect).and_then(|a| {
        a.children()
            .find(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)
    });
    let mut associated = vec![false; formals.len()];
    let mut position = 0;
    for element in list
        .iter()
        .flat_map(|l| l.children())
        .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
    {
        if !has_arrow(&element) {
            if let Some(a) = associated.get_mut(position) {
                *a = true;
            }
            position += 1;
            continue;
        }
        // The formal designator resolves to its port: `p`, `p(3)`,
        // `to_integer(p)`.
        let Some(formal) = element.first_child() else {
            continue;
        };
        for token in formal
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
        {
            let Some(r) = crate::types::reference_at(analysis, file, token.text_range()) else {
                continue;
            };
            for target in &r.targets {
                if let Target::Decl(id) = target
                    && let Some(i) = formals.iter().position(|f| f == id)
                {
                    associated[i] = true;
                }
            }
        }
    }
    let mut required = Vec::new();
    let mut optional = Vec::new();
    for (&id, _) in formals.iter().zip(&associated).filter(|(_, a)| !**a) {
        match analysis.decl(id).map(|d| d.kind) {
            Some(
                DeclKind::Port {
                    mode: Mode::In,
                    has_default: false,
                }
                | DeclKind::Generic { has_default: false },
            ) => required.push(id),
            // A mode view or a generic type has no `open`.
            Some(DeclKind::Port { mode, .. }) if mode != Mode::View => optional.push(id),
            Some(DeclKind::Generic { has_default: true }) => optional.push(id),
            _ => {}
        }
    }
    Some(Missing {
        stmt,
        list,
        ports,
        required,
        optional,
    })
}

fn has_arrow(element: &SyntaxNode) -> bool {
    element
        .children_with_tokens()
        .any(|e| e.kind() == SyntaxKind::ARROW)
}

/// The quick fix's title: what it adds.
pub(crate) fn title(world: &World, missing: &Missing, all: bool) -> String {
    let kind = if missing.ports { "port" } else { "generic" };
    if all {
        return format!("Associate every {kind}, the optional ones `open`");
    }
    let names: Vec<String> = missing
        .required
        .iter()
        .filter_map(|&id| world.analysis.decl(id))
        .map(|d| format!("`{}`", d.spelling))
        .collect();
    let s = if names.len() == 1 { "" } else { "s" };
    format!("Associate {kind}{s} {}", names.join(", "))
}

/// The edits adding the missing associations (all of them with `all`, else
/// the required ones) to the file. A required port gets the object of its
/// own name, which is often the signal it is meant for and otherwise shows
/// as undeclared, to be filled in; the others get `open`. When the
/// instantiation was laid out as the formatter would, the result is too.
pub(crate) fn edits(
    world: &mut World,
    file: FileId,
    at: TextSize,
    ports: bool,
    all: bool,
) -> Option<Edits> {
    let missing = missing(world, file, at, ports)?;
    let analysis = &world.analysis;
    let mut new: Vec<(String, String)> = missing
        .required
        .iter()
        .filter_map(|&id| analysis.decl(id))
        .map(|d| (d.spelling.clone(), d.spelling.clone()))
        .collect();
    if all {
        // In declaration order, as written in the entity.
        let mut ids: Vec<DeclId> = missing
            .required
            .iter()
            .chain(&missing.optional)
            .copied()
            .collect();
        ids.sort();
        new = ids
            .into_iter()
            .filter_map(|id| analysis.decl(id).map(|d| (id, d)))
            .map(|(id, d)| {
                let actual = if missing.required.contains(&id) {
                    d.spelling.clone()
                } else {
                    "open".to_owned()
                };
                (d.spelling.clone(), actual)
            })
            .collect();
    }
    if new.is_empty() {
        return None;
    }
    let path = analysis.files()[file as usize].path.clone();
    let text: String = world.doc(file).text.to_owned();
    let config = world.config(&path).clone();
    let raw = layout(&text, &missing, &new, &config);
    let stmt = missing.stmt.text_range();
    Some(
        formatted(&path, &text, stmt, &raw, &config)
            .unwrap_or(raw)
            .into_iter()
            .map(|(range, text)| (file, range, text))
            .collect(),
    )
}

/// The edits of [`edits`], laid out after the map that is there.
fn layout(
    text: &str,
    missing: &Missing,
    new: &[(String, String)],
    config: &Config,
) -> Vec<(TextRange, String)> {
    let elements: Vec<SyntaxNode> = missing
        .list
        .iter()
        .flat_map(|l| l.children())
        .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_ELEMENT)
        .collect();
    let Some(last) = elements.last() else {
        return vec![new_map(text, missing, new, config)];
    };
    let last_start = usize::from(last.text_range().start());
    let at = last.text_range().end();
    let line_start = line_start(text, last_start);
    let prefix = &text[line_start..last_start];
    let first_start = usize::from(elements[0].text_range().start());
    let open = missing
        .list
        .as_ref()
        .and_then(|l| l.parent())
        .and_then(|a| {
            a.descendants_with_tokens()
                .find(|e| e.kind() == SyntaxKind::LPAREN)
        })
        .map_or(first_start, |p| usize::from(p.text_range().end()));
    let one_line = !text[open.min(first_start)..last_start].contains('\n');
    // One element per line, the comma before or after each: as in the map
    // when it has more than one, else as configured.
    let leading = config.format.association_separator == Separator::Leading;
    let (separator, width) = match prefix.trim() {
        _ if one_line => (", ".to_owned(), 0),
        "" if leading && elements.len() == 1 && prefix.len() >= 2 => (
            format!("\n{}, ", &prefix[..prefix.len() - 2]),
            arrow_column(text, &elements),
        ),
        "" => (format!(",\n{prefix}"), arrow_column(text, &elements)),
        "," => (format!("\n{prefix}"), arrow_column(text, &elements)),
        _ => (", ".to_owned(), 0),
    };
    let widest = new
        .iter()
        .map(|(f, _)| f.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = Vec::new();
    // Aligned arrows stay aligned, the new formals included.
    let column = if width > 0 { width.max(widest + 1) } else { 0 };
    if column > width && width > 0 {
        for element in &elements {
            if let Some(arrow) = element
                .children_with_tokens()
                .find(|e| e.kind() == SyntaxKind::ARROW)
            {
                out.push((
                    TextRange::empty(arrow.text_range().start()),
                    " ".repeat(column - width),
                ));
            }
        }
    }
    let mut insert = String::new();
    for (formal, actual) in new {
        insert.push_str(&separator);
        insert.push_str(&association(formal, actual, column));
    }
    out.push((TextRange::empty(at), insert));
    out
}

/// `formal => actual`, the arrow at `column` after the formal's start
/// when it is not 0.
fn association(formal: &str, actual: &str, column: usize) -> String {
    let pad = column.saturating_sub(formal.chars().count()).max(1);
    format!("{formal}{}=> {actual}", " ".repeat(pad))
}

/// Where the arrows of `elements` that start their line are, from the
/// element's start, when they are all at the same place and there are at
/// least two; else 0.
fn arrow_column(text: &str, elements: &[SyntaxNode]) -> usize {
    let mut columns = elements.iter().filter_map(|element| {
        let start = usize::from(element.text_range().start());
        let arrow = element
            .children_with_tokens()
            .find(|e| e.kind() == SyntaxKind::ARROW)?;
        let arrow = usize::from(arrow.text_range().start());
        let before = text[line_start(text, start)..start].trim();
        (before.is_empty() || before == ",").then(|| text[start..arrow].chars().count())
    });
    let Some(first) = columns.next() else {
        return 0;
    };
    let mut count = 1;
    for column in columns {
        if column != first {
            return 0;
        }
        count += 1;
    }
    if count >= 2 { first } else { 0 }
}

fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |i| i + 1)
}

/// A whole `port map (...)` (or `generic map`) for an instantiation that
/// has none, one association per line.
fn new_map(
    text: &str,
    missing: &Missing,
    new: &[(String, String)],
    config: &Config,
) -> (TextRange, String) {
    let stmt = &missing.stmt;
    // After the unit's name, and after the generic map for a port map.
    let after = stmt
        .children()
        .filter(|n| {
            matches!(
                n.kind(),
                SyntaxKind::NAME | SyntaxKind::SELECTED_NAME | SyntaxKind::CALL_EXPR
            ) || (missing.ports && n.kind() == SyntaxKind::GENERIC_MAP_ASPECT)
        })
        .last()
        .map_or(stmt.text_range().end(), |n| n.text_range().end());
    let unit = if config.format.use_tabs {
        "\t".to_owned()
    } else {
        " ".repeat(config.format.indent as usize)
    };
    let stmt_start = stmt
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia())
        .map_or(stmt.text_range().start(), |t| t.text_range().start());
    let stmt_start = usize::from(stmt_start);
    let line = &text[line_start(text, stmt_start)..];
    let indent = &line[..line.len() - line.trim_start_matches([' ', '\t']).len()];
    // The generic map's own line, when it has one.
    let generic_line = stmt
        .children()
        .find(|n| n.kind() == SyntaxKind::GENERIC_MAP_ASPECT)
        .map(|g| usize::from(g.text_range().start()))
        .filter(|&g| text[..g].trim_end_matches([' ', '\t']).ends_with('\n'))
        .map(|g| &text[line_start(text, g)..g]);
    let map_indent = generic_line.map_or_else(|| format!("{indent}{unit}"), str::to_owned);
    let keyword = if missing.ports {
        "port map"
    } else {
        "generic map"
    };
    let keyword = if config.format.keyword_case == Case::Upper {
        keyword.to_ascii_uppercase()
    } else {
        keyword.to_owned()
    };
    let space = if config.format.space_after_keyword {
        " "
    } else {
        ""
    };
    let column = if config.format.align.association {
        new.iter()
            .map(|(f, _)| f.chars().count() + 1)
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    let indent = format!("{map_indent}{unit}");
    // The comma left of the elements, which line up.
    let separator = match config.format.association_separator {
        Separator::Leading if indent.len() >= 2 && !config.format.use_tabs => {
            format!("\n{}, ", &indent[..indent.len() - 2])
        }
        _ => format!(",\n{indent}"),
    };
    let associations: Vec<String> = new.iter().map(|(f, a)| association(f, a, column)).collect();
    (
        TextRange::empty(after),
        format!(
            "\n{map_indent}{keyword}{space}(\n{indent}{}\n{map_indent})",
            associations.join(&separator)
        ),
    )
}

/// `raw` formatted, as one edit, when the statement at `stmt` was
/// formatted already: the fix then keeps it formatted, aligned and with
/// the commas where the formatter puts them. `None` when it was not (the
/// fix leaves the rest of it alone) or the file does not format.
fn formatted(
    path: &std::path::Path,
    text: &str,
    stmt: TextRange,
    raw: &[(TextRange, String)],
    config: &Config,
) -> Option<Vec<(TextRange, String)>> {
    let range = usize::from(stmt.start())..usize::from(stmt.end());
    let no_root = std::path::PathBuf::new;
    let before = crate::format::range(Some(path), text, range.clone(), config, no_root).ok()?;
    if !before.is_empty() {
        return None;
    }
    let mut edits: Vec<vfl_fmt::Edit> = raw
        .iter()
        .map(|(r, t)| vfl_fmt::Edit {
            range: usize::from(r.start())..usize::from(r.end()),
            text: t.clone(),
        })
        .collect();
    edits.sort_by_key(|e| e.range.start);
    let grown: usize = raw.iter().map(|(_, t)| t.len()).sum();
    let fixed = vfl_fmt::apply_edits(text, &edits);
    let after = crate::format::range(
        Some(path),
        &fixed,
        range.start..range.end + grown,
        config,
        no_root,
    )
    .ok()?;
    let result = vfl_fmt::apply_edits(&fixed, &after);
    // One edit covering what changed.
    let prefix = text
        .bytes()
        .zip(result.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    let mut suffix = text[prefix..]
        .bytes()
        .rev()
        .zip(result[prefix..].bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let mut prefix = prefix;
    while !text.is_char_boundary(prefix) {
        prefix -= 1;
    }
    while !text.is_char_boundary(text.len() - suffix) {
        suffix -= 1;
    }
    let (old_end, new_end) = (text.len() - suffix, result.len() - suffix);
    Some(vec![(
        TextRange::new((prefix as u32).into(), (old_end as u32).into()),
        result[prefix..new_end].to_owned(),
    )])
}
