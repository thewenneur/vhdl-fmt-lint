//! Navigation, refactoring and fixes: go to definition and back, find
//! references, symbols in the file or the project, rename, quick fixes,
//! and highlighting the uses of the name at the caret.
//!
//! Lists of places or fixes are Scintilla user lists; picking an item
//! comes back as `SCN_USERLISTSELECTION`.

#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::ptr;

use vfl_lsp::editor::{Location, QuickFix, Use};

use crate::engine::{BufferText, Engine};
use crate::plugin::{
    Trigger, USE_INDICATOR, WRITE_INDICATOR, buffer_path, c_string, caret, current_buffer,
    current_scintilla, guard, indicators, is_vhdl, lint_current, npp, read_buffer, replace_text,
    with,
};
use crate::win::*;

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
    text: String,
    /// Each item's label, in the buffer's encoding, and its action.
    items: Vec<(Vec<u8>, Action)>,
}

enum Action {
    Go(Location),
    /// Select a range of the buffer the list was shown in.
    Select(Range<usize>),
    Fix(QuickFix),
}

/// The current buffer, read for a query.
struct Here {
    sci: Hwnd,
    id: usize,
    path: Option<PathBuf>,
    buffer: BufferText,
    position: usize,
    offset: usize,
}

fn here() -> Here {
    let (sci, _) = current_scintilla();
    let id = current_buffer();
    let buffer = read_buffer(sci);
    let position = caret(sci);
    let offset = buffer.offset(position);
    Here {
        sci,
        id,
        path: buffer_path(id),
        buffer,
        position,
        offset,
    }
}

impl Here {
    /// The path to give the engine: `None` for unsaved buffers.
    fn path(&self) -> Option<&Path> {
        self.path.as_deref().filter(|p| p.is_absolute())
    }
}

fn engine<T>(f: impl FnOnce(&mut Engine) -> T) -> Option<T> {
    with(|s| s.engine.try_borrow_mut().ok().map(|mut e| f(&mut e)))
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
            text: here.buffer.text.clone(),
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
            if here.id == list.buffer && here.buffer.text == list.text {
                remember(&here);
                select(here.sci, &here.buffer, range);
            }
        }
        Action::Fix(fix) => {
            let here = here();
            if here.id != list.buffer || here.buffer.text != list.text {
                tell("The file changed since the fixes were listed. Try again.");
                return;
            }
            apply_edits(here.sci, &here.buffer, &fix.edits);
            lint_current(Trigger::Refresh);
        }
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
    let here = here();
    let found =
        engine(|e| e.definition(here.path(), &here.buffer.text, here.offset)).unwrap_or_default();
    match found.as_slice() {
        [] => tell("No declaration found for the name at the caret."),
        [one] => open_location(one, true),
        _ => show_list(
            &here,
            found
                .into_iter()
                .map(|l| (location_label(&l), Action::Go(l)))
                .collect(),
        ),
    }
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
    let here = here();
    let found =
        engine(|e| e.references(here.path(), &here.buffer.text, here.offset)).unwrap_or_default();
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
}

pub(crate) fn outline() {
    let here = here();
    let symbols = engine(|e| e.outline(here.path(), &here.buffer.text)).unwrap_or_default();
    if symbols.is_empty() {
        tell("No declarations in this file.");
        return;
    }
    let items = symbols
        .into_iter()
        .map(|symbol| {
            let position = here.buffer.position(symbol.range.start);
            let line = send(here.sci, SCI_LINEFROMPOSITION, position, 0).max(0) + 1;
            (
                format!(
                    "{}{}  ({}, line {line})",
                    "    ".repeat(symbol.depth),
                    symbol.name,
                    symbol.kind
                ),
                Action::Select(symbol.range),
            )
        })
        .collect();
    show_list(&here, items);
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
    let found = engine(|e| e.project_symbols(here.path(), &here.buffer.text, query.trim()))
        .unwrap_or_default();
    if found.is_empty() {
        tell(&format!(
            "No design unit, type, subprogram or package object named like `{}`.",
            query.trim()
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
    if fix.safe {
        format!("{}  [{}]", fix.title, fix.rule)
    } else {
        format!("{} (may change behavior)  [{}]", fix.title, fix.rule)
    }
}

pub(crate) fn quick_fix() {
    let here = here();
    let fixes = engine(|e| e.quick_fixes(here.path(), &here.buffer.text, here.offset, here.offset))
        .unwrap_or_default();
    match fixes.as_slice() {
        [] => tell("No fix for a problem at the caret."),
        [one] if one.safe => {
            apply_edits(here.sci, &here.buffer, &one.edits);
            lint_current(Trigger::Refresh);
        }
        _ => show_list(
            &here,
            fixes
                .into_iter()
                .map(|f| (fix_label(&f), Action::Fix(f)))
                .collect(),
        ),
    }
}

pub(crate) fn fix_all(trigger: Trigger) {
    let here = here();
    match engine(|e| e.fix_all(here.path(), &here.buffer.text)).flatten() {
        Some((fixed, _)) => {
            replace_text(here.sci, &here.buffer, &fixed);
            lint_current(Trigger::Refresh);
        }
        None if trigger == Trigger::Command => tell("No problem with a safe fix."),
        None => {}
    }
}

pub(crate) fn rename() {
    let here = here();
    let range = match engine(|e| e.prepare_rename(here.path(), &here.buffer.text, here.offset)) {
        Some(Ok(range)) => range,
        Some(Err(why)) => {
            tell(&format!("Cannot rename: {why}."));
            return;
        }
        None => return,
    };
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
    let files = match engine(|e| e.rename(here.path(), &here.buffer.text, here.offset, &new)) {
        Some(Ok(files)) => files,
        Some(Err(why)) => {
            tell(&format!("Cannot rename: {why}."));
            return;
        }
        None => return,
    };
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
        let renamed = read_buffer(sci);
        engine(|e| e.update(here.path().map(|_| file.path.as_path()), &renamed.text));
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
    let here = here();
    let uses =
        engine(|e| e.highlights(here.path(), &here.buffer.text, here.offset)).unwrap_or_default();
    if uses.len() < 2 {
        return;
    }
    for (indicator, color) in [(read, 0xC08040), (write, 0x3080E0)] {
        send(sci, SCI_INDICSETSTYLE, indicator, INDIC_ROUNDBOX);
        send(sci, SCI_INDICSETFORE, indicator, color);
        send(sci, SCI_INDICSETALPHA, indicator, 60);
        send(sci, SCI_INDICSETOUTLINEALPHA, indicator, 140);
        send(sci, SCI_INDICSETUNDER, indicator, 1);
    }
    for (range, kind) in uses {
        let start = here.buffer.position(range.start);
        let end = here.buffer.position(range.end);
        let indicator = if kind == Use::Write { write } else { read };
        send(sci, SCI_SETINDICATORCURRENT, indicator, 0);
        send(sci, SCI_INDICATORFILLRANGE, start, (end - start) as isize);
    }
}
