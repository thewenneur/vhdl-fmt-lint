//! The Notepad++ side: the exported plugin functions, the menu, and
//! showing problems in the editor.
//!
//! Notepad++ calls the plugin on its UI thread only, so the state lives in
//! a thread local.

#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::ptr;

use crate::engine::{BufferText, Engine, Severity, is_project_file, is_vhdl_path, minimal_edit};
use crate::win::*;

/// Menu entries, in order. `None` is a separator.
const MENU: &[(&str, Option<extern "C" fn()>)] = &[
    ("Format document", Some(cmd_format)),
    ("Lint document", Some(cmd_lint)),
    ("Next problem", Some(cmd_next)),
    ("Previous problem", Some(cmd_previous)),
    ("Clear problems", Some(cmd_clear)),
    ("", None),
    ("Format on save", Some(cmd_toggle_format_on_save)),
    ("Lint automatically", Some(cmd_toggle_auto_lint)),
    ("Show messages under lines", Some(cmd_toggle_inline)),
    ("", None),
    ("Reload project", Some(cmd_reload)),
    ("About", Some(cmd_about)),
];
const FORMAT_ON_SAVE: usize = 6;
const AUTO_LINT: usize = 7;
const INLINE: usize = 8;

/// How long typing must pause before the buffer is linted again.
const LINT_DELAY_MS: u32 = 400;
const DWELL_MS: isize = 500;
/// Indicators to use when Notepad++ is too old to hand them out.
const FALLBACK_INDICATORS: [usize; 3] = [9, 10, 11];

/// Squiggle and message colors (0x00BBGGRR), by severity.
fn color(severity: Severity) -> isize {
    match severity {
        Severity::Error => 0x4040E0,
        Severity::Warning => 0x008CD0,
        Severity::Info => 0xE08030,
    }
}

fn severity_index(severity: Severity) -> usize {
    match severity {
        Severity::Info => 0,
        Severity::Warning => 1,
        Severity::Error => 2,
    }
}

#[derive(Clone, Copy)]
struct Settings {
    format_on_save: bool,
    auto_lint: bool,
    inline_messages: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            format_on_save: false,
            auto_lint: true,
            inline_messages: true,
        }
    }
}

/// A problem placed in a buffer, at Scintilla positions.
struct Problem {
    start: usize,
    end: usize,
    severity: Severity,
    /// `error: message [rule]`, in the buffer's encoding.
    text: Vec<u8>,
}

struct State {
    npp: Cell<NppData>,
    engine: RefCell<Engine>,
    /// The problems of each linted buffer, by buffer id, sorted.
    problems: RefCell<HashMap<usize, Vec<Problem>>>,
    /// Buffers showing our messages under lines.
    annotated: RefCell<HashSet<usize>>,
    settings: Cell<Settings>,
    indicators: Cell<Option<[usize; 3]>>,
    /// The first of our three annotation styles, per view.
    styles: Cell<[Option<isize>; 2]>,
    funcs: Cell<*mut FuncItem>,
    timer: Cell<usize>,
    calltip: Cell<bool>,
}

thread_local! {
    static STATE: State = State {
        npp: Cell::new(NppData {
            npp: ptr::null_mut(),
            scintilla_main: ptr::null_mut(),
            scintilla_second: ptr::null_mut(),
        }),
        engine: RefCell::default(),
        problems: RefCell::default(),
        annotated: RefCell::default(),
        settings: Cell::default(),
        indicators: Cell::new(None),
        styles: Cell::new([None, None]),
        funcs: Cell::new(ptr::null_mut()),
        timer: Cell::new(0),
        calltip: Cell::new(false),
    };
}

fn with<T>(f: impl FnOnce(&State) -> T) -> T {
    STATE.with(f)
}

fn npp() -> Hwnd {
    with(|s| s.npp.get().npp)
}

/// Runs `f`, turning a panic into a message instead of taking Notepad++
/// down with it.
fn guard<T: Default>(f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            message_box(
                npp(),
                &format!("vfl hit an internal error and skipped this action.\n\n{message}"),
                MB_ICONWARNING,
            );
            T::default()
        }
    }
}

// The plugin interface.

#[unsafe(no_mangle)]
pub extern "C" fn isUnicode() -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn setInfo(data: NppData) {
    guard(|| {
        with(|s| s.npp.set(data));
        load_settings();
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn getName() -> *const u16 {
    static NAME: [u16; 4] = [b'v' as u16, b'f' as u16, b'l' as u16, 0];
    NAME.as_ptr()
}

/// # Safety
///
/// `count` must be valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getFuncsArray(count: *mut i32) -> *mut FuncItem {
    let funcs = with(|s| {
        if s.funcs.get().is_null() {
            let settings = s.settings.get();
            let items: Vec<FuncItem> = MENU
                .iter()
                .enumerate()
                .map(|(i, &(name, func))| {
                    let mut wide_name = [0u16; MENU_ITEM_SIZE];
                    for (slot, unit) in wide_name
                        .iter_mut()
                        .zip(name.encode_utf16().take(MENU_ITEM_SIZE - 1))
                    {
                        *slot = unit;
                    }
                    FuncItem {
                        name: wide_name,
                        func,
                        cmd_id: 0,
                        init_checked: match i {
                            FORMAT_ON_SAVE => settings.format_on_save,
                            AUTO_LINT => settings.auto_lint,
                            INLINE => settings.inline_messages,
                            _ => false,
                        },
                        shortcut: ptr::null_mut(),
                    }
                })
                .collect();
            // Notepad++ keeps the array for the life of the process.
            s.funcs
                .set(Box::leak(items.into_boxed_slice()).as_mut_ptr());
        }
        s.funcs.get()
    });
    // SAFETY: the caller passes a valid pointer.
    unsafe { *count = MENU.len() as i32 };
    funcs
}

/// # Safety
///
/// `notification` must point to a valid `SCNotification`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn beNotified(notification: *mut ScNotification) {
    // SAFETY: the caller passes a valid notification.
    let Some(n) = (unsafe { notification.as_ref() }) else {
        return;
    };
    guard(|| on_notification(n));
}

#[unsafe(no_mangle)]
pub extern "C" fn messageProc(_msg: u32, _wparam: usize, _lparam: isize) -> isize {
    1
}

fn on_notification(n: &ScNotification) {
    let data = with(|s| s.npp.get());
    let from_npp = n.nmhdr.hwnd_from == data.npp;
    match n.nmhdr.code {
        NPPN_READY if from_npp => {
            for sci in [data.scintilla_main, data.scintilla_second] {
                if send(sci, SCI_GETMOUSEDWELLTIME, 0, 0) == SC_TIME_FOREVER {
                    send(sci, SCI_SETMOUSEDWELLTIME, DWELL_MS as usize, 0);
                }
            }
            lint_current(Trigger::Auto);
        }
        NPPN_BUFFERACTIVATED | NPPN_LANGCHANGED if from_npp => lint_current(Trigger::Auto),
        NPPN_FILEBEFORESAVE if from_npp => {
            let id = n.nmhdr.id_from;
            if with(|s| s.settings.get().format_on_save) && id == current_buffer() && is_vhdl(id) {
                format_current(Trigger::Auto);
            }
        }
        NPPN_FILESAVED if from_npp => {
            let id = n.nmhdr.id_from;
            if buffer_path(id).is_some_and(|p| is_project_file(&p)) {
                with(|s| {
                    if let Ok(mut engine) = s.engine.try_borrow_mut() {
                        engine.reload();
                    }
                });
            }
            if id == current_buffer() {
                lint_current(Trigger::Auto);
            }
        }
        NPPN_FILEBEFORECLOSE if from_npp => {
            let id = n.nmhdr.id_from;
            if let Some(path) = buffer_path(id) {
                with(|s| {
                    if let Ok(mut engine) = s.engine.try_borrow_mut() {
                        engine.forget(&path);
                    }
                });
            }
            with(|s| {
                s.problems.borrow_mut().remove(&id);
                s.annotated.borrow_mut().remove(&id);
            });
        }
        NPPN_SHUTDOWN if from_npp => cancel_timer(),
        SCN_MODIFIED if n.nmhdr.hwnd_from == current_scintilla().0 => {
            if n.modification_type & (SC_MOD_INSERTTEXT | SC_MOD_DELETETEXT) != 0
                && with(|s| s.settings.get().auto_lint)
            {
                schedule_lint();
            }
        }
        SCN_DWELLSTART if n.nmhdr.hwnd_from == current_scintilla().0 => {
            if let Ok(position) = usize::try_from(n.position) {
                show_problem_at(n.nmhdr.hwnd_from, position);
            }
        }
        SCN_DWELLEND if with(|s| s.calltip.replace(false)) => {
            send(n.nmhdr.hwnd_from, SCI_CALLTIPCANCEL, 0, 0);
        }
        _ => {}
    }
}

// Commands.

extern "C" fn cmd_format() {
    guard(|| format_current(Trigger::Command));
}

extern "C" fn cmd_lint() {
    guard(|| lint_current(Trigger::Command));
}

extern "C" fn cmd_next() {
    guard(|| step_problem(true));
}

extern "C" fn cmd_previous() {
    guard(|| step_problem(false));
}

extern "C" fn cmd_clear() {
    guard(|| {
        let (sci, _) = current_scintilla();
        let id = current_buffer();
        clear_indicators(sci);
        with(|s| {
            s.problems.borrow_mut().remove(&id);
            if s.annotated.borrow_mut().remove(&id) {
                send(sci, SCI_ANNOTATIONCLEARALL, 0, 0);
            }
        });
    });
}

extern "C" fn cmd_toggle_format_on_save() {
    guard(|| toggle(FORMAT_ON_SAVE, |s| &mut s.format_on_save));
}

extern "C" fn cmd_toggle_auto_lint() {
    guard(|| {
        if toggle(AUTO_LINT, |s| &mut s.auto_lint) {
            lint_current(Trigger::Auto);
        }
    });
}

extern "C" fn cmd_toggle_inline() {
    guard(|| {
        toggle(INLINE, |s| &mut s.inline_messages);
        lint_current(Trigger::Refresh);
    });
}

extern "C" fn cmd_reload() {
    guard(|| {
        with(|s| {
            if let Ok(mut engine) = s.engine.try_borrow_mut() {
                engine.reload();
            }
        });
        lint_current(Trigger::Refresh);
    });
}

extern "C" fn cmd_about() {
    guard(|| {
        message_box(
            npp(),
            &format!(
                "vfl {} for Notepad++\n\
                 VHDL formatter and linter.\n\n\
                 Settings come from vfl.toml, found like the vfl command does.\n\n\
                 https://github.com/thewenneur/vhdl-fmt-lint",
                env!("CARGO_PKG_VERSION")
            ),
            MB_ICONINFORMATION,
        )
    });
}

/// Flips a setting and its menu check mark. Returns the new value.
fn toggle(item: usize, field: impl Fn(&mut Settings) -> &mut bool) -> bool {
    let mut settings = with(|s| s.settings.get());
    let value = field(&mut settings);
    *value = !*value;
    let value = *value;
    with(|s| s.settings.set(settings));
    send(npp(), NPPM_SETMENUITEMCHECK, cmd_id(item), value as isize);
    save_settings();
    value
}

fn cmd_id(item: usize) -> usize {
    let funcs = with(|s| s.funcs.get());
    if funcs.is_null() || item >= MENU.len() {
        return 0;
    }
    // SAFETY: `funcs` holds `MENU.len()` items and is never freed.
    unsafe { (*funcs.add(item)).cmd_id as usize }
}

// Buffers.

/// The current view's Scintilla window and its index.
fn current_scintilla() -> (Hwnd, usize) {
    let data = with(|s| s.npp.get());
    let mut which: i32 = 0;
    send(
        data.npp,
        NPPM_GETCURRENTSCINTILLA,
        0,
        &mut which as *mut i32 as isize,
    );
    if which == 1 {
        (data.scintilla_second, 1)
    } else {
        (data.scintilla_main, 0)
    }
}

fn current_buffer() -> usize {
    send(npp(), NPPM_GETCURRENTBUFFERID, 0, 0) as usize
}

/// The buffer's path; unsaved buffers have a bare name like `new 1`.
fn buffer_path(id: usize) -> Option<PathBuf> {
    let npp = npp();
    let len = send(npp, NPPM_GETFULLPATHFROMBUFFERID, id, 0);
    let len = usize::try_from(len).ok()?;
    let mut buf = vec![0u16; len + 1];
    send(
        npp,
        NPPM_GETFULLPATHFROMBUFFERID,
        id,
        buf.as_mut_ptr() as isize,
    );
    buf.truncate(len);
    Some(PathBuf::from(OsString::from_wide(&buf)))
}

fn is_vhdl(id: usize) -> bool {
    send(npp(), NPPM_GETBUFFERLANGTYPE, id, 0) == L_VHDL
        || buffer_path(id).is_some_and(|p| is_vhdl_path(&p))
}

fn read_buffer(sci: Hwnd) -> BufferText {
    let len = send(sci, SCI_GETLENGTH, 0, 0).max(0) as usize;
    let data = send(sci, SCI_GETCHARACTERPOINTER, 0, 0) as *const u8;
    let bytes = if data.is_null() || len == 0 {
        &[][..]
    } else {
        // SAFETY: Scintilla returns its contiguous buffer of `len` bytes,
        // valid until the document changes; it is copied right away.
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    let utf8 = send(sci, SCI_GETCODEPAGE, 0, 0) == SC_CP_UTF8;
    BufferText::new(bytes, utf8)
}

/// NUL-terminated, for Scintilla.
fn c_string(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.retain(|&b| b != 0);
    bytes.push(0);
    bytes
}

// Formatting.

#[derive(Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// Typing, saving, switching buffers: only VHDL buffers, silently.
    Auto,
    /// A menu command: any buffer, and say when there is nothing to do.
    Command,
    /// Re-show problems after a settings change, silently.
    Refresh,
}

fn format_current(trigger: Trigger) {
    let (sci, _) = current_scintilla();
    let id = current_buffer();
    let path = buffer_path(id);
    let buffer = read_buffer(sci);
    let result = with(|s| match s.engine.try_borrow_mut() {
        Ok(mut engine) => Some(engine.format(path.as_deref(), &buffer.text)),
        Err(_) => None,
    });
    match result {
        Some(Ok(Some(formatted))) => replace_text(sci, &buffer, &formatted),
        Some(Err(failure)) if trigger == Trigger::Command => {
            if let Some(offset) = failure.offset {
                go_to(sci, buffer.position(offset));
            }
            message_box(
                npp(),
                &format!("Not formatted: {}", failure.message),
                MB_ICONWARNING,
            );
        }
        _ => {}
    }
}

/// Replaces the buffer's text with `new`, as one undo step, touching only
/// the part that changed and keeping the caret and scroll position.
fn replace_text(sci: Hwnd, buffer: &BufferText, new: &str) {
    let old = buffer.encode(&buffer.text);
    let new = buffer.encode(new);
    let (start, end, with) = minimal_edit(&old, &new);
    let caret = send(sci, SCI_GETCURRENTPOS, 0, 0).max(0) as usize;
    let caret_line = send(sci, SCI_LINEFROMPOSITION, caret, 0);
    let caret_column = send(sci, SCI_GETCOLUMN, caret, 0);
    let first_visible = send(sci, SCI_GETFIRSTVISIBLELINE, 0, 0);

    send(sci, SCI_BEGINUNDOACTION, 0, 0);
    send(sci, SCI_SETTARGETRANGE, start, end as isize);
    send(sci, SCI_REPLACETARGET, with.len(), with.as_ptr() as isize);
    send(sci, SCI_ENDUNDOACTION, 0, 0);

    let caret = if caret <= start {
        caret
    } else if caret >= end {
        caret + with.len() - (end - start)
    } else {
        send(sci, SCI_FINDCOLUMN, caret_line as usize, caret_column).max(0) as usize
    };
    send(sci, SCI_GOTOPOS, caret, 0);
    send(sci, SCI_SETFIRSTVISIBLELINE, first_visible as usize, 0);
}

// Linting.

fn lint_current(trigger: Trigger) {
    let (sci, view) = current_scintilla();
    let id = current_buffer();
    let auto = with(|s| s.settings.get().auto_lint);
    let wanted = match trigger {
        Trigger::Auto => auto && is_vhdl(id),
        Trigger::Command => true,
        Trigger::Refresh => is_vhdl(id) || with(|s| s.problems.borrow().contains_key(&id)),
    };
    if !wanted {
        return;
    }
    let path = buffer_path(id);
    let buffer = read_buffer(sci);
    let Some(output) = with(|s| {
        s.engine
            .try_borrow_mut()
            .ok()
            .map(|mut engine| engine.lint(path.as_deref(), &buffer.text))
    }) else {
        return;
    };
    let mut problems: Vec<Problem> = output
        .findings
        .iter()
        .map(|f| Problem {
            start: buffer.position(f.start),
            end: buffer.position(f.end),
            severity: f.severity,
            text: buffer.encode(&format!(
                "{}: {} [{}]",
                f.severity.label(),
                f.message,
                f.rule
            )),
        })
        .collect();
    problems.sort_by_key(|p| (p.start, p.end));
    show_problems(sci, view, id, &problems);
    let count = problems.len();
    with(|s| s.problems.borrow_mut().insert(id, problems));

    if trigger == Trigger::Command {
        if !output.config_errors.is_empty() {
            message_box(
                npp(),
                &format!(
                    "Problems in the configuration:\n\n{}",
                    output.config_errors.join("\n")
                ),
                MB_ICONWARNING,
            );
        } else if count == 0 {
            message_box(npp(), "No problems found.", MB_ICONINFORMATION);
        } else {
            step_problem(true);
        }
    }
}

fn schedule_lint() {
    cancel_timer();
    // SAFETY: a thread timer with a plain callback.
    let id = unsafe { SetTimer(ptr::null_mut(), 0, LINT_DELAY_MS, Some(on_timer)) };
    with(|s| s.timer.set(id));
}

fn cancel_timer() {
    let id = with(|s| s.timer.replace(0));
    if id != 0 {
        // SAFETY: `id` is a thread timer this plugin created.
        unsafe { KillTimer(ptr::null_mut(), id) };
    }
}

unsafe extern "system" fn on_timer(_hwnd: Hwnd, _msg: u32, _id: usize, _time: u32) {
    guard(|| {
        cancel_timer();
        lint_current(Trigger::Auto);
    });
}

fn indicators() -> [usize; 3] {
    if let Some(indicators) = with(|s| s.indicators.get()) {
        return indicators;
    }
    let mut start: i32 = 0;
    let allocated = send(
        npp(),
        NPPM_ALLOCATEINDICATOR,
        3,
        &mut start as *mut i32 as isize,
    );
    let indicators = match usize::try_from(start) {
        Ok(start) if allocated != 0 && start > 0 => [start, start + 1, start + 2],
        _ => FALLBACK_INDICATORS,
    };
    with(|s| s.indicators.set(Some(indicators)));
    indicators
}

fn clear_indicators(sci: Hwnd) {
    let len = send(sci, SCI_GETLENGTH, 0, 0);
    for indicator in indicators() {
        send(sci, SCI_SETINDICATORCURRENT, indicator, 0);
        send(sci, SCI_INDICATORCLEARRANGE, 0, len);
    }
}

fn show_problems(sci: Hwnd, view: usize, id: usize, problems: &[Problem]) {
    let len = send(sci, SCI_GETLENGTH, 0, 0).max(0) as usize;
    let indicators = indicators();
    for severity in [Severity::Info, Severity::Warning, Severity::Error] {
        let indicator = indicators[severity_index(severity)];
        send(sci, SCI_INDICSETSTYLE, indicator, INDIC_SQUIGGLEPIXMAP);
        send(sci, SCI_INDICSETFORE, indicator, color(severity));
    }
    clear_indicators(sci);
    for problem in problems {
        // Give empty ranges (a missing `;`) one character to underline.
        let (start, end) = if problem.end > problem.start {
            (problem.start, problem.end.min(len))
        } else if problem.start < len {
            (problem.start, problem.start + 1)
        } else {
            (len.saturating_sub(1), len)
        };
        if start >= end {
            continue;
        }
        send(
            sci,
            SCI_SETINDICATORCURRENT,
            indicators[severity_index(problem.severity)],
            0,
        );
        send(sci, SCI_INDICATORFILLRANGE, start, (end - start) as isize);
    }

    let inline = with(|s| s.settings.get().inline_messages);
    let was_annotated = with(|s| s.annotated.borrow().contains(&id));
    if was_annotated || inline {
        send(sci, SCI_ANNOTATIONCLEARALL, 0, 0);
    }
    if !inline || problems.is_empty() {
        with(|s| s.annotated.borrow_mut().remove(&id));
        return;
    }
    let base = annotation_styles(sci, view);
    send(sci, SCI_ANNOTATIONSETSTYLEOFFSET, base as usize, 0);
    send(sci, SCI_ANNOTATIONSETVISIBLE, ANNOTATION_STANDARD, 0);
    let mut lines: BTreeMap<usize, (Severity, Vec<u8>)> = BTreeMap::new();
    for problem in problems {
        let line = send(sci, SCI_LINEFROMPOSITION, problem.start, 0).max(0) as usize;
        let (severity, text) = lines.entry(line).or_insert((problem.severity, Vec::new()));
        *severity = (*severity).max(problem.severity);
        if !text.is_empty() {
            text.push(b'\n');
        }
        text.extend_from_slice(&problem.text);
    }
    for (line, (severity, text)) in lines {
        let text = c_string(text);
        send(sci, SCI_ANNOTATIONSETTEXT, line, text.as_ptr() as isize);
        send(
            sci,
            SCI_ANNOTATIONSETSTYLE,
            line,
            severity_index(severity) as isize,
        );
    }
    with(|s| s.annotated.borrow_mut().insert(id));
}

/// Our three annotation styles in `view`, set to the editor's default
/// font and background (both follow the theme) in each severity's color.
fn annotation_styles(sci: Hwnd, view: usize) -> isize {
    let mut styles = with(|s| s.styles.get());
    let base =
        *styles[view.min(1)].get_or_insert_with(|| send(sci, SCI_ALLOCATEEXTENDEDSTYLES, 3, 0));
    with(|s| s.styles.set(styles));

    let mut font = [0u8; 128];
    send(
        sci,
        SCI_STYLEGETFONT,
        STYLE_DEFAULT,
        font.as_mut_ptr() as isize,
    );
    let size = send(sci, SCI_STYLEGETSIZE, STYLE_DEFAULT, 0);
    let back = send(sci, SCI_STYLEGETBACK, STYLE_DEFAULT, 0);
    for severity in [Severity::Info, Severity::Warning, Severity::Error] {
        let style = (base + severity_index(severity) as isize) as usize;
        if font[0] != 0 {
            send(sci, SCI_STYLESETFONT, style, font.as_ptr() as isize);
        }
        send(sci, SCI_STYLESETSIZE, style, size);
        send(sci, SCI_STYLESETFORE, style, color(severity));
        send(sci, SCI_STYLESETBACK, style, back);
        send(sci, SCI_STYLESETITALIC, style, 1);
    }
    base
}

/// Moves to the next (or previous) problem, wrapping around, and shows
/// its message.
fn step_problem(forward: bool) {
    let (sci, _) = current_scintilla();
    let id = current_buffer();
    let caret = send(sci, SCI_GETCURRENTPOS, 0, 0).max(0) as usize;
    let target = with(|s| {
        let problems = s.problems.borrow();
        let list = problems.get(&id)?;
        let found = if forward {
            list.iter().find(|p| p.start > caret).or(list.first())
        } else {
            list.iter().rev().find(|p| p.start < caret).or(list.last())
        };
        found.map(|p| (p.start, c_string(p.text.clone())))
    });
    match target {
        Some((position, text)) => {
            go_to(sci, position);
            send(sci, SCI_CALLTIPSHOW, position, text.as_ptr() as isize);
            with(|s| s.calltip.set(true));
        }
        None => message_box(npp(), "No problems found.", MB_ICONINFORMATION),
    }
}

fn go_to(sci: Hwnd, position: usize) {
    let line = send(sci, SCI_LINEFROMPOSITION, position, 0).max(0) as usize;
    send(sci, SCI_ENSUREVISIBLE, line, 0);
    send(sci, SCI_GOTOPOS, position, 0);
}

/// Shows the message of the problem under the mouse.
fn show_problem_at(sci: Hwnd, position: usize) {
    let id = current_buffer();
    let text = with(|s| {
        let problems = s.problems.borrow();
        let texts: Vec<&[u8]> = problems
            .get(&id)?
            .iter()
            .filter(|p| {
                p.start <= position
                    && (position < p.end || (p.start == p.end && position == p.start))
            })
            .map(|p| p.text.as_slice())
            .collect();
        (!texts.is_empty()).then(|| c_string(texts.join(&b'\n')))
    });
    if let Some(text) = text {
        send(sci, SCI_CALLTIPSHOW, position, text.as_ptr() as isize);
        with(|s| s.calltip.set(true));
    }
}

// Settings, kept in `plugins\Config\vfl_npp.ini`.

fn settings_path() -> Option<PathBuf> {
    let mut buf = vec![0u16; 1024];
    let ok = send(
        npp(),
        NPPM_GETPLUGINSCONFIGDIR,
        buf.len(),
        buf.as_mut_ptr() as isize,
    );
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    if ok == 0 || len == 0 {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(&buf[..len])).join("vfl_npp.ini"))
}

fn load_settings() {
    let mut settings = Settings::default();
    if let Some(text) = settings_path().and_then(|p| std::fs::read_to_string(p).ok()) {
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim() == "1";
            match key.trim() {
                "format_on_save" => settings.format_on_save = value,
                "lint_automatically" => settings.auto_lint = value,
                "messages_under_lines" => settings.inline_messages = value,
                _ => {}
            }
        }
    }
    with(|s| s.settings.set(settings));
}

fn save_settings() {
    let Some(path) = settings_path() else {
        return;
    };
    let s = with(|s| s.settings.get());
    let text = format!(
        "format_on_save={}\nlint_automatically={}\nmessages_under_lines={}\n",
        s.format_on_save as u8, s.auto_lint as u8, s.inline_messages as u8
    );
    // A settings file that cannot be written only loses the settings.
    let _ = std::fs::write(path, text);
}
