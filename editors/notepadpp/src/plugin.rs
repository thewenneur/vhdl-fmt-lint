//! The Notepad++ side: the exported plugin functions, the menu, showing
//! problems, and code assistance. Navigation, rename and fixes are in
//! [`crate::tools`].
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

use vfl_lsp::editor::CompletionKind;

use crate::engine::{
    BufferText, Engine, FormatFailure, Severity, completes_after_dot, completion_list,
    is_library_copy, is_project_file, is_vhdl_path, minimal_edit, plain_text, signature_tip,
};
use crate::tools;
use crate::win::*;

/// Menu entries, in order. `None` is a separator.
const MENU: &[(&str, Option<extern "C" fn()>)] = &[
    ("Format document", Some(cmd_format)),
    ("Format selection", Some(cmd_format_selection)),
    ("Lint document", Some(cmd_lint)),
    ("Next problem", Some(cmd_next)),
    ("Previous problem", Some(cmd_previous)),
    ("Fix problem at caret", Some(cmd_quick_fix)),
    ("Fix all problems", Some(cmd_fix_all)),
    ("Clear problems", Some(cmd_clear)),
    ("", None),
    ("Go to definition", Some(cmd_definition)),
    ("Go back", Some(cmd_back)),
    ("Find references", Some(cmd_references)),
    ("Rename symbol", Some(cmd_rename)),
    ("Go to symbol in file", Some(cmd_outline)),
    ("Go to symbol in project", Some(cmd_project_symbols)),
    ("", None),
    ("Show type", Some(cmd_show_type)),
    ("Show parameters", Some(cmd_show_parameters)),
    ("Complete word", Some(cmd_complete)),
    ("", None),
    ("Format on save", Some(cmd_toggle_format_on_save)),
    ("Lint automatically", Some(cmd_toggle_auto_lint)),
    ("Show messages under lines", Some(cmd_toggle_inline)),
    ("Code assistance", Some(cmd_toggle_assist)),
    ("Highlight uses", Some(cmd_toggle_highlight)),
    ("", None),
    ("Reload project", Some(cmd_reload)),
    ("About", Some(cmd_about)),
];

/// The position of a menu entry.
const fn item(name: &str) -> usize {
    let mut i = 0;
    while i < MENU.len() {
        if eq(MENU[i].0.as_bytes(), name.as_bytes()) {
            return i;
        }
        i += 1;
    }
    panic!("no such menu entry");
}

const fn eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

const FORMAT_ON_SAVE: usize = item("Format on save");
const AUTO_LINT: usize = item("Lint automatically");
const INLINE: usize = item("Show messages under lines");
const ASSIST: usize = item("Code assistance");
const HIGHLIGHT: usize = item("Highlight uses");

/// How long typing must pause before the buffer is linted again.
const LINT_DELAY_MS: u32 = 400;
const DWELL_MS: isize = 500;
/// How often, at most, the projects are checked for changes on disk.
const RESCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
/// Indicators to use when Notepad++ is too old to hand them out.
const FALLBACK_INDICATORS: [usize; INDICATORS] = [9, 10, 11, 12, 13, 14];
/// Problems by severity, uses of a name (read or other, written), and
/// unused names.
const INDICATORS: usize = 6;
pub(crate) const USE_INDICATOR: usize = 3;
pub(crate) const WRITE_INDICATOR: usize = 4;
const UNUSED_INDICATOR: usize = 5;

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
pub(crate) struct Settings {
    format_on_save: bool,
    auto_lint: bool,
    inline_messages: bool,
    /// Types on hover, parameter hints and completion as you type.
    assist: bool,
    /// Highlight the uses of the name at the caret.
    pub(crate) highlight: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            format_on_save: false,
            auto_lint: true,
            inline_messages: true,
            assist: true,
            highlight: true,
        }
    }
}

/// What the call tip on screen is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tip {
    None,
    /// Problems or a type under the mouse, or from a command: closes when
    /// the mouse moves.
    Info,
    /// Parameter hints: follow the caret while it stays in the call.
    Parameters,
}

/// A problem placed in a buffer, at Scintilla positions.
struct Problem {
    start: usize,
    end: usize,
    severity: Severity,
    /// `error: message [rule]`, in the buffer's encoding.
    text: Vec<u8>,
    /// An unused declaration, shown greyed out.
    unused: bool,
}

pub(crate) struct State {
    npp: Cell<NppData>,
    pub(crate) engine: RefCell<Engine>,
    /// The problems of each linted buffer, by buffer id, sorted.
    problems: RefCell<HashMap<usize, Vec<Problem>>>,
    /// Buffers showing our messages under lines.
    annotated: RefCell<HashSet<usize>>,
    pub(crate) settings: Cell<Settings>,
    indicators: Cell<Option<[usize; INDICATORS]>>,
    /// The first of our three annotation styles, per view.
    styles: Cell<[Option<isize>; 2]>,
    funcs: Cell<*mut FuncItem>,
    timer: Cell<usize>,
    pub(crate) tip: Cell<Tip>,
    /// The caret position the parameter hints were shown for.
    tip_caret: Cell<usize>,
    /// The details of the completions in the list on screen, by label,
    /// while it is ours.
    details: RefCell<Option<HashMap<String, String>>>,
    /// The status bar text a completion's details replaced.
    status: RefCell<Option<String>>,
    /// Configuration errors already shown.
    shown_errors: RefCell<HashSet<String>>,
    last_rescan: Cell<Option<std::time::Instant>>,
    pub(crate) tools: tools::ToolState,
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
        tip: Cell::new(Tip::None),
        tip_caret: Cell::new(0),
        details: RefCell::new(None),
        status: RefCell::new(None),
        shown_errors: RefCell::default(),
        last_rescan: Cell::new(None),
        tools: tools::ToolState::default(),
    };
}

pub(crate) fn with<T>(f: impl FnOnce(&State) -> T) -> T {
    STATE.with(f)
}

pub(crate) fn npp() -> Hwnd {
    with(|s| s.npp.get().npp)
}

/// Runs `f`, turning a panic into a message instead of taking Notepad++
/// down with it.
pub(crate) fn guard<T: Default>(f: impl FnOnce() -> T) -> T {
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
                            ASSIST => settings.assist,
                            HIGHLIGHT => settings.highlight,
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
            rescan();
            lint_current(Trigger::Auto);
        }
        NPPN_BUFFERACTIVATED | NPPN_LANGCHANGED if from_npp => {
            rescan();
            lint_current(Trigger::Auto);
            tools::schedule_highlight();
        }
        // Coming back to Notepad++, maybe after changing files elsewhere.
        SCN_FOCUSIN if n.nmhdr.hwnd_from == current_scintilla().0 => {
            if rescan() {
                lint_current(Trigger::Auto);
            }
        }
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
        NPPN_SHUTDOWN if from_npp => {
            cancel_timer();
            tools::cancel_highlight();
        }
        SCN_MODIFIED if n.nmhdr.hwnd_from == current_scintilla().0 => {
            if n.modification_type & (SC_MOD_INSERTTEXT | SC_MOD_DELETETEXT) != 0
                && with(|s| s.settings.get().auto_lint)
            {
                schedule_lint();
            }
        }
        SCN_CHARADDED if n.nmhdr.hwnd_from == current_scintilla().0 => on_char(n.ch),
        SCN_UPDATEUI if n.nmhdr.hwnd_from == current_scintilla().0 => {
            tools::caret_moved(n.nmhdr.hwnd_from);
            if with(|s| s.tip.get()) == Tip::Parameters {
                let sci = n.nmhdr.hwnd_from;
                if send(sci, SCI_CALLTIPACTIVE, 0, 0) == 0 {
                    with(|s| s.tip.set(Tip::None));
                } else if caret(sci) != with(|s| s.tip_caret.get()) {
                    show_parameters(Trigger::Auto);
                }
            }
        }
        SCN_USERLISTSELECTION if n.nmhdr.hwnd_from == current_scintilla().0 => {
            tools::list_selected(n);
        }
        SCN_AUTOCSELECTIONCHANGE if n.nmhdr.hwnd_from == current_scintilla().0 => {
            show_detail(n);
        }
        SCN_AUTOCCOMPLETED | SCN_AUTOCCANCELLED => {
            if with(|s| s.details.borrow_mut().take()).is_some() {
                restore_status();
            }
        }
        SCN_DWELLSTART if n.nmhdr.hwnd_from == current_scintilla().0 => {
            if let Ok(position) = usize::try_from(n.position) {
                show_info_at(n.nmhdr.hwnd_from, position);
            }
        }
        SCN_DWELLEND if with(|s| s.tip.get()) == Tip::Info => {
            with(|s| s.tip.set(Tip::None));
            send(n.nmhdr.hwnd_from, SCI_CALLTIPCANCEL, 0, 0);
        }
        _ => {}
    }
}

// Commands.

extern "C" fn cmd_format() {
    guard(|| format_current(Trigger::Command));
}

extern "C" fn cmd_format_selection() {
    guard(format_selection);
}

extern "C" fn cmd_lint() {
    guard(|| {
        rescan();
        lint_current(Trigger::Command);
    });
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

extern "C" fn cmd_quick_fix() {
    guard(tools::quick_fix);
}

extern "C" fn cmd_fix_all() {
    guard(|| tools::fix_all(Trigger::Command));
}

extern "C" fn cmd_definition() {
    guard(tools::go_to_definition);
}

extern "C" fn cmd_back() {
    guard(tools::go_back);
}

extern "C" fn cmd_references() {
    guard(tools::find_references);
}

extern "C" fn cmd_rename() {
    guard(tools::rename);
}

extern "C" fn cmd_outline() {
    guard(tools::outline);
}

extern "C" fn cmd_project_symbols() {
    guard(tools::project_symbols);
}

extern "C" fn cmd_toggle_highlight() {
    guard(|| {
        toggle(HIGHLIGHT, |s| &mut s.highlight);
        tools::schedule_highlight();
    });
}

extern "C" fn cmd_show_type() {
    guard(|| {
        let (sci, _) = current_scintilla();
        let position = caret(sci);
        if !show_info_at(sci, position) {
            message_box(npp(), "Nothing to show here.", MB_ICONINFORMATION);
        }
    });
}

extern "C" fn cmd_show_parameters() {
    guard(|| show_parameters(Trigger::Command));
}

extern "C" fn cmd_complete() {
    guard(|| complete(Trigger::Command));
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

extern "C" fn cmd_toggle_assist() {
    guard(|| {
        toggle(ASSIST, |s| &mut s.assist);
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
                 VHDL formatter, linter, navigation and code assistance.\n\n\
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
pub(crate) fn current_scintilla() -> (Hwnd, usize) {
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

pub(crate) fn caret(sci: Hwnd) -> usize {
    send(sci, SCI_GETCURRENTPOS, 0, 0).max(0) as usize
}

pub(crate) fn current_buffer() -> usize {
    send(npp(), NPPM_GETCURRENTBUFFERID, 0, 0) as usize
}

/// The buffer's path; unsaved buffers have a bare name like `new 1`.
pub(crate) fn buffer_path(id: usize) -> Option<PathBuf> {
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

pub(crate) fn is_vhdl(id: usize) -> bool {
    send(npp(), NPPM_GETBUFFERLANGTYPE, id, 0) == L_VHDL
        || buffer_path(id).is_some_and(|p| is_vhdl_path(&p))
}

pub(crate) fn read_buffer(sci: Hwnd) -> BufferText {
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
pub(crate) fn c_string(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.retain(|&b| b != 0);
    bytes.push(0);
    bytes
}

// Formatting.

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trigger {
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
    show_format_result(sci, &buffer, result, trigger);
}

/// Formats the selected lines, or the caret's line, and the statements and
/// aligned lines they are part of.
fn format_selection() {
    let (sci, _) = current_scintilla();
    let path = buffer_path(current_buffer());
    let buffer = read_buffer(sci);
    let start = buffer.offset(send(sci, SCI_GETSELECTIONSTART, 0, 0).max(0) as usize);
    let end = buffer.offset(send(sci, SCI_GETSELECTIONEND, 0, 0).max(0) as usize);
    let result = with(|s| match s.engine.try_borrow_mut() {
        Ok(mut engine) => Some(engine.format_range(path.as_deref(), &buffer.text, start..end)),
        Err(_) => None,
    });
    show_format_result(sci, &buffer, result, Trigger::Command);
}

fn show_format_result(
    sci: Hwnd,
    buffer: &BufferText,
    result: Option<Result<Option<String>, FormatFailure>>,
    trigger: Trigger,
) {
    match result {
        Some(Ok(Some(formatted))) => replace_text(sci, buffer, &formatted),
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
pub(crate) fn replace_text(sci: Hwnd, buffer: &BufferText, new: &str) {
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

pub(crate) fn lint_current(trigger: Trigger) {
    let (sci, view) = current_scintilla();
    let id = current_buffer();
    let auto = with(|s| s.settings.get().auto_lint);
    let path = buffer_path(id);
    // Library sources opened by go to definition are not the user's.
    let library = path.as_deref().is_some_and(is_library_copy);
    let wanted = match trigger {
        Trigger::Auto => auto && is_vhdl(id) && !library,
        Trigger::Command => true,
        Trigger::Refresh => {
            (is_vhdl(id) && !library) || with(|s| s.problems.borrow().contains_key(&id))
        }
    };
    if !wanted {
        return;
    }
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
            unused: f.rule.starts_with("sema/unused-"),
        })
        .collect();
    problems.sort_by_key(|p| (p.start, p.end));
    show_problems(sci, view, id, &problems);
    let count = problems.len();
    with(|s| s.problems.borrow_mut().insert(id, problems));

    if trigger != Trigger::Command {
        // Show each configuration error once, like the language server.
        let new: Vec<String> = with(|s| {
            let mut shown = s.shown_errors.borrow_mut();
            output
                .config_errors
                .iter()
                .filter(|e| shown.insert((*e).clone()))
                .cloned()
                .collect()
        });
        if !new.is_empty() {
            message_box(
                npp(),
                &format!("Problems in the configuration:\n\n{}", new.join("\n")),
                MB_ICONWARNING,
            );
        }
    }
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

pub(crate) fn indicators() -> [usize; INDICATORS] {
    if let Some(indicators) = with(|s| s.indicators.get()) {
        return indicators;
    }
    let mut start: i32 = 0;
    let allocated = send(
        npp(),
        NPPM_ALLOCATEINDICATOR,
        INDICATORS,
        &mut start as *mut i32 as isize,
    );
    let indicators = match usize::try_from(start) {
        Ok(start) if allocated != 0 && start > 0 => std::array::from_fn(|i| start + i),
        _ => FALLBACK_INDICATORS,
    };
    with(|s| s.indicators.set(Some(indicators)));
    indicators
}

/// Removes the problem underlines and greyed-out names.
fn clear_indicators(sci: Hwnd) {
    let len = send(sci, SCI_GETLENGTH, 0, 0);
    let indicators = indicators();
    for i in [0, 1, 2, UNUSED_INDICATOR] {
        send(sci, SCI_SETINDICATORCURRENT, indicators[i], 0);
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
    send(
        sci,
        SCI_INDICSETSTYLE,
        indicators[UNUSED_INDICATOR],
        INDIC_TEXTFORE,
    );
    send(
        sci,
        SCI_INDICSETFORE,
        indicators[UNUSED_INDICATOR],
        0x909090,
    );
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
        if problem.unused {
            send(
                sci,
                SCI_SETINDICATORCURRENT,
                indicators[UNUSED_INDICATOR],
                0,
            );
            send(sci, SCI_INDICATORFILLRANGE, start, (end - start) as isize);
        }
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
            with(|s| s.tip.set(Tip::Info));
        }
        None => message_box(npp(), "No problems found.", MB_ICONINFORMATION),
    }
}

pub(crate) fn go_to(sci: Hwnd, position: usize) {
    let line = send(sci, SCI_LINEFROMPOSITION, position, 0).max(0) as usize;
    send(sci, SCI_ENSUREVISIBLE, line, 0);
    send(sci, SCI_GOTOPOS, position, 0);
}

/// Shows the messages of the problems at `position` and, with code
/// assistance on, the type of what is named there. Returns whether there
/// was anything to show.
fn show_info_at(sci: Hwnd, position: usize) -> bool {
    let id = current_buffer();
    let mut parts: Vec<Vec<u8>> = with(|s| {
        let problems = s.problems.borrow();
        problems
            .get(&id)
            .into_iter()
            .flatten()
            .filter(|p| {
                p.start <= position
                    && (position < p.end || (p.start == p.end && position == p.start))
            })
            .map(|p| p.text.clone())
            .collect()
    });
    if with(|s| s.settings.get().assist) && is_vhdl(id) {
        let path = buffer_path(id);
        let buffer = read_buffer(sci);
        let offset = buffer.offset(position);
        let hover = with(|s| {
            s.engine
                .try_borrow_mut()
                .ok()
                .and_then(|mut engine| engine.hover(path.as_deref(), &buffer.text, offset))
        });
        if let Some(hover) = hover {
            parts.push(buffer.encode(&plain_text(&hover.markdown)));
        }
    }
    if parts.is_empty() {
        return false;
    }
    let text = c_string(parts.join(&b"\n\n"[..]));
    send(sci, SCI_CALLTIPSHOW, position, text.as_ptr() as isize);
    with(|s| s.tip.set(Tip::Info));
    true
}

// Code assistance.

fn on_char(ch: i32) {
    if !with(|s| s.settings.get().assist) {
        return;
    }
    let Some(ch) = u32::try_from(ch).ok().and_then(char::from_u32) else {
        return;
    };
    match ch {
        '.' => complete(Trigger::Auto),
        '(' | ',' => show_parameters(Trigger::Auto),
        // Typing in a call moves between parameters (`name =>`) and out
        // of it (`)`).
        _ if with(|s| s.tip.get()) == Tip::Parameters => show_parameters(Trigger::Auto),
        _ => {}
    }
}

/// Shows the parameters of the call or port map around the caret, the
/// one being typed highlighted. Closes the hints when there is none.
fn show_parameters(trigger: Trigger) {
    let (sci, _) = current_scintilla();
    let id = current_buffer();
    if trigger == Trigger::Auto && !is_vhdl(id) {
        return;
    }
    let path = buffer_path(id);
    let buffer = read_buffer(sci);
    let position = caret(sci);
    let offset = buffer.offset(position);
    let help = with(|s| {
        s.engine
            .try_borrow_mut()
            .ok()
            .and_then(|mut engine| engine.signature_help(path.as_deref(), &buffer.text, offset))
    });
    let Some(help) = help else {
        if with(|s| s.tip.get()) == Tip::Parameters {
            send(sci, SCI_CALLTIPCANCEL, 0, 0);
            with(|s| s.tip.set(Tip::None));
        }
        if trigger == Trigger::Command {
            message_box(npp(), "The caret is not in a call.", MB_ICONINFORMATION);
        }
        return;
    };
    let (tip, highlight) = signature_tip(&help);
    let text = c_string(buffer.encode(&tip));
    send(sci, SCI_CALLTIPSHOW, position, text.as_ptr() as isize);
    let (start, end) = match highlight {
        Some(range) => (
            buffer.encoded_len(&tip[..range.start]),
            buffer.encoded_len(&tip[..range.end]),
        ),
        None => (0, 0),
    };
    send(sci, SCI_CALLTIPSETHLT, start, end as isize);
    with(|s| {
        s.tip.set(Tip::Parameters);
        s.tip_caret.set(position);
    });
}

/// Lists what can be typed at the caret: after `name.`, what `name`
/// contains; elsewhere, what is visible and keywords.
fn complete(trigger: Trigger) {
    let (sci, _) = current_scintilla();
    let id = current_buffer();
    if trigger == Trigger::Auto && !is_vhdl(id) {
        return;
    }
    let path = buffer_path(id);
    let buffer = read_buffer(sci);
    let position = caret(sci);
    let offset = buffer.offset(position);
    if trigger == Trigger::Auto && !completes_after_dot(&buffer.text, offset) {
        return;
    }
    let completions = with(|s| {
        s.engine
            .try_borrow_mut()
            .ok()
            .and_then(|mut engine| engine.completion(path.as_deref(), &buffer.text, offset))
    });
    let Some(completions) = completions else {
        return;
    };
    let items = completion_list(&completions);
    if items.is_empty() {
        return;
    }
    let entered = position.saturating_sub(buffer.position(completions.start));
    for (kind, image) in IMAGES {
        let xpm = c_string(xpm(*image).into_bytes());
        send(
            sci,
            SCI_REGISTERIMAGE,
            image_id(*kind),
            xpm.as_ptr() as isize,
        );
    }
    let list: Vec<String> = items
        .iter()
        .map(|c| format!("{}\x1e{}", c.label, image_id(c.kind)))
        .collect();
    let list = c_string(buffer.encode(&list.join("\n")));
    let details: HashMap<String, String> = items
        .iter()
        .filter(|c| !c.detail.is_empty())
        .map(|c| (c.label.clone(), c.detail.clone()))
        .collect();
    with(|s| *s.details.borrow_mut() = Some(details));
    // Notepad++ uses its own separators for its lists: put them back.
    let separator = send(sci, SCI_AUTOCGETSEPARATOR, 0, 0);
    let type_separator = send(sci, SCI_AUTOCGETTYPESEPARATOR, 0, 0);
    send(sci, SCI_AUTOCSETSEPARATOR, b'\n' as usize, 0);
    send(sci, SCI_AUTOCSETTYPESEPARATOR, 0x1E, 0);
    send(sci, SCI_AUTOCSETIGNORECASE, 1, 0);
    send(sci, SCI_AUTOCSHOW, entered, list.as_ptr() as isize);
    send(sci, SCI_AUTOCSETSEPARATOR, separator.max(0) as usize, 0);
    send(
        sci,
        SCI_AUTOCSETTYPESEPARATOR,
        type_separator.max(0) as usize,
        0,
    );
}

/// Re-reads what changed on disk outside Notepad++, at most every
/// [`RESCAN_INTERVAL`]. Whether anything changed.
fn rescan() -> bool {
    let now = std::time::Instant::now();
    if with(|s| s.last_rescan.get()).is_some_and(|last| now - last < RESCAN_INTERVAL) {
        return false;
    }
    with(|s| {
        s.last_rescan.set(Some(now));
        s.engine
            .try_borrow_mut()
            .is_ok_and(|mut engine| engine.rescan())
    })
}

/// Shows what the selected completion is in the status bar. Not in a call
/// tip, which would close the list.
fn show_detail(n: &ScNotification) {
    if n.text.is_null() {
        return;
    }
    // SAFETY: Scintilla passes the selected item's NUL-terminated text.
    let label = unsafe { std::ffi::CStr::from_ptr(n.text.cast()) };
    let sci = n.nmhdr.hwnd_from;
    let buffer_utf8 = send(sci, SCI_GETCODEPAGE, 0, 0) == SC_CP_UTF8;
    let label = BufferText::new(label.to_bytes(), buffer_utf8).text;
    let detail = with(|s| {
        s.details
            .borrow()
            .as_ref()
            .and_then(|d| d.get(&label).cloned())
    });
    let npp = npp();
    with(|s| {
        let mut saved = s.status.borrow_mut();
        if saved.is_none() {
            *saved = status_text(npp);
        }
    });
    match detail {
        Some(detail) => set_status_text(npp, &format!("{label}: {detail}")),
        None => restore_status(),
    }
}

/// Puts back the status bar text [`show_detail`] replaced.
fn restore_status() {
    if let Some(text) = with(|s| s.status.borrow_mut().take()) {
        set_status_text(npp(), &text);
    }
}

/// Completion list icons: a shape and a color per kind.
#[derive(Clone, Copy)]
enum Shape {
    Square,
    Circle,
    Diamond,
    Bar,
}

const IMAGES: &[(CompletionKind, (Shape, &str))] = &[
    (CompletionKind::Keyword, (Shape::Bar, "#808080")),
    (CompletionKind::Module, (Shape::Square, "#A0662B")),
    (CompletionKind::Unit, (Shape::Square, "#8E44AD")),
    (CompletionKind::Type, (Shape::Square, "#16A085")),
    (CompletionKind::Function, (Shape::Diamond, "#2E6FD8")),
    (CompletionKind::Procedure, (Shape::Diamond, "#6A4FD8")),
    (CompletionKind::Object, (Shape::Circle, "#D9822B")),
    (CompletionKind::Field, (Shape::Circle, "#27AE60")),
    (CompletionKind::Literal, (Shape::Bar, "#2E86C1")),
    (CompletionKind::Other, (Shape::Circle, "#808080")),
];

/// Image ids above Notepad++'s own.
fn image_id(kind: CompletionKind) -> usize {
    7100 + IMAGES.iter().position(|(k, _)| *k == kind).unwrap_or(0)
}

/// A 12 by 12 XPM image.
fn xpm((shape, color): (Shape, &str)) -> String {
    let mut text = format!(
        "/* XPM */\nstatic char *icon[] = {{\n\"12 12 2 1\",\n\"  c None\",\n\". c {color}\",\n"
    );
    for y in 0..12i32 {
        let row: String = (0..12i32)
            .map(|x| {
                let (dx, dy) = (2 * x - 11, 2 * y - 11);
                let filled = match shape {
                    Shape::Square => (1..11).contains(&x) && (1..11).contains(&y),
                    Shape::Circle => dx * dx + dy * dy <= 100,
                    Shape::Diamond => dx.abs() + dy.abs() <= 11,
                    Shape::Bar => (1..11).contains(&x) && (4..8).contains(&y),
                };
                if filled { '.' } else { ' ' }
            })
            .collect();
        text.push_str(&format!("\"{row}\"{}\n", if y < 11 { "," } else { "};" }));
    }
    text
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
                "code_assistance" => settings.assist = value,
                "highlight_uses" => settings.highlight = value,
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
        "format_on_save={}\nlint_automatically={}\nmessages_under_lines={}\n\
         code_assistance={}\nhighlight_uses={}\n",
        s.format_on_save as u8,
        s.auto_lint as u8,
        s.inline_messages as u8,
        s.assist as u8,
        s.highlight as u8
    );
    // A settings file that cannot be written only loses the settings.
    let _ = std::fs::write(path, text);
}
