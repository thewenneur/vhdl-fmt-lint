//! Navigation, refactoring and fixes: go to definition and back, find
//! references, symbols in the file or the project, rename, quick fixes,
//! code generated from an entity, and highlighting the uses of the name
//! at the caret.
//!
//! Lists of places or fixes are Scintilla user lists; picking an item
//! comes back as `SCN_USERLISTSELECTION`. The answers come from the
//! worker's thread, see [`Here::ask`].

#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Arc;

use vfl_lsp::editor::{FileEdits, Location, QuickFix, Use};
use vfl_lsp::generate::Kind;

use vfl_config::Level;

use crate::engine::{BufferText, Severity};
use crate::plugin::{
    Here, Trigger, USE_INDICATOR, WRITE_INDICATOR, buffer_path, c_string, caret, current_buffer,
    current_scintilla, guard, here, indicators, is_vhdl, lint_current, npp, post, read_buffer,
    replace_text, request, with,
};
use crate::win::*;
use crate::worker::Key;

/// The list type of our user lists, to tell them from other plugins'.
const LIST_ID: i32 = 0x7F1;
/// How long the caret must rest before the uses of its name are shown.
const HIGHLIGHT_DELAY_MS: u32 = 250;
/// How many places Go back remembers.
const MAX_BACK: usize = 64;

#[derive(Default)]
pub(crate) struct ToolState {
    /// The user list on screen and what each item does.
    list: RefCell<Option<List>>,
    /// Where Go to definition and the symbol lists came from.
    back: RefCell<Vec<(PathBuf, usize)>>,
    highlight_timer: Cell<usize>,
    /// The buffer and caret the uses are shown for.
    highlighted: Cell<Option<(usize, usize)>>,
}

struct List {
    /// The buffer the list was shown in, and its text then.
    buffer: usize,
    text: Arc<BufferText>,
    /// Each item's label, in the buffer's encoding, and its action.
    items: Vec<(Vec<u8>, Action)>,
}

enum Action {
    Go(Location),
    /// Select a range of the buffer the list was shown in.
    Select(Range<usize>),
    Fix(QuickFix),
    /// Generate code from an entity, by qualified name.
    Generate(Kind, String),
}

fn tell(text: &str) {
    message_box(npp(), text, MB_ICONINFORMATION);
}

// Lists.

/// Shows `items` in a list at the caret; picking one runs its action.
fn show_list(here: &Here, items: Vec<(String, Action)>) {
    // Labels must differ to tell which was picked.
    let mut seen = std::collections::HashMap::<String, usize>::new();
    let items: Vec<(Vec<u8>, Action)> = items
        .into_iter()
        .map(|(label, action)| {
            let label = label.replace(['\n', '\r', '\0'], " ");
            let count = seen.entry(label.clone()).or_default();
            *count += 1;
            let label = if *count > 1 {
                format!("{label} ({count})")
            } else {
                label
            };
            (here.buffer.encode(&label), action)
        })
        .collect();
    let list = c_string(
        items
            .iter()
            .map(|(l, _)| l.as_slice())
            .collect::<Vec<_>>()
            .join(&b'\n'),
    );
    let sci = here.sci;
    let separator = send(sci, SCI_AUTOCGETSEPARATOR, 0, 0);
    let order = send(sci, SCI_AUTOCGETORDER, 0, 0);
    send(sci, SCI_AUTOCSETSEPARATOR, b'\n' as usize, 0);
    // Keep our order: places in file order, symbols as nested.
    send(sci, SCI_AUTOCSETORDER, SC_ORDER_CUSTOM, 0);
    send(sci, SCI_AUTOCSETIGNORECASE, 1, 0);
    send(
        sci,
        SCI_USERLISTSHOW,
        LIST_ID as usize,
        list.as_ptr() as isize,
    );
    send(sci, SCI_AUTOCSETSEPARATOR, separator.max(0) as usize, 0);
    send(sci, SCI_AUTOCSETORDER, order.max(0) as usize, 0);
    with(|s| {
        *s.tools.list.borrow_mut() = Some(List {
            buffer: here.id,
            text: here.buffer.clone(),
            items,
        })
    });
}

/// An item of one of our lists was picked.
pub(crate) fn list_selected(n: &ScNotification) {
    if n.list_type != LIST_ID || n.text.is_null() {
        return;
    }
    // SAFETY: Scintilla passes the picked item's NUL-terminated text.
    let label = unsafe { std::ffi::CStr::from_ptr(n.text.cast()) }.to_bytes();
    let Some(list) = with(|s| s.tools.list.borrow_mut().take()) else {
        return;
    };
    let Some((_, action)) = list.items.into_iter().find(|(l, _)| l == label) else {
        return;
    };
    match action {
        Action::Go(location) => open_location(&location, true),
        Action::Select(range) => {
            let here = here();
            if here.id == list.buffer && here.buffer.text == list.text.text {
                remember(&here);
                select(here.sci, &here.buffer, range);
            }
        }
        Action::Fix(fix) => {
            let here = here();
            if here.id != list.buffer || here.buffer.text != list.text.text {
                tell("The file changed since the fixes were listed. Try again.");
                return;
            }
            apply_fix(here, fix);
        }
        Action::Generate(kind, entity) => generate_from(kind, &entity),
    }
}

// Navigation.

/// Remembers the caret, for Go back.
fn remember(here: &Here) {
    if let Some(path) = here.path() {
        with(|s| {
            let mut back = s.tools.back.borrow_mut();
            if back.len() >= MAX_BACK {
                back.remove(0);
            }
            back.push((path.to_owned(), here.position));
        });
    }
}

/// Opens `path` in Notepad++, or switches to it. Returns whether it is now
/// the current file.
fn open(path: &Path) -> bool {
    let wide_path = wide(&path.to_string_lossy());
    send(npp(), NPPM_DOOPEN, 0, wide_path.as_ptr() as isize) != 0
        && buffer_path(current_buffer()).as_deref() == Some(path)
}

fn select(sci: Hwnd, buffer: &BufferText, range: Range<usize>) {
    let start = buffer.position(range.start);
    let end = buffer.position(range.end);
    let line = send(sci, SCI_LINEFROMPOSITION, start, 0).max(0) as usize;
    send(sci, SCI_ENSUREVISIBLE, line, 0);
    send(sci, SCI_SETSEL, end, start as isize);
    send(sci, SCI_SCROLLCARET, 0, 0);
}

fn open_location(location: &Location, remember_here: bool) {
    let here = here();
    if remember_here {
        remember(&here);
    }
    if here.path.as_deref() != Some(location.path.as_path()) && !open(&location.path) {
        tell(&format!("Could not open {}.", location.path.display()));
        return;
    }
    let (sci, _) = current_scintilla();
    let buffer = read_buffer(sci);
    select(sci, &buffer, location.range.clone());
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// `top.vhd:12: n <= width;`
fn location_label(location: &Location) -> String {
    format!(
        "{}:{}: {}",
        file_name(&location.path),
        location.line,
        location.line_text
    )
}

pub(crate) fn go_to_definition() {
    here().ask(
        Key::Command,
        |engine, path, buffer, offset| engine.definition(path, &buffer.text, offset),
        |here, found| match found.as_slice() {
            [] => tell("No declaration found for the name at the caret."),
            [one] => open_location(one, true),
            _ => show_list(
                &here,
                found
                    .into_iter()
                    .map(|l| (location_label(&l), Action::Go(l)))
                    .collect(),
            ),
        },
    );
}

pub(crate) fn go_back() {
    let Some((path, position)) = with(|s| s.tools.back.borrow_mut().pop()) else {
        tell("Nowhere to go back to.");
        return;
    };
    if buffer_path(current_buffer()).as_deref() != Some(path.as_path()) && !open(&path) {
        return;
    }
    let (sci, _) = current_scintilla();
    crate::plugin::go_to(sci, position);
}

pub(crate) fn find_references() {
    here().ask(
        Key::Command,
        |engine, path, buffer, offset| engine.references(path, &buffer.text, offset),
        |here, found| {
            if found.is_empty() {
                tell("No references found for the name at the caret.");
                return;
            }
            show_list(
                &here,
                found
                    .into_iter()
                    .map(|l| (location_label(&l), Action::Go(l)))
                    .collect(),
            );
        },
    );
}

pub(crate) fn outline() {
    here().ask(
        Key::Command,
        |engine, path, buffer, _| {
            let symbols = engine.outline(path, &buffer.text);
            let line_starts: Vec<usize> = std::iter::once(0)
                .chain(buffer.text.match_indices('\n').map(|(i, _)| i + 1))
                .collect();
            symbols
                .into_iter()
                .map(|symbol| {
                    let line = line_starts.partition_point(|&s| s <= symbol.range.start);
                    (
                        format!(
                            "{}{}  ({}, line {line})",
                            "    ".repeat(symbol.depth),
                            symbol.name,
                            symbol.kind
                        ),
                        symbol.range,
                    )
                })
                .collect::<Vec<_>>()
        },
        |here, items| {
            if items.is_empty() {
                tell("No declarations in this file.");
                return;
            }
            let items = items
                .into_iter()
                .map(|(label, range)| (label, Action::Select(range)))
                .collect();
            show_list(&here, items);
        },
    );
}

/// Lists the problems of every file of the project, like the language
/// server's problems view.
pub(crate) fn project_problems() {
    here().ask(
        Key::Command,
        |engine, path, buffer, _| engine.project_problems(path, &buffer.text),
        |here, problems| {
            if problems.is_empty() {
                tell("No problems found in the project.");
                return;
            }
            let items = problems
                .into_iter()
                .map(|p| {
                    let severity = match p.level {
                        Level::Error => Severity::Error,
                        Level::Warn => Severity::Warning,
                        _ => Severity::Info,
                    };
                    (
                        format!(
                            "{}:{}: {}: {} [{}]",
                            file_name(&p.location.path),
                            p.location.line,
                            severity.label(),
                            p.message,
                            p.rule
                        ),
                        Action::Go(p.location),
                    )
                })
                .collect();
            show_list(&here, items);
        },
    );
}

/// The word at the caret, to start a search with.
fn word_at_caret(here: &Here) -> String {
    let start = send(here.sci, SCI_WORDSTARTPOSITION, here.position, 1).max(0) as usize;
    let end = send(here.sci, SCI_WORDENDPOSITION, here.position, 1).max(0) as usize;
    let (start, end) = (here.buffer.offset(start), here.buffer.offset(end));
    here.buffer
        .text
        .get(start..end)
        .unwrap_or_default()
        .to_owned()
}

pub(crate) fn project_symbols() {
    let here = here();
    let Some(query) = input_box(
        npp(),
        "Go to symbol in project",
        "Names containing:",
        &word_at_caret(&here),
    ) else {
        return;
    };
    let query = query.trim().to_owned();
    here.ask(
        Key::Command,
        {
            let query = query.clone();
            move |engine, path, buffer, _| engine.project_symbols(path, &buffer.text, &query)
        },
        move |here, found| {
            if found.is_empty() {
                tell(&format!(
                    "No design unit, type, subprogram or package object named like `{query}`."
                ));
                return;
            }
            let items = found
                .into_iter()
                .map(|symbol| {
                    let place = match &symbol.container {
                        Some(container) => format!("{} in {container}", symbol.kind),
                        None => symbol.kind.to_owned(),
                    };
                    (
                        format!(
                            "{}  ({place}, {}:{})",
                            symbol.name,
                            file_name(&symbol.location.path),
                            symbol.location.line
                        ),
                        Action::Go(symbol.location),
                    )
                })
                .collect();
            show_list(&here, items);
        },
    );
}

// Editing.

/// Replaces byte ranges of `buffer`'s text, as one undo step.
fn apply_edits(sci: Hwnd, buffer: &BufferText, edits: &[(Range<usize>, String)]) {
    let mut edits: Vec<&(Range<usize>, String)> = edits.iter().collect();
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    send(sci, SCI_BEGINUNDOACTION, 0, 0);
    for (range, text) in edits {
        let start = buffer.position(range.start);
        let end = buffer.position(range.end);
        let bytes = buffer.encode(text);
        send(sci, SCI_SETTARGETRANGE, start, end as isize);
        send(sci, SCI_REPLACETARGET, bytes.len(), bytes.as_ptr() as isize);
    }
    send(sci, SCI_ENDUNDOACTION, 0, 0);
}

fn fix_label(fix: &QuickFix) -> String {
    // The titles of unsafe fixes say so already.
    format!("{}  [{}]", fix.title, fix.rule)
}

/// Applies a fix to the buffer it was listed for, which is in front.
fn apply_fix(here: Here, fix: QuickFix) {
    match fix.rename {
        Some(new_name) => rename_to(here, fix.at, new_name),
        None => {
            apply_edits(here.sci, &here.buffer, &fix.edits);
            lint_current(Trigger::Refresh);
        }
    }
}

/// Tells that the buffer changed while an answer was computed.
fn changed() {
    tell("The file changed while vfl was working on it. Try again.");
}

pub(crate) fn quick_fix() {
    let here = here();
    let anchor = send(here.sci, SCI_GETANCHOR, 0, 0).max(0) as usize;
    here.ask(
        Key::Command,
        move |engine, path, buffer, offset| {
            // The problems the selection touches, or those at the caret.
            let other = buffer.offset(anchor);
            let (start, end) = (offset.min(other), offset.max(other));
            engine.quick_fixes(path, &buffer.text, start, end)
        },
        |here, fixes| {
            let fixing: Vec<&QuickFix> = fixes.iter().filter(|f| !f.silences).collect();
            match fixing.as_slice() {
                [] if fixes.is_empty() => tell("No fix for a problem at the caret."),
                // A single safe fix is applied; the list offers the others.
                [one] if one.safe && one.rename.is_none() => {
                    if !here.is_current() {
                        return changed();
                    }
                    let one = (*one).clone();
                    apply_fix(here, one);
                }
                _ => show_list(
                    &here,
                    fixes
                        .into_iter()
                        .map(|f| (fix_label(&f), Action::Fix(f)))
                        .collect(),
                ),
            }
        },
    );
}

pub(crate) fn fix_all(trigger: Trigger) {
    here().ask(
        Key::Command,
        |engine, path, buffer, _| engine.fix_all(path, &buffer.text),
        move |here, fixed| match fixed {
            Some(_) if !here.is_current() => changed(),
            Some((fixed, _)) => {
                replace_text(here.sci, &here.buffer, &fixed);
                lint_current(Trigger::Refresh);
            }
            None if trigger == Trigger::Command => tell("No problem with a safe fix."),
            None => {}
        },
    );
}

pub(crate) fn rename() {
    here().ask(
        Key::Command,
        |engine, path, buffer, offset| engine.prepare_rename(path, &buffer.text, offset),
        |here, range| match range {
            Ok(range) => ask_new_name(here, range),
            Err(why) => tell(&format!("Cannot rename: {why}.")),
        },
    );
}

fn ask_new_name(here: Here, range: Range<usize>) {
    let old = here.buffer.text[range].to_owned();
    let Some(new) = input_box(
        npp(),
        "Rename symbol",
        &format!("New name for {old}:"),
        &old,
    ) else {
        return;
    };
    let new = new.trim().to_owned();
    if new == old {
        return;
    }
    let offset = here.offset();
    rename_to(here, offset, new);
}

/// Renames the name at byte `offset` of the buffer in front to `new`,
/// asking first when other files change.
fn rename_to(here: Here, offset: usize, new: String) {
    let path = here.path().map(Path::to_owned);
    let buffer = here.buffer.clone();
    let new_name = new.clone();
    request(
        Some(Key::Command),
        move |engine| {
            let path = path.as_deref();
            let old = engine
                .prepare_rename(path, &buffer.text, offset)
                .map_or_else(|_| String::new(), |range| buffer.text[range].to_owned());
            (old, engine.rename(path, &buffer.text, offset, &new_name))
        },
        move |(old, files)| match files {
            Ok(files) => apply_rename(&here, &old, &new, files),
            Err(why) => tell(&format!("Cannot rename: {why}.")),
        },
    );
}

fn apply_rename(here: &Here, old: &str, new: &str, files: Vec<FileEdits>) {
    let places: usize = files.iter().map(|f| f.edits.len()).sum();
    let elsewhere = files
        .iter()
        .any(|f| here.path() != Some(f.path.as_path()) && here.path().is_some());
    if elsewhere
        && !confirm(
            npp(),
            &format!(
                "Rename {old} to {new} in {places} places across {} files?\n\n\
                 The other files are opened in Notepad++ and left unsaved, so you \
                 can review and undo the change in each.",
                files.len()
            ),
        )
    {
        return;
    }
    let mut skipped = Vec::new();
    for file in &files {
        // Unsaved buffers are renamed in place; files by path, since an
        // earlier file of the rename may now be in front.
        let in_front = here.path().is_none()
            || buffer_path(current_buffer()).as_deref() == Some(file.path.as_path());
        if !in_front && !open(&file.path) {
            skipped.push(file.path.clone());
            continue;
        }
        let (sci, _) = current_scintilla();
        let buffer = read_buffer(sci);
        if buffer.text != file.text {
            skipped.push(file.path.clone());
            continue;
        }
        apply_edits(sci, &buffer, &file.edits);
        // The file may go to the back before it is linted again.
        let renamed = read_buffer(sci).text;
        let path = here.path().map(|_| file.path.clone());
        post(move |engine| engine.update(path.as_deref(), &renamed));
    }
    if let Some(path) = here.path()
        && buffer_path(current_buffer()).as_deref() != Some(path)
    {
        let wide_path = wide(&path.to_string_lossy());
        send(npp(), NPPM_SWITCHTOFILE, 0, wide_path.as_ptr() as isize);
    }
    lint_current(Trigger::Refresh);
    if !skipped.is_empty() {
        let names: Vec<String> = skipped.iter().map(|p| p.display().to_string()).collect();
        message_box(
            npp(),
            &format!(
                "Not renamed in these files, which changed since they were analyzed:\n\n{}",
                names.join("\n")
            ),
            MB_ICONWARNING,
        );
    }
}

// Code generation.

/// Asks which entity of the project to generate `kind` from, those of the
/// current file first.
pub(crate) fn generate(kind: Kind) {
    here().ask(
        Key::Command,
        |engine, path, buffer, _| engine.entities(path, &buffer.text),
        move |here, mut entities| {
            entities.sort_by_key(|e| here.path() != Some(e.path.as_path()));
            match entities.as_slice() {
                [] => tell("No entity in this project."),
                [one] => generate_from(kind, &one.qualified()),
                _ => show_list(
                    &here,
                    entities
                        .iter()
                        .map(|e| {
                            (
                                format!("{}  ({}, {})", e.name, e.library, file_name(&e.path)),
                                Action::Generate(kind, e.qualified()),
                            )
                        })
                        .collect(),
                ),
            }
        },
    );
}

fn generate_from(kind: Kind, entity: &str) {
    let entity = entity.to_owned();
    here().ask(
        Key::Command,
        move |engine, path, buffer, _| engine.generate(path, &buffer.text, kind, &entity),
        |_, generated| {
            let generated = match generated {
                Ok(generated) => generated,
                Err(why) => {
                    tell(&format!("Cannot generate: {why}."));
                    return;
                }
            };
            match generated.path {
                Some(path) => {
                    if path.exists()
                        && !confirm(npp(), &format!("{} exists. Replace it?", path.display()))
                    {
                        return;
                    }
                    if let Err(err) = std::fs::write(&path, &generated.text) {
                        tell(&format!("Could not write {}: {err}.", path.display()));
                        return;
                    }
                    if !open(&path) {
                        tell(&format!("Wrote {}.", path.display()));
                    }
                }
                // Where the caret is now: the code does not depend on it.
                None => insert_at_caret(&here(), &generated.text),
            }
        },
    );
}

/// Inserts lines at the caret, each after the first indented like the
/// caret's line.
fn insert_at_caret(here: &Here, text: &str) {
    let offset = here.offset();
    let line_start = here.buffer.text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line = &here.buffer.text[line_start..];
    let indent = &line[..line.len() - line.trim_start_matches([' ', '\t']).len()];
    let indent = &indent[..indent.len().min(offset - line_start)];
    let newline = if here.buffer.text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let text = text
        .trim_end_matches('\n')
        .replace('\n', &format!("{newline}{indent}"));
    apply_edits(here.sci, &here.buffer, &[(offset..offset, text.clone())]);
    let end = here.buffer.position(offset) + here.buffer.encoded_len(&text);
    send(here.sci, SCI_GOTOPOS, end, 0);
}

// Highlighting the uses of the name at the caret.

/// The caret may have moved: show the uses of its name once it rests.
pub(crate) fn caret_moved(sci: Hwnd) {
    let now = (current_buffer(), caret(sci));
    if with(|s| s.tools.highlighted.get()) != Some(now) {
        schedule_highlight();
    }
}

pub(crate) fn schedule_highlight() {
    cancel_highlight();
    // SAFETY: a thread timer with a plain callback.
    let id = unsafe {
        SetTimer(
            ptr::null_mut(),
            0,
            HIGHLIGHT_DELAY_MS,
            Some(on_highlight_timer),
        )
    };
    with(|s| s.tools.highlight_timer.set(id));
}

pub(crate) fn cancel_highlight() {
    let id = with(|s| s.tools.highlight_timer.replace(0));
    if id != 0 {
        // SAFETY: `id` is a thread timer this plugin created.
        unsafe { KillTimer(ptr::null_mut(), id) };
    }
}

unsafe extern "system" fn on_highlight_timer(_hwnd: Hwnd, _msg: u32, _id: usize, _time: u32) {
    guard(|| {
        cancel_highlight();
        highlight_uses();
    });
}

fn highlight_uses() {
    let (sci, _) = current_scintilla();
    let id = current_buffer();
    let position = caret(sci);
    with(|s| s.tools.highlighted.set(Some((id, position))));
    let indicators = indicators();
    let (read, write) = (indicators[USE_INDICATOR], indicators[WRITE_INDICATOR]);
    let len = send(sci, SCI_GETLENGTH, 0, 0);
    for indicator in [read, write] {
        send(sci, SCI_SETINDICATORCURRENT, indicator, 0);
        send(sci, SCI_INDICATORCLEARRANGE, 0, len);
    }
    let settings = with(|s| s.settings.get());
    let path = buffer_path(id);
    let library = path.as_deref().is_some_and(crate::engine::is_library_copy);
    if !settings.highlight || !is_vhdl(id) || library {
        return;
    }
    // Only when the caret is on a name: nothing for a selection.
    if send(sci, SCI_GETSELECTIONSTART, 0, 0) != send(sci, SCI_GETSELECTIONEND, 0, 0) {
        return;
    }
    here().ask(
        Key::Highlight,
        |engine, path, buffer, offset| {
            engine
                .highlights(path, &buffer.text, offset)
                .into_iter()
                .map(|(range, kind)| {
                    (
                        buffer.position(range.start),
                        buffer.position(range.end),
                        kind == Use::Write,
                    )
                })
                .collect::<Vec<_>>()
        },
        move |here, uses| {
            if uses.len() < 2
                || !here.is_current()
                || caret(here.sci) != here.position
                || with(|s| s.tools.highlighted.get()) != Some((here.id, here.position))
            {
                return;
            }
            let sci = here.sci;
            for (indicator, color) in [(read, 0xC08040), (write, 0x3080E0)] {
                send(sci, SCI_INDICSETSTYLE, indicator, INDIC_ROUNDBOX);
                send(sci, SCI_INDICSETFORE, indicator, color);
                send(sci, SCI_INDICSETALPHA, indicator, 60);
                send(sci, SCI_INDICSETOUTLINEALPHA, indicator, 140);
                send(sci, SCI_INDICSETUNDER, indicator, 1);
            }
            for (start, end, written) in uses {
                let indicator = if written { write } else { read };
                send(sci, SCI_SETINDICATORCURRENT, indicator, 0);
                send(sci, SCI_INDICATORFILLRANGE, start, (end - start) as isize);
            }
        },
    );
}

// Documentation.

/// The current file, if it is saved: the documentation is made from the
/// files on disk.
fn saved_file() -> Option<PathBuf> {
    let path = buffer_path(current_buffer()).filter(|p| p.is_absolute() && p.is_file());
    if path.is_none() {
        tell("Save the file first: the documentation is made from the files on disk.");
    }
    path
}

/// Writes the documentation of the current file's project where `[doc]`
/// says, then opens its index: an HTML one in the browser, others here.
pub(crate) fn generate_docs() {
    let Some(file) = saved_file() else {
        return;
    };
    match crate::docs::write(&file, None) {
        Ok(written) => {
            let opened = if written.html {
                shell_open(npp(), &written.index)
            } else {
                open(&written.index)
            };
            if !opened {
                tell(&format!("Wrote {}.", written.index.display()));
            }
        }
        Err(why) => tell(&format!("Could not write the documentation: {why}.")),
    }
}

/// Writes the HTML documentation of the current file's project to a
/// temporary directory and opens the page of the file's entity, with its
/// diagrams, in the browser.
pub(crate) fn show_diagrams() {
    let Some(file) = saved_file() else {
        return;
    };
    match crate::docs::write(&file, Some(&crate::docs::diagrams_dir())) {
        Ok(written) => {
            if !shell_open(npp(), &written.page) {
                tell(&format!("Could not open {}.", written.page.display()));
            }
        }
        Err(why) => tell(&format!("Could not draw the diagrams: {why}.")),
    }
}
