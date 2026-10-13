//! The fixes offered for the problems in a range: each rule's own fix,
//! renaming a declaration to follow a naming convention, importing a name
//! that is declared in a package, and silencing the rule with a
//! `-- vfl-ignore` comment.

use vfl_sema::{DeclKind, FileId};
use vfl_syntax::{SyntaxKind, TextRange, TextSize};

use crate::world::World;

/// Replacements, by file.
pub(crate) type Edits = Vec<(FileId, TextRange, String)>;

/// A fix for one problem.
pub(crate) struct Action {
    pub title: String,
    /// The problem's index in `world.lint`.
    pub problem: usize,
    /// Whether it keeps the design's behavior.
    pub safe: bool,
    /// The fix to apply without asking, when it is the only one.
    pub preferred: bool,
    /// A rename everywhere rather than a fix of the problem's file.
    pub rename: Option<String>,
    /// Silences the problem rather than fixing it.
    pub silences: bool,
    pub edits: Edits,
}

/// The fixes for the problems of `file` that touch `start..end`, the
/// rules' own fixes first.
pub(crate) fn actions(world: &World, file: FileId, start: TextSize, end: TextSize) -> Vec<Action> {
    let mut fixes = Vec::new();
    let mut silencing = Vec::new();
    for (index, d) in world.lint.iter().enumerate() {
        if d.file != file || d.range.end() < start || d.range.start() > end {
            continue;
        }
        if let Some(fix) = &d.fix {
            fixes.push(Action {
                title: if fix.safe {
                    fix.title.clone()
                } else {
                    format!("{} (may change behavior)", fix.title)
                },
                problem: index,
                safe: fix.safe,
                preferred: fix.safe,
                rename: None,
                silences: false,
                edits: fix
                    .edits
                    .iter()
                    .map(|e| (file, e.range, e.text.clone()))
                    .collect(),
            });
        }
        if let Some(new_name) = &d.rename
            && let Ok((_, symbol)) =
                crate::handlers::rename_targets_at(world, file, d.range.start())
            && let Ok(edits) = crate::handlers::rename_edits(world, &symbol, new_name)
        {
            let old = &world.doc(file).text[std::ops::Range::<usize>::from(d.range)];
            fixes.push(Action {
                title: format!("Rename `{old}` to `{new_name}` everywhere"),
                problem: index,
                safe: true,
                preferred: true,
                rename: Some(new_name.clone()),
                silences: false,
                edits,
            });
        }
        if d.rule == vfl_sema::codes::UNRESOLVED_NAME {
            fixes.extend(
                imports(world, file, d.range)
                    .into_iter()
                    .map(|(title, edits)| Action {
                        title,
                        problem: index,
                        safe: true,
                        preferred: false,
                        rename: None,
                        silences: false,
                        edits,
                    }),
            );
        }
        if d.rule != "syntax/error" {
            for (title, edit) in silence(world, file, d.range, d.rule) {
                if silencing.iter().any(|a: &Action| a.title == title) {
                    continue;
                }
                silencing.push(Action {
                    title,
                    problem: index,
                    safe: true,
                    preferred: false,
                    rename: None,
                    silences: true,
                    edits: vec![edit],
                });
            }
        }
    }
    fixes.extend(silencing);
    fixes
}

/// `use` clauses that would make the name at `range` visible: one per
/// package that declares it, in the project or the bundled libraries.
fn imports(world: &World, file: FileId, range: TextRange) -> Vec<(String, Edits)> {
    let analysis = &world.analysis;
    let doc = world.doc(file);
    let name = &doc.text[std::ops::Range::<usize>::from(range)];
    if name.contains('.') {
        return Vec::new();
    }
    let key = vfl_syntax::identifier_key(name);
    let root = analysis.files()[file as usize].syntax();
    let Some(design_unit) = root
        .children()
        .filter(|n| n.kind() == SyntaxKind::DESIGN_UNIT)
        .find(|n| n.text_range().contains_range(range))
    else {
        return Vec::new();
    };
    let here = analysis
        .file_units(file)
        .next()
        .map(|u| analysis.units()[u as usize].library.clone());
    let context = design_unit
        .children()
        .find(|n| n.kind() == SyntaxKind::CONTEXT_CLAUSE);
    // The clauses of the unit and of the units before it (an
    // architecture's entity, in the same file).
    let context_text: String = root
        .children()
        .filter(|n| n.kind() == SyntaxKind::DESIGN_UNIT && n.text_range().start() <= range.start())
        .filter_map(|u| {
            u.children()
                .find(|n| n.kind() == SyntaxKind::CONTEXT_CLAUSE)
        })
        .map(|c| c.text().to_string().to_ascii_lowercase())
        .collect();
    // After the last clause, or before the unit.
    let (at, before, after) = match context.as_ref().and_then(|c| c.last_child()) {
        Some(last) => (last.text_range().end(), "\n", ""),
        None => {
            let start = design_unit
                .first_token()
                .map(|t| {
                    let mut t = t;
                    while t.kind().is_trivia() {
                        match t.next_token() {
                            Some(next) => t = next,
                            None => break,
                        }
                    }
                    t.text_range().start()
                })
                .unwrap_or(design_unit.text_range().start());
            (start, "", "\n")
        }
    };
    let mut packages: Vec<(String, String)> = Vec::new();
    for unit in 0..analysis.units().len() as vfl_sema::UnitId {
        let Some(decl) = analysis.unit_decls(unit).first() else {
            continue;
        };
        if decl.kind != DeclKind::Package {
            continue;
        }
        let Some(members) = &decl.members else {
            continue;
        };
        if members.get(&key).is_empty() {
            continue;
        }
        let library = analysis.units()[unit as usize].library.clone();
        let library = if Some(&library) == here.as_ref() {
            "work".to_owned()
        } else {
            library
        };
        // The bundled sources are in upper case.
        let builtin = analysis.files()[decl.file as usize].builtin;
        let package = if builtin { &decl.name } else { &decl.spelling };
        let entry = (library, package.clone());
        if !packages.contains(&entry) {
            packages.push(entry);
        }
    }
    packages
        .sort_by_key(|(library, package)| (library != "work", library.clone(), package.clone()));
    packages
        .into_iter()
        .take(5)
        .map(|(library, package)| {
            let clause = format!("use {library}.{package}.all;");
            let declared = library == "work"
                || library == "std"
                || context_text
                    .split(';')
                    .any(|c| c.trim().starts_with("library") && c.contains(library.as_str()));
            let mut text = String::from(before);
            if !declared {
                text.push_str(&format!("library {library};\n"));
            }
            text.push_str(&clause);
            text.push_str(after);
            (
                format!("Add `{clause}`"),
                vec![(file, TextRange::empty(at), text)],
            )
        })
        .collect()
}

/// Comments that silence `rule` on the line of `range`, and in the whole
/// file.
fn silence(
    world: &World,
    file: FileId,
    range: TextRange,
    rule: &str,
) -> Vec<(String, (FileId, TextRange, String))> {
    let text = world.doc(file).text;
    let start = usize::from(range.start());
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let line = text[line_start..line_end].trim_end_matches('\r');
    let indent = &line[..line.len() - line.trim_start().len()];
    let mut out = Vec::new();
    let on_line = match ignore_comment(line) {
        // Add the rule to the comment that is there.
        Some(after_rules) => (line_start + after_rules, format!(", {rule}")),
        None if !line.contains("--") => (
            line_start + line.trim_end().len(),
            format!("  -- vfl-ignore {rule}"),
        ),
        // A comment ends the line: silence it from the line above.
        None => (line_start, format!("{indent}-- vfl-ignore {rule}\n")),
    };
    out.push((
        format!("Ignore `{rule}` on this line"),
        (
            file,
            TextRange::empty(TextSize::from(on_line.0 as u32)),
            on_line.1,
        ),
    ));
    out.push((
        format!("Ignore `{rule}` in this file"),
        (
            file,
            TextRange::empty(TextSize::from(0)),
            format!("-- vfl-ignore-file {rule}\n"),
        ),
    ));
    out
}

/// Where the rule list of a trailing `-- vfl-ignore rules` comment ends in
/// `line`, if it has one that names rules.
fn ignore_comment(line: &str) -> Option<usize> {
    let comment = line.find("-- vfl-ignore ")?;
    if line[comment..].starts_with("-- vfl-ignore-file") {
        return None;
    }
    let rules_start = comment + "-- vfl-ignore ".len();
    let rest = &line[rules_start..];
    let rules = rest.find("--").map_or(rest, |i| &rest[..i]).trim_end();
    (!rules.is_empty()).then_some(rules_start + rules.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailing_ignore_comments_are_extended() {
        let line = "  signal x : bit;  -- vfl-ignore sema/unused-signal -- legacy";
        let at = ignore_comment(line).unwrap();
        assert_eq!(
            &line[..at],
            "  signal x : bit;  -- vfl-ignore sema/unused-signal"
        );
        assert_eq!(ignore_comment("  x <= y; -- note"), None);
    }
}
