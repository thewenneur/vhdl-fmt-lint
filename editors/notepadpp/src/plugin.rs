//! The Notepad++ side: the exported plugin functions, the menu, showing
//! problems, and code assistance. Navigation, rename and fixes are in
//! [`crate::tools`].
//!
//! Notepad++ calls the plugin on its UI thread only, so the state lives in
//! a thread local. Everything that analyzes a project runs on the
//! [`Worker`]'s thread instead, so that Notepad++ never waits for it: the UI
//! thread sends a copy of the buffer with each request, and the result comes
//! back through a hidden window, to a continuation that first checks the
//! buffer is still the one it was computed for.

#![allow(unsafe_code)]

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Arc;

use vfl_config::{Config, Preset, Style};
use vfl_lsp::editor::CompletionKind;
use vfl_lsp::generate::Kind;

use crate::engine::{
    BufferText, Engine, FormatFailure, NameColor, Severity, completes_after_dot, completion_list,
    editor_config, is_library_copy, is_project_file, is_verilog_path, is_vhdl_path, minimal_edit,
    plain_text, signature_tip,
};
use crate::tools;
use crate::win::*;
use crate::worker::{Key, Worker, panic_message};

/// Menu entries, in order. `None` is a separator.
const MENU: &[(&str, Option<extern "C" fn()>)] = &[
    ("Format document", Some(cmd_format)),
    ("Format selection", Some(cmd_format_selection)),
    ("Lint document", Some(cmd_lint)),
    ("Next problem", Some(cmd_next)),
    ("Previous problem", Some(cmd_previous)),
    ("List problems in project", Some(cmd_project_problems)),
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
    ("Insert entity instantiation", Some(cmd_insert_instance)),
    (
        "Insert component instantiation",
        Some(cmd_insert_component_instance),
    ),
    ("Insert component declaration", Some(cmd_insert_component)),
    ("Generate testbench", Some(cmd_testbench)),
    ("", None),
    ("Generate documentation", Some(cmd_generate_docs)),
    ("Show diagrams", Some(cmd_show_diagrams)),
    ("", None),
    ("Show type", Some(cmd_show_type)),
    ("Show parameters", Some(cmd_show_parameters)),
    ("Complete word", Some(cmd_complete)),
    ("", None),
    ("Format on save", Some(cmd_toggle_format_on_save)),
    ("Format on type", Some(cmd_toggle_format_on_type)),
    ("Lint automatically", Some(cmd_toggle_auto_lint)),
    ("Show messages under lines", Some(cmd_toggle_inline)),
    ("Code assistance", Some(cmd_toggle_assist)),
    ("Highlight uses", Some(cmd_toggle_highlight)),
    ("Color names by meaning", Some(cmd_toggle_name_colors)),
    ("", None),
    ("Reload project", Some(cmd_reload)),
    ("About", Some(cmd_about)),
    // Notepad++ keeps shortcuts by menu position: new entries go last.
    ("", None),
    ("Lint preset without vfl.toml...", Some(cmd_lint_preset)),
    ("Format style without vfl.toml...", Some(cmd_format_style)),
    (
        "Generate project config (vfl.toml)",
        Some(cmd_generate_config),
    ),
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
const FORMAT_ON_TYPE: usize = item("Format on type");
const AUTO_LINT: usize = item("Lint automatically");
const INLINE: usize = item("Show messages under lines");
const ASSIST: usize = item("Code assistance");
const HIGHLIGHT: usize = item("Highlight uses");
const NAME_COLORS: usize = item("Color names by meaning");

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
/// Indicators for name colors, one per [`NameColor`], when Notepad++ is
/// too old to hand them out. 21 is Notepad++'s fifth mark style, the one
/// least likely to be in use.
const FALLBACK_NAME_INDICATORS: [usize; NameColor::ALL.len()] = [8, 15, 16, 17, 18, 19, 20, 21];

/// Name colors (0x00BBGGRR) on a light and on a dark background. Input
/// ports are blue and output ports red on both: far apart for red-green
/// color blindness too, and clear of the keyword, comment and type colors
/// of Notepad++'s default and dark themes.
fn name_color(color: NameColor, dark: bool) -> isize {
    match (color, dark) {
        (NameColor::Signal, false) => 0xAA248E,
        (NameColor::PortIn, false) => 0xB25300,
        (NameColor::PortOut, false) => 0x0000D5,
        (NameColor::PortInout, false) => 0x5714AD,
        (NameColor::Constant, false) => 0x005EC2,
        (NameColor::Variable, false) => 0x801000,
        (NameColor::Type, false) => 0x997F26,
        (NameColor::Subprogram, false) => 0x265E79,
        (NameColor::Signal, true) => 0xC086C5,
        (NameColor::PortIn, true) => 0xFFA947,
        (NameColor::PortOut, true) => 0x5784FF,
        (NameColor::PortInout, true) => 0xB18FF4,
        (NameColor::Constant, true) => 0x50A0E5,
        (NameColor::Variable, true) => 0xFEDC9C,
        (NameColor::Type, true) => 0xB0C94E,
        (NameColor::Subprogram, true) => 0xAADCDC,
    }
}

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
    /// Formats the line of a `;` just typed, or the line above a new line.
    format_on_type: bool,
    auto_lint: bool,
    inline_messages: bool,
    /// Types on hover, parameter hints and completion as you type.
    assist: bool,
    /// Highlight the uses of the name at the caret.
    pub(crate) highlight: bool,
    /// Color signals, ports, constants, ... by what the analysis found.
    name_colors: bool,
    /// The lint preset and format style of files no `vfl.toml` applies to.
    pub(crate) lint_preset: Preset,
    pub(crate) format_style: Style,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            format_on_save: false,
            format_on_type: false,
            auto_lint: true,
            inline_messages: true,
            assist: true,
            highlight: true,
            name_colors: true,
            lint_preset: Preset::Recommended,
            format_style: Style::Default,
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

type Continuation = Box<dyn FnOnce(Box<dyn Any + Send>)>;

pub(crate) struct State {
    npp: Cell<NppData>,
    /// Formats buffers on this thread, since saving waits for it. It only
    /// reads configurations, never projects.
    formatter: RefCell<Engine>,
    /// Loads and analyzes projects, started on the first request.
    worker: RefCell<Option<Worker>>,
    /// What to do with the results still wanted, by job.
    pending: RefCell<HashMap<u64, Continuation>>,
    /// The latest job with each key, whose result replaces older ones'.
    keyed: RefCell<HashMap<Key, u64>>,
    /// The hidden window the worker wakes this thread through.
    window: Cell<Hwnd>,
    /// Counts edits to any buffer, to tell results computed for older text.
    edits: Cell<u64>,
    /// The buffer and position the mouse rests over.
    dwell: Cell<Option<(usize, usize)>>,
    /// Parameter hints were asked for and have not come back yet.
    parameters_pending: Cell<bool>,
    /// The name colors of the buffer in front.
    names: RefCell<Option<Names>>,
    /// The problems of each linted buffer, by buffer id, sorted.
    problems: RefCell<HashMap<usize, Vec<Problem>>>,
    /// Buffers showing our messages under lines.
    annotated: RefCell<HashSet<usize>>,
    pub(crate) settings: Cell<Settings>,
    indicators: Cell<Option<[usize; INDICATORS]>>,
    name_indicators: Cell<Option<[usize; NameColor::ALL.len()]>>,
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
        formatter: RefCell::default(),
        worker: RefCell::new(None),
        pending: RefCell::default(),
        keyed: RefCell::default(),
        window: Cell::new(ptr::null_mut()),
        edits: Cell::new(0),
        dwell: Cell::new(None),
        parameters_pending: Cell::new(false),
        names: RefCell::new(None),
        problems: RefCell::default(),
        annotated: RefCell::default(),
        settings: Cell::default(),
        indicators: Cell::new(None),
        name_indicators: Cell::new(None),
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
#[track_caller]
pub(crate) fn guard<T: Default>(f: impl FnOnce() -> T) -> T {
    let at = std::panic::Location::caller();
    guard_as(format_args!("{}:{}", at.file(), at.line()), f)
}

/// [`guard`], with what to call the call in the trace.
pub(crate) fn guard_as<T: Default>(label: std::fmt::Arguments, f: impl FnOnce() -> T) -> T {
    let start = std::time::Instant::now();
    let result = catch_unwind(AssertUnwindSafe(f));
    crate::trace::record(label, start);
    match result {
        Ok(value) => value,
        Err(payload) => {
            internal_error(&panic_message(&*payload));
            T::default()
        }
    }
}

fn internal_error(message: &str) {
    message_box(
        npp(),
        &format!("vfl hit an internal error and skipped this action.\n\n{message}"),
        MB_ICONWARNING,
    );
}

// Work on the worker's thread.

/// The message the worker posts when a result is ready.
const WM_RESULT: u32 = WM_APP + 0x7F1;

/// Runs `work` on the worker's thread, then `then` with its result on this
/// one. A request with a key replaces the one with the same key still
/// waiting or running: only the latest one's `then` runs.
pub(crate) fn request<T: Send + 'static>(
    key: Option<Key>,
    work: impl FnOnce(&mut Engine) -> T + Send + 'static,
    then: impl FnOnce(T) + 'static,
) {
    with(|s| {
        let mut worker = s.worker.borrow_mut();
        let worker = worker.get_or_insert_with(|| {
            let window = s.window.get() as usize;
            Worker::start(move || {
                // SAFETY: posting to a window that lives as long as
                // Notepad++; a failed post only loses a wake-up.
                unsafe { PostMessageW(window as Hwnd, WM_RESULT, 0, 0) };
            })
        });
        let seq = worker.submit(key, work);
        let then: Continuation = Box::new(move |value| {
            if let Ok(value) = value.downcast::<T>() {
                then(*value);
            }
        });
        let mut pending = s.pending.borrow_mut();
        pending.insert(seq, then);
        if let Some(key) = key
            && let Some(old) = s.keyed.borrow_mut().insert(key, seq)
        {
            pending.remove(&old);
        }
    })
}

/// Runs `work` on the worker's thread, in order with the requests, for its
/// effect alone.
pub(crate) fn post(work: impl FnOnce(&mut Engine) + Send + 'static) {
    request(None, work, |()| {});
}

unsafe extern "system" fn on_window_message(
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if msg == WM_RESULT {
        guard_as(format_args!("results"), deliver_results);
        return 0;
    }
    // SAFETY: the default handling of a message to our window.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Hands the finished jobs' results to their continuations.
fn deliver_results() {
    // A continuation can show a dialog, whose message loop delivers the
    // next results: take one at a time, holding no borrow.
    while let Some(done) = with(|s| s.worker.borrow().as_ref().and_then(Worker::result)) {
        let Some(then) = with(|s| s.pending.borrow_mut().remove(&done.seq)) else {
            continue;
        };
        match done.value {
            Ok(value) => guard(move || then(value)),
            Err(message) => {
                // Typing must not ask again for hints that cannot come.
                with(|s| s.parameters_pending.set(false));
                internal_error(&message);
            }
        }
    }
}

/// How many edits were made so far, to any buffer.
pub(crate) fn edits() -> u64 {
    with(|s| s.edits.get())
}

/// The current buffer, read for a request.
pub(crate) struct Here {
    pub(crate) sci: Hwnd,
    pub(crate) id: usize,
    pub(crate) path: Option<PathBuf>,
    pub(crate) buffer: Arc<BufferText>,
    /// The caret, as a Scintilla position.
    pub(crate) position: usize,
    /// [`edits`] when the buffer was read.
    pub(crate) edits: u64,
}

pub(crate) fn here() -> Here {
    let (sci, _) = current_scintilla();
    let id = current_buffer();
    Here {
        sci,
        id,
        path: buffer_path(id),
        buffer: Arc::new(read_buffer(sci)),
        position: caret(sci),
        edits: edits(),
    }
}

impl Here {
    /// The path to give the engine: `None` for unsaved buffers.
    pub(crate) fn path(&self) -> Option<&Path> {
        self.path.as_deref().filter(|p| p.is_absolute())
    }

    /// The caret, as a byte offset into the text.
    pub(crate) fn offset(&self) -> usize {
        self.buffer.offset(self.position)
    }

    /// Whether the buffer is still in front, unchanged.
    pub(crate) fn is_current(&self) -> bool {
        edits() == self.edits && current_buffer() == self.id
    }

    /// Asks the engine about this buffer at the caret on the worker's
    /// thread, then runs `then` with this and the answer here.
    pub(crate) fn ask<T: Send + 'static>(
        self,
        key: Key,
        work: impl FnOnce(&mut Engine, Option<&Path>, &BufferText, usize) -> T + Send + 'static,
        then: impl FnOnce(Here, T) + 'static,
    ) {
        let path = self.path().map(Path::to_owned);
        let buffer = self.buffer.clone();
        let position = self.position;
        request(
            Some(key),
            move |engine| {
                let offset = buffer.offset(position);
                work(engine, path.as_deref(), &buffer, offset)
            },
            move |answer| then(self, answer),
        );
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
        pin_module();
        let window = message_window("vfl_npp_results", on_window_message);
        with(|s| s.window.set(window));
        apply_defaults();
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
                            FORMAT_ON_TYPE => settings.format_on_type,
                            AUTO_LINT => settings.auto_lint,
                            INLINE => settings.inline_messages,
                            ASSIST => settings.assist,
                            HIGHLIGHT => settings.highlight,
                            NAME_COLORS => settings.name_colors,
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
    guard_as(format_args!("notification {}", n.nmhdr.code), || {
        on_notification(n)
    });
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
            rescan(false);
            lint_current(Trigger::Auto);
        }
        NPPN_BUFFERACTIVATED | NPPN_LANGCHANGED if from_npp => {
            rescan(false);
            lint_current(Trigger::Auto);
            tools::schedule_highlight();
        }
        // Coming back to Notepad++, maybe after changing files elsewhere.
        SCN_FOCUSIN if n.nmhdr.hwnd_from == current_scintilla().0 => rescan(true),
        NPPN_FILEBEFORESAVE if from_npp => {
            let id = n.nmhdr.id_from;
            if with(|s| s.settings.get().format_on_save) && id == current_buffer() && is_hdl(id) {
                format_current(Trigger::Auto);
            }
        }
        NPPN_FILESAVED if from_npp => {
            let id = n.nmhdr.id_from;
            if buffer_path(id).is_some_and(|p| is_project_file(&p)) {
                reload();
            }
            if id == current_buffer() {
                lint_current(Trigger::Auto);
            }
        }
        NPPN_FILEBEFORECLOSE if from_npp => {
            let id = n.nmhdr.id_from;
            if let Some(path) = buffer_path(id) {
                post(move |engine| engine.forget(&path));
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
        SCN_MODIFIED if n.modification_type & (SC_MOD_INSERTTEXT | SC_MOD_DELETETEXT) != 0 => {
            with(|s| s.edits.set(s.edits.get() + 1));
            if n.nmhdr.hwnd_from == current_scintilla().0 && with(|s| s.settings.get().auto_lint) {
                schedule_lint();
            }
        }
        SCN_CHARADDED if n.nmhdr.hwnd_from == current_scintilla().0 => on_char(n.ch),
        SCN_UPDATEUI if n.nmhdr.hwnd_from == current_scintilla().0 => {
            paint_visible_names(n.nmhdr.hwnd_from);
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
                with(|s| s.dwell.set(Some((current_buffer(), position))));
                show_info_at(n.nmhdr.hwnd_from, position, Trigger::Auto);
            }
        }
        SCN_DWELLEND => {
            with(|s| s.dwell.set(None));
            if with(|s| s.tip.get()) == Tip::Info {
                with(|s| s.tip.set(Tip::None));
                send(n.nmhdr.hwnd_from, SCI_CALLTIPCANCEL, 0, 0);
            }
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
        rescan(false);
        lint_current(Trigger::Command);
    });
}

extern "C" fn cmd_next() {
    guard(|| step_problem(true));
}

extern "C" fn cmd_previous() {
    guard(|| step_problem(false));
}

extern "C" fn cmd_project_problems() {
    guard(tools::project_problems);
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

extern "C" fn cmd_insert_instance() {
    guard(|| tools::generate(Kind::Instance));
}

extern "C" fn cmd_insert_component_instance() {
    guard(|| tools::generate(Kind::ComponentInstance));
}

extern "C" fn cmd_insert_component() {
    guard(|| tools::generate(Kind::Component));
}

extern "C" fn cmd_testbench() {
    guard(|| tools::generate(Kind::Testbench));
}

extern "C" fn cmd_generate_docs() {
    guard(tools::generate_docs);
}

extern "C" fn cmd_generate_config() {
    guard(tools::generate_config);
}

extern "C" fn cmd_lint_preset() {
    guard(tools::choose_preset);
}

extern "C" fn cmd_format_style() {
    guard(tools::choose_style);
}

extern "C" fn cmd_show_diagrams() {
    guard(tools::show_diagrams);
}

extern "C" fn cmd_toggle_highlight() {
    guard(|| {
        toggle(HIGHLIGHT, |s| &mut s.highlight);
        tools::schedule_highlight();
    });
}

extern "C" fn cmd_toggle_name_colors() {
    guard(|| {
        toggle(NAME_COLORS, |s| &mut s.name_colors);
        lint_current(Trigger::Refresh);
    });
}

extern "C" fn cmd_show_type() {
    guard(|| {
        let (sci, _) = current_scintilla();
        show_info_at(sci, caret(sci), Trigger::Command);
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

extern "C" fn cmd_toggle_format_on_type() {
    guard(|| toggle(FORMAT_ON_TYPE, |s| &mut s.format_on_type));
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
        reload();
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
                 Settings come from vfl.toml, found like the vfl command does; \
                 without one, from the lint preset and format style in this menu.\n\n\
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

/// Whether a buffer is VHDL, Verilog or SystemVerilog, which vfl formats
/// and checks.
pub(crate) fn is_hdl(id: usize) -> bool {
    is_vhdl(id)
        || send(npp(), NPPM_GETBUFFERLANGTYPE, id, 0) == L_VERILOG
        || buffer_path(id).is_some_and(|p| is_verilog_path(&p))
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
    let result = with(|s| match s.formatter.try_borrow_mut() {
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
    let result = with(|s| match s.formatter.try_borrow_mut() {
        Ok(mut engine) => Some(engine.format_range(path.as_deref(), &buffer.text, start..end)),
        Err(_) => None,
    });
    show_format_result(sci, &buffer, result, Trigger::Command);
}

/// Formats the line just finished after `ch` was typed, silently.
fn format_typed(ch: char) {
    let (sci, _) = current_scintilla();
    let id = current_buffer();
    if !is_hdl(id) {
        return;
    }
    let path = buffer_path(id);
    let buffer = read_buffer(sci);
    let cursor = buffer.offset(caret(sci));
    let result = with(|s| {
        s.formatter
            .try_borrow_mut()
            .ok()
            .and_then(|mut engine| engine.format_typed(path.as_deref(), &buffer.text, cursor, ch))
    });
    if let Some(formatted) = result {
        replace_text(sci, &buffer, &formatted);
    }
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
    let id = current_buffer();
    let auto = with(|s| s.settings.get().auto_lint);
    let path = buffer_path(id);
    // Library sources opened by go to definition are not the user's.
    let library = path.as_deref().is_some_and(is_library_copy);
    let wanted = match trigger {
        Trigger::Auto => auto && is_hdl(id) && !library,
        Trigger::Command => true,
        Trigger::Refresh => {
            (is_hdl(id) && !library) || with(|s| s.problems.borrow().contains_key(&id))
        }
    };
    if !wanted {
        return;
    }
    let colors = with(|s| s.settings.get().name_colors);
    let here = here();
    let (path, buffer) = (path.filter(|p| p.is_absolute()), here.buffer.clone());
    request(
        Some(Key::Lint),
        move |engine| lint(engine, path.as_deref(), &buffer, colors),
        move |linted| show_lint(here, trigger, linted),
    );
}

/// A buffer's problems and name colors, at Scintilla positions.
struct Linted {
    problems: Vec<Problem>,
    /// Ranges to color, with the index of their color in [`NameColor::ALL`].
    names: Vec<(usize, usize, usize)>,
    config_errors: Vec<String>,
    notices: Vec<String>,
}

/// Lints a buffer, on the worker's thread.
fn lint(engine: &mut Engine, path: Option<&Path>, buffer: &BufferText, colors: bool) -> Linted {
    let output = engine.lint(path, &buffer.text);
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
    let mut names = Vec::new();
    if colors {
        // Unused names stay greyed out.
        let unused: Vec<(usize, usize)> = problems
            .iter()
            .filter(|p| p.unused)
            .map(|p| (p.start, p.end))
            .collect();
        for (range, color) in engine.name_colors(path, &buffer.text) {
            let start = buffer.position(range.start);
            let end = buffer.position(range.end);
            if start >= end || unused.iter().any(|&(s, e)| start < e && s < end) {
                continue;
            }
            let index = NameColor::ALL.iter().position(|c| *c == color).unwrap_or(0);
            names.push((start, end, index));
        }
    }
    Linted {
        problems,
        names,
        config_errors: output.config_errors,
        notices: output.notices,
    }
}

/// Shows the problems found in `here`'s buffer, if it did not change since.
fn show_lint(here: Here, trigger: Trigger, linted: Linted) {
    if !here.is_current() {
        // Typing schedules the next lint; other triggers ask again.
        if trigger != Trigger::Auto && current_buffer() == here.id {
            lint_current(trigger);
        }
        return;
    }
    let (sci, view) = current_scintilla();
    let id = here.id;
    let Linted {
        problems,
        names,
        config_errors,
        notices,
    } = linted;
    quietly(sci, || show_problems(sci, view, id, &problems));
    show_name_colors(sci, id, here.edits, names);
    let count = problems.len();
    with(|s| s.problems.borrow_mut().insert(id, problems));

    // Each notice once, like the language server.
    let new: Vec<String> = with(|s| {
        let mut shown = s.shown_errors.borrow_mut();
        notices
            .into_iter()
            .filter(|n| shown.insert(n.clone()))
            .collect()
    });
    if !new.is_empty() {
        message_box(npp(), &new.join("\n\n"), MB_ICONWARNING);
    }

    if trigger != Trigger::Command {
        // Show each configuration error once, like the language server.
        let new: Vec<String> = with(|s| {
            let mut shown = s.shown_errors.borrow_mut();
            config_errors
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
        if !config_errors.is_empty() {
            message_box(
                npp(),
                &format!(
                    "Problems in the configuration:\n\n{}",
                    config_errors.join("\n")
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

fn name_indicators() -> [usize; NameColor::ALL.len()] {
    if let Some(indicators) = with(|s| s.name_indicators.get()) {
        return indicators;
    }
    let mut start: i32 = 0;
    let allocated = send(
        npp(),
        NPPM_ALLOCATEINDICATOR,
        NameColor::ALL.len(),
        &mut start as *mut i32 as isize,
    );
    let indicators = match usize::try_from(start) {
        Ok(start) if allocated != 0 && start > 0 => std::array::from_fn(|i| start + i),
        _ => FALLBACK_NAME_INDICATORS,
    };
    with(|s| s.name_indicators.set(Some(indicators)));
    indicators
}

/// Colors the names of the buffer by what they resolve to: `names` are
/// ranges with the index of their color in [`NameColor::ALL`], in order.
/// Only the lines on screen are colored, then the others as they scroll
/// into view: filling thousands of ranges at once takes seconds.
fn show_name_colors(sci: Hwnd, id: usize, edits: u64, names: Vec<(usize, usize, usize)>) {
    let len = send(sci, SCI_GETLENGTH, 0, 0);
    let indicators = name_indicators();
    quietly(sci, || {
        for indicator in indicators {
            send(sci, SCI_SETINDICATORCURRENT, indicator, 0);
            send(sci, SCI_INDICATORCLEARRANGE, 0, len);
        }
    });
    if names.is_empty() {
        with(|s| *s.names.borrow_mut() = None);
        return;
    }
    // The theme's background tells which palette reads well.
    let back = send(sci, SCI_STYLEGETBACK, STYLE_DEFAULT, 0);
    let (r, g, b) = (back & 0xFF, (back >> 8) & 0xFF, (back >> 16) & 0xFF);
    let dark = r * 299 + g * 587 + b * 114 < 128_000;
    for (color, indicator) in NameColor::ALL.iter().zip(indicators) {
        send(sci, SCI_INDICSETSTYLE, indicator, INDIC_TEXTFORE);
        send(sci, SCI_INDICSETFORE, indicator, name_color(*color, dark));
    }
    with(|s| {
        *s.names.borrow_mut() = Some(Names {
            id,
            edits,
            ranges: names,
            painted: HashSet::new(),
        })
    });
    paint_visible_names(sci);
}

/// The name colors of the buffer in front, to paint as it scrolls.
struct Names {
    id: usize,
    /// [`edits`] when they were found; after more edits they are stale.
    edits: u64,
    ranges: Vec<(usize, usize, usize)>,
    /// The blocks of [`NAME_BLOCK`] lines already colored.
    painted: HashSet<usize>,
}

/// How many lines are colored at a time.
const NAME_BLOCK: usize = 16;

/// Colors the names on screen that are not colored yet.
fn paint_visible_names(sci: Hwnd) {
    let Some(blocks) = with(|s| {
        let names = s.names.borrow();
        let names = names.as_ref()?;
        if names.edits != s.edits.get() || names.id != current_buffer() {
            return None;
        }
        let first = send(sci, SCI_GETFIRSTVISIBLELINE, 0, 0).max(0) as usize;
        let screen = send(sci, SCI_LINESONSCREEN, 0, 0).max(1) as usize;
        let doc_line =
            |visible: usize| send(sci, SCI_DOCLINEFROMVISIBLE, visible, 0).max(0) as usize;
        let top = doc_line(first);
        let bottom = doc_line(first + screen);
        let blocks: Vec<usize> = (top / NAME_BLOCK..=bottom / NAME_BLOCK)
            .filter(|b| !names.painted.contains(b))
            .collect();
        (!blocks.is_empty()).then_some(blocks)
    }) else {
        return;
    };
    let indicators = name_indicators();
    let lines = send(sci, SCI_GETLINECOUNT, 0, 0).max(0) as usize;
    let len = send(sci, SCI_GETLENGTH, 0, 0).max(0) as usize;
    let position = |line: usize| {
        if line >= lines {
            len
        } else {
            send(sci, SCI_POSITIONFROMLINE, line, 0).max(0) as usize
        }
    };
    // Fill outside the borrow: Scintilla may call back.
    let fills: Vec<(usize, usize, usize)> = with(|s| {
        let mut names = s.names.borrow_mut();
        let Some(names) = names.as_mut() else {
            return Vec::new();
        };
        let mut fills = Vec::new();
        for block in blocks {
            names.painted.insert(block);
            let (from, to) = (
                position(block * NAME_BLOCK),
                position((block + 1) * NAME_BLOCK),
            );
            let first = names.ranges.partition_point(|r| r.0 < from);
            fills.extend(
                names.ranges[first..]
                    .iter()
                    .take_while(|r| r.0 < to)
                    .map(|&(start, end, index)| (start, end.min(len), index))
                    .filter(|(start, end, _)| start < end),
            );
        }
        fills
    });
    quietly(sci, || {
        for (start, end, index) in fills {
            send(sci, SCI_SETINDICATORCURRENT, indicators[index], 0);
            send(sci, SCI_INDICATORFILLRANGE, start, (end - start) as isize);
        }
    });
}

/// Runs `f`, which sets indicators, without Scintilla telling Notepad++ and
/// every plugin about each range it fills: thousands of notifications take
/// seconds.
pub(crate) fn quietly<T>(sci: Hwnd, f: impl FnOnce() -> T) -> T {
    let mask = send(sci, SCI_GETMODEVENTMASK, 0, 0);
    send(
        sci,
        SCI_SETMODEVENTMASK,
        (mask & !SC_MOD_CHANGEINDICATOR) as usize,
        0,
    );
    let result = f();
    send(sci, SCI_SETMODEVENTMASK, mask as usize, 0);
    result
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
    // Both re-lay out the whole view, even when nothing changes.
    if send(sci, SCI_ANNOTATIONGETSTYLEOFFSET, 0, 0) != base {
        send(sci, SCI_ANNOTATIONSETSTYLEOFFSET, base as usize, 0);
    }
    if send(sci, SCI_ANNOTATIONGETVISIBLE, 0, 0) != ANNOTATION_STANDARD as isize {
        send(sci, SCI_ANNOTATIONSETVISIBLE, ANNOTATION_STANDARD, 0);
    }
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
    // Each change re-lays out the whole view: change only what differs.
    for severity in [Severity::Info, Severity::Warning, Severity::Error] {
        let style = (base + severity_index(severity) as isize) as usize;
        let mut current = [0u8; 128];
        send(sci, SCI_STYLEGETFONT, style, current.as_mut_ptr() as isize);
        if font[0] != 0 && current != font {
            send(sci, SCI_STYLESETFONT, style, font.as_ptr() as isize);
        }
        for (get, set, value) in [
            (SCI_STYLEGETSIZE, SCI_STYLESETSIZE, size),
            (SCI_STYLEGETFORE, SCI_STYLESETFORE, color(severity)),
            (SCI_STYLEGETBACK, SCI_STYLESETBACK, back),
            (SCI_STYLEGETITALIC, SCI_STYLESETITALIC, 1),
        ] {
            if send(sci, get, style, 0) != value {
                send(sci, set, style, value);
            }
        }
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
/// assistance on, the type of what is named there, once the worker has
/// found it. A command says when there is nothing to show.
fn show_info_at(sci: Hwnd, position: usize, trigger: Trigger) {
    let id = current_buffer();
    let parts: Vec<Vec<u8>> = with(|s| {
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
    show_tip(sci, position, &parts);
    if !(with(|s| s.settings.get().assist) && is_vhdl(id)) {
        if parts.is_empty() && trigger == Trigger::Command {
            message_box(npp(), "Nothing to show here.", MB_ICONINFORMATION);
        }
        return;
    }
    let mut here = here();
    here.position = position;
    here.ask(
        Key::Info,
        |engine, path, buffer, offset| {
            engine
                .hover(path, &buffer.text, offset)
                .map(|hover| buffer.encode(&plain_text(&hover.markdown)))
        },
        move |here, hover| {
            // Only while the mouse rests there, or the caret for a command.
            let wanted = match trigger {
                Trigger::Command => caret(here.sci) == position,
                _ => with(|s| s.dwell.get()) == Some((here.id, position)),
            };
            if !here.is_current() || !wanted {
                return;
            }
            let mut parts = parts;
            parts.extend(hover);
            if !show_tip(here.sci, position, &parts) && trigger == Trigger::Command {
                message_box(npp(), "Nothing to show here.", MB_ICONINFORMATION);
            }
        },
    );
}

/// Shows `parts` in a call tip at `position`. Whether there were any.
fn show_tip(sci: Hwnd, position: usize, parts: &[Vec<u8>]) -> bool {
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
    let Some(ch) = u32::try_from(ch).ok().and_then(char::from_u32) else {
        return;
    };
    if matches!(ch, ';' | '\n') && with(|s| s.settings.get().format_on_type) {
        format_typed(ch);
    }
    if !with(|s| s.settings.get().assist) {
        return;
    }
    match ch {
        '.' => complete(Trigger::Auto),
        '(' | ',' => show_parameters(Trigger::Auto),
        // Typing in a call moves between parameters (`name =>`) and out
        // of it (`)`).
        _ if with(|s| s.tip.get() == Tip::Parameters || s.parameters_pending.get()) => {
            show_parameters(Trigger::Auto)
        }
        _ => {}
    }
}

/// Shows the parameters of the call or port map around the caret, the
/// one being typed highlighted. Closes the hints when there is none.
fn show_parameters(trigger: Trigger) {
    let id = current_buffer();
    if trigger == Trigger::Auto && !is_vhdl(id) {
        return;
    }
    with(|s| s.parameters_pending.set(true));
    here().ask(
        Key::Parameters,
        |engine, path, buffer, offset| {
            let help = engine.signature_help(path, &buffer.text, offset)?;
            let (tip, highlight) = signature_tip(&help);
            let highlight = match highlight {
                Some(range) => (
                    buffer.encoded_len(&tip[..range.start]),
                    buffer.encoded_len(&tip[..range.end]),
                ),
                None => (0, 0),
            };
            Some((c_string(buffer.encode(&tip)), highlight))
        },
        move |here, help| {
            with(|s| s.parameters_pending.set(false));
            let sci = here.sci;
            // Typing on asked again already.
            if !here.is_current() || caret(sci) != here.position {
                return;
            }
            let Some((text, (start, end))) = help else {
                if with(|s| s.tip.get()) == Tip::Parameters {
                    send(sci, SCI_CALLTIPCANCEL, 0, 0);
                    with(|s| s.tip.set(Tip::None));
                }
                if trigger == Trigger::Command {
                    message_box(npp(), "The caret is not in a call.", MB_ICONINFORMATION);
                }
                return;
            };
            send(sci, SCI_CALLTIPSHOW, here.position, text.as_ptr() as isize);
            send(sci, SCI_CALLTIPSETHLT, start, end as isize);
            with(|s| {
                s.tip.set(Tip::Parameters);
                s.tip_caret.set(here.position);
            });
        },
    );
}

/// A completion list for Scintilla.
struct CompletionList {
    /// Where the word being completed starts, as a Scintilla position.
    start: usize,
    /// `label\x1eimage` items separated by `\n`, NUL-terminated.
    list: Vec<u8>,
    /// The details of the items, by label.
    details: HashMap<String, String>,
}

/// Lists what can be typed at the caret: after `name.`, what `name`
/// contains; elsewhere, what is visible and keywords.
fn complete(trigger: Trigger) {
    let id = current_buffer();
    if trigger == Trigger::Auto && !is_vhdl(id) {
        return;
    }
    let here = here();
    if trigger == Trigger::Auto && !completes_after_dot(&here.buffer.text, here.offset()) {
        return;
    }
    here.ask(
        Key::Completion,
        |engine, path, buffer, offset| {
            let completions = engine.completion(path, &buffer.text, offset)?;
            let items = completion_list(&completions);
            if items.is_empty() {
                return None;
            }
            let list: Vec<String> = items
                .iter()
                .map(|c| format!("{}\x1e{}", c.label, image_id(c.kind)))
                .collect();
            Some(CompletionList {
                start: buffer.position(completions.start),
                list: c_string(buffer.encode(&list.join("\n"))),
                details: items
                    .iter()
                    .filter(|c| !c.detail.is_empty())
                    .map(|c| (c.label.clone(), c.detail.clone()))
                    .collect(),
            })
        },
        |here, list| {
            if let Some(list) = list {
                show_completions(here, list);
            }
        },
    );
}

fn show_completions(here: Here, completions: CompletionList) {
    let sci = here.sci;
    if current_buffer() != here.id {
        return;
    }
    let position = caret(sci);
    if !here.is_current() {
        // Keep the list if the caret only went on typing the word.
        let now = read_buffer(sci);
        let typed = now
            .text
            .get(now.offset(here.position)..now.offset(position))
            .unwrap_or("");
        if position < here.position
            || !typed.chars().all(|c| c.is_alphanumeric() || c == '_')
            || now.text.get(..now.offset(here.position)) != here.buffer.text.get(..here.offset())
        {
            return;
        }
    }
    let entered = position.saturating_sub(completions.start);
    for (kind, image) in IMAGES {
        let xpm = c_string(xpm(*image).into_bytes());
        send(
            sci,
            SCI_REGISTERIMAGE,
            image_id(*kind),
            xpm.as_ptr() as isize,
        );
    }
    with(|s| *s.details.borrow_mut() = Some(completions.details));
    // Notepad++ uses its own separators for its lists: put them back.
    let separator = send(sci, SCI_AUTOCGETSEPARATOR, 0, 0);
    let type_separator = send(sci, SCI_AUTOCGETTYPESEPARATOR, 0, 0);
    send(sci, SCI_AUTOCSETSEPARATOR, b'\n' as usize, 0);
    send(sci, SCI_AUTOCSETTYPESEPARATOR, 0x1E, 0);
    send(sci, SCI_AUTOCSETIGNORECASE, 1, 0);
    send(
        sci,
        SCI_AUTOCSHOW,
        entered,
        completions.list.as_ptr() as isize,
    );
    send(sci, SCI_AUTOCSETSEPARATOR, separator.max(0) as usize, 0);
    send(
        sci,
        SCI_AUTOCSETTYPESEPARATOR,
        type_separator.max(0) as usize,
        0,
    );
}

/// Re-reads what changed on disk outside Notepad++, at most every
/// [`RESCAN_INTERVAL`], and lints the buffer again if anything did and
/// `lint` is set.
fn rescan(lint: bool) {
    let now = std::time::Instant::now();
    if with(|s| s.last_rescan.get()).is_some_and(|last| now - last < RESCAN_INTERVAL) {
        return;
    }
    with(|s| s.last_rescan.set(Some(now)));
    request(Some(Key::Rescan), Engine::rescan, move |changed| {
        if changed {
            with(|s| s.formatter.borrow_mut().reload());
            if lint {
                lint_current(Trigger::Auto);
            }
        }
    });
}

/// Changes the lint preset or format style of files no `vfl.toml` applies
/// to, and shows the current buffer's problems with it.
pub(crate) fn change_defaults(change: impl FnOnce(&mut Settings)) {
    let mut settings = with(|s| s.settings.get());
    change(&mut settings);
    with(|s| s.settings.set(settings));
    save_settings();
    apply_defaults();
    lint_current(Trigger::Refresh);
}

/// Hands the lint preset and format style to both engines.
fn apply_defaults() {
    let settings = with(|s| s.settings.get());
    let config = editor_config(settings.lint_preset, settings.format_style);
    with(|s| s.formatter.borrow_mut().set_defaults(config.clone()));
    // A worker not started yet has the default config: start it only for
    // another one.
    if with(|s| s.worker.borrow().is_some()) || config != Config::default() {
        post(move |engine| engine.set_defaults(config));
    }
}

/// Drops the loaded projects and configurations.
pub(crate) fn reload() {
    with(|s| s.formatter.borrow_mut().reload());
    post(Engine::reload);
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
            let value = value.trim();
            match key.trim() {
                "lint_preset" => {
                    if let Some(p) = Preset::ALL.into_iter().find(|p| p.name() == value) {
                        settings.lint_preset = p;
                    }
                    continue;
                }
                "format_style" => {
                    if let Some(s) = Style::ALL.into_iter().find(|s| s.name() == value) {
                        settings.format_style = s;
                    }
                    continue;
                }
                _ => {}
            }
            let value = value == "1";
            match key.trim() {
                "format_on_save" => settings.format_on_save = value,
                "format_on_type" => settings.format_on_type = value,
                "lint_automatically" => settings.auto_lint = value,
                "messages_under_lines" => settings.inline_messages = value,
                "code_assistance" => settings.assist = value,
                "highlight_uses" => settings.highlight = value,
                "color_names" => settings.name_colors = value,
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
        "format_on_save={}\nformat_on_type={}\nlint_automatically={}\nmessages_under_lines={}\n\
         code_assistance={}\nhighlight_uses={}\ncolor_names={}\nlint_preset={}\n\
         format_style={}\n",
        s.format_on_save as u8,
        s.format_on_type as u8,
        s.auto_lint as u8,
        s.inline_messages as u8,
        s.assist as u8,
        s.highlight as u8,
        s.name_colors as u8,
        s.lint_preset.name(),
        s.format_style.name()
    );
    // A settings file that cannot be written only loses the settings.
    let _ = std::fs::write(path, text);
}
