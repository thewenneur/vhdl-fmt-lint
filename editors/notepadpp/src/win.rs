//! The parts of Win32, Scintilla and the Notepad++ plugin interface the
//! plugin uses. Values come from `Scintilla.h`, `Notepad_plus_msgs.h`
//! and `PluginInterface.h`.

#![allow(unsafe_code, non_snake_case, dead_code)]

use std::ffi::c_void;

pub type Hwnd = *mut c_void;

#[link(name = "user32")]
unsafe extern "system" {
    pub fn SendMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    pub fn MessageBoxW(hwnd: Hwnd, text: *const u16, caption: *const u16, kind: u32) -> i32;
    pub fn SetTimer(
        hwnd: Hwnd,
        id: usize,
        elapse: u32,
        func: Option<unsafe extern "system" fn(Hwnd, u32, usize, u32)>,
    ) -> usize;
    pub fn KillTimer(hwnd: Hwnd, id: usize) -> i32;
    fn DialogBoxIndirectParamW(
        instance: Hwnd,
        template: *const c_void,
        parent: Hwnd,
        proc: Option<unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize>,
        param: isize,
    ) -> isize;
    fn EndDialog(dialog: Hwnd, result: isize) -> i32;
    fn GetDlgItem(dialog: Hwnd, id: i32) -> Hwnd;
    fn GetDlgItemTextW(dialog: Hwnd, id: i32, text: *mut u16, max: i32) -> u32;
    fn SetDlgItemTextW(dialog: Hwnd, id: i32, text: *const u16) -> i32;
    fn SetFocus(hwnd: Hwnd) -> Hwnd;
    fn FindWindowExW(parent: Hwnd, after: Hwnd, class: *const u16, title: *const u16) -> Hwnd;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Hwnd;
}

pub const MB_OK: u32 = 0x0;
pub const MB_OKCANCEL: u32 = 0x1;
pub const MB_ICONQUESTION: u32 = 0x20;
pub const MB_ICONWARNING: u32 = 0x30;
pub const MB_ICONINFORMATION: u32 = 0x40;
pub const IDOK: i32 = 1;
pub const IDCANCEL: i32 = 2;

// PluginInterface.h

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NppData {
    pub npp: Hwnd,
    pub scintilla_main: Hwnd,
    pub scintilla_second: Hwnd,
}

#[repr(C)]
pub struct ShortcutKey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: u8,
}

pub const MENU_ITEM_SIZE: usize = 64;

#[repr(C)]
pub struct FuncItem {
    pub name: [u16; MENU_ITEM_SIZE],
    pub func: Option<extern "C" fn()>,
    pub cmd_id: i32,
    pub init_checked: bool,
    pub shortcut: *mut ShortcutKey,
}

// Scintilla.h

#[repr(C)]
pub struct NotifyHeader {
    pub hwnd_from: Hwnd,
    pub id_from: usize,
    pub code: u32,
}

/// The start of `SCNotification`, up to `listType`; the plugin reads no
/// further fields.
#[repr(C)]
pub struct ScNotification {
    pub nmhdr: NotifyHeader,
    pub position: isize,
    pub ch: i32,
    pub modifiers: i32,
    pub modification_type: i32,
    pub text: *const u8,
    pub length: isize,
    pub lines_added: isize,
    pub message: i32,
    pub wparam: usize,
    pub lparam: isize,
    pub line: isize,
    pub fold_level_now: i32,
    pub fold_level_prev: i32,
    pub margin: i32,
    pub list_type: i32,
}

pub const SCI_GETLENGTH: u32 = 2006;
pub const SCI_GETCURRENTPOS: u32 = 2008;
pub const SCI_GOTOPOS: u32 = 2025;
pub const SCI_AUTOCSHOW: u32 = 2100;
pub const SCI_AUTOCCANCEL: u32 = 2101;
pub const SCI_AUTOCACTIVE: u32 = 2102;
pub const SCI_AUTOCSETSEPARATOR: u32 = 2106;
pub const SCI_AUTOCGETSEPARATOR: u32 = 2107;
pub const SCI_AUTOCSETIGNORECASE: u32 = 2115;
pub const SCI_USERLISTSHOW: u32 = 2117;
pub const SCI_STYLESETFORE: u32 = 2051;
pub const SCI_STYLESETBACK: u32 = 2052;
pub const SCI_STYLESETITALIC: u32 = 2054;
pub const SCI_STYLESETSIZE: u32 = 2055;
pub const SCI_STYLESETFONT: u32 = 2056;
pub const SCI_BEGINUNDOACTION: u32 = 2078;
pub const SCI_ENDUNDOACTION: u32 = 2079;
pub const SCI_INDICSETSTYLE: u32 = 2080;
pub const SCI_INDICSETFORE: u32 = 2082;
pub const SCI_GETCOLUMN: u32 = 2129;
pub const SCI_GETCODEPAGE: u32 = 2137;
pub const SCI_GETFIRSTVISIBLELINE: u32 = 2152;
pub const SCI_GETSELECTIONSTART: u32 = 2143;
pub const SCI_GETSELECTIONEND: u32 = 2145;
pub const SCI_SETSEL: u32 = 2160;
pub const SCI_LINEFROMPOSITION: u32 = 2166;
pub const SCI_SCROLLCARET: u32 = 2169;
pub const SCI_REPLACETARGET: u32 = 2194;
pub const SCI_CALLTIPSHOW: u32 = 2200;
pub const SCI_CALLTIPCANCEL: u32 = 2201;
pub const SCI_CALLTIPACTIVE: u32 = 2202;
pub const SCI_CALLTIPSETHLT: u32 = 2204;
pub const SCI_CALLTIPSETPOSITION: u32 = 2213;
pub const SCI_ENSUREVISIBLE: u32 = 2232;
pub const SCI_SETMOUSEDWELLTIME: u32 = 2264;
pub const SCI_GETMOUSEDWELLTIME: u32 = 2265;
pub const SCI_WORDSTARTPOSITION: u32 = 2266;
pub const SCI_WORDENDPOSITION: u32 = 2267;
pub const SCI_AUTOCGETTYPESEPARATOR: u32 = 2285;
pub const SCI_AUTOCSETTYPESEPARATOR: u32 = 2286;
pub const SCI_REGISTERIMAGE: u32 = 2405;
pub const SCI_FINDCOLUMN: u32 = 2456;
pub const SCI_STYLEGETBACK: u32 = 2482;
pub const SCI_STYLEGETSIZE: u32 = 2485;
pub const SCI_STYLEGETFONT: u32 = 2486;
pub const SCI_SETINDICATORCURRENT: u32 = 2500;
pub const SCI_INDICATORFILLRANGE: u32 = 2504;
pub const SCI_INDICATORCLEARRANGE: u32 = 2505;
pub const SCI_INDICSETUNDER: u32 = 2510;
pub const SCI_INDICSETALPHA: u32 = 2523;
pub const SCI_INDICSETOUTLINEALPHA: u32 = 2558;
pub const SCI_GETCHARACTERPOINTER: u32 = 2520;
pub const SCI_ANNOTATIONSETTEXT: u32 = 2540;
pub const SCI_ANNOTATIONSETSTYLE: u32 = 2542;
pub const SCI_ANNOTATIONCLEARALL: u32 = 2547;
pub const SCI_ANNOTATIONSETVISIBLE: u32 = 2548;
pub const SCI_ANNOTATIONSETSTYLEOFFSET: u32 = 2550;
pub const SCI_ALLOCATEEXTENDEDSTYLES: u32 = 2553;
pub const SCI_SETFIRSTVISIBLELINE: u32 = 2613;
pub const SCI_AUTOCSETORDER: u32 = 2660;
pub const SCI_AUTOCGETORDER: u32 = 2661;
pub const SCI_SETTARGETRANGE: u32 = 2686;

pub const SC_CP_UTF8: isize = 65001;
pub const STYLE_DEFAULT: usize = 32;
pub const INDIC_SQUIGGLEPIXMAP: isize = 13;
pub const INDIC_ROUNDBOX: isize = 7;
pub const INDIC_TEXTFORE: isize = 17;
pub const SC_ORDER_CUSTOM: usize = 2;
pub const ANNOTATION_STANDARD: usize = 1;
pub const SC_TIME_FOREVER: isize = 10_000_000;
pub const SC_MOD_INSERTTEXT: i32 = 0x1;
pub const SC_MOD_DELETETEXT: i32 = 0x2;

pub const SCN_CHARADDED: u32 = 2001;
pub const SCN_UPDATEUI: u32 = 2007;
pub const SCN_MODIFIED: u32 = 2008;
pub const SCN_DWELLSTART: u32 = 2016;
pub const SCN_DWELLEND: u32 = 2017;
pub const SCN_USERLISTSELECTION: u32 = 2014;
pub const SCN_AUTOCCANCELLED: u32 = 2025;
pub const SCN_FOCUSIN: u32 = 2028;
pub const SCN_AUTOCCOMPLETED: u32 = 2030;
pub const SCN_AUTOCSELECTIONCHANGE: u32 = 2032;

// Notepad_plus_msgs.h

const WM_USER: u32 = 0x400;
const NPPMSG: u32 = WM_USER + 1000;
pub const NPPM_GETCURRENTSCINTILLA: u32 = NPPMSG + 4;
pub const NPPM_SWITCHTOFILE: u32 = NPPMSG + 37;
const NPPM_SETSTATUSBAR: u32 = NPPMSG + 24;
const STATUSBAR_DOC_TYPE: usize = 0;
const SB_GETTEXTLENGTHW: u32 = WM_USER + 12;
const SB_GETTEXTW: u32 = WM_USER + 13;
pub const NPPM_SETMENUITEMCHECK: u32 = NPPMSG + 40;
pub const NPPM_GETPLUGINSCONFIGDIR: u32 = NPPMSG + 46;
pub const NPPM_GETFULLPATHFROMBUFFERID: u32 = NPPMSG + 58;
pub const NPPM_GETCURRENTBUFFERID: u32 = NPPMSG + 60;
pub const NPPM_GETBUFFERLANGTYPE: u32 = NPPMSG + 64;
pub const NPPM_DOOPEN: u32 = NPPMSG + 77;
pub const NPPM_ALLOCATEINDICATOR: u32 = NPPMSG + 113;

/// `L_VHDL` in `enum LangType`.
pub const L_VHDL: isize = 38;

const NPPN_FIRST: u32 = 1000;
pub const NPPN_READY: u32 = NPPN_FIRST + 1;
pub const NPPN_FILEBEFORECLOSE: u32 = NPPN_FIRST + 3;
pub const NPPN_FILEBEFORESAVE: u32 = NPPN_FIRST + 7;
pub const NPPN_FILESAVED: u32 = NPPN_FIRST + 8;
pub const NPPN_SHUTDOWN: u32 = NPPN_FIRST + 9;
pub const NPPN_BUFFERACTIVATED: u32 = NPPN_FIRST + 10;
pub const NPPN_LANGCHANGED: u32 = NPPN_FIRST + 11;

/// A NUL-terminated UTF-16 string.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

pub fn send(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: every message the plugin sends takes plain values, or
    // pointers to buffers that outlive the call.
    unsafe { SendMessageW(hwnd, msg, wparam, lparam) }
}

/// The text of the first field of Notepad++'s status bar, the file type.
pub fn status_text(npp: Hwnd) -> Option<String> {
    let class = wide("msctls_statusbar32");
    // SAFETY: the class name is NUL-terminated and lives across the call.
    let bar = unsafe { FindWindowExW(npp, std::ptr::null_mut(), class.as_ptr(), std::ptr::null()) };
    if bar.is_null() {
        return None;
    }
    let length = (send(bar, SB_GETTEXTLENGTHW, STATUSBAR_DOC_TYPE, 0) & 0xFFFF) as usize;
    let mut text = vec![0u16; length + 1];
    send(
        bar,
        SB_GETTEXTW,
        STATUSBAR_DOC_TYPE,
        text.as_mut_ptr() as isize,
    );
    text.truncate(length);
    Some(String::from_utf16_lossy(&text))
}

/// Replaces the first field of Notepad++'s status bar, the file type.
pub fn set_status_text(npp: Hwnd, text: &str) {
    let text = wide(text);
    send(
        npp,
        NPPM_SETSTATUSBAR,
        STATUSBAR_DOC_TYPE,
        text.as_ptr() as isize,
    );
}

pub fn message_box(owner: Hwnd, text: &str, kind: u32) {
    let text = wide(text);
    let caption = wide("vfl");
    // SAFETY: both strings are NUL-terminated and live across the call.
    unsafe { MessageBoxW(owner, text.as_ptr(), caption.as_ptr(), kind) };
}

pub fn confirm(owner: Hwnd, text: &str) -> bool {
    let text = wide(text);
    let caption = wide("vfl");
    // SAFETY: both strings are NUL-terminated and live across the call.
    let answer = unsafe {
        MessageBoxW(
            owner,
            text.as_ptr(),
            caption.as_ptr(),
            MB_OKCANCEL | MB_ICONQUESTION,
        )
    };
    answer == IDOK
}

// A one-line input box, built in memory since the plugin has no resource
// file.

const EDIT_ID: i32 = 100;

thread_local! {
    /// The text of the open input box: the initial text, then the answer.
    static INPUT: std::cell::RefCell<Option<Vec<u16>>> = const { std::cell::RefCell::new(None) };
}

/// Asks for one line of text. `None` when cancelled.
pub fn input_box(owner: Hwnd, title: &str, prompt: &str, initial: &str) -> Option<String> {
    const WS_POPUP: u32 = 0x8000_0000;
    const WS_CHILD: u32 = 0x4000_0000;
    const WS_VISIBLE: u32 = 0x1000_0000;
    const WS_CAPTION: u32 = 0x00C0_0000;
    const WS_SYSMENU: u32 = 0x0008_0000;
    const WS_BORDER: u32 = 0x0080_0000;
    const WS_TABSTOP: u32 = 0x0001_0000;
    const DS_MODALFRAME: u32 = 0x80;
    const DS_SETFONT: u32 = 0x40;
    const DS_CENTER: u32 = 0x800;
    const ES_AUTOHSCROLL: u32 = 0x80;
    const BS_DEFPUSHBUTTON: u32 = 0x1;
    const BUTTON: u16 = 0x80;
    const EDIT: u16 = 0x81;
    const STATIC: u16 = 0x82;

    let mut t: Vec<u16> = Vec::new();
    let push_u32 = |t: &mut Vec<u16>, v: u32| {
        t.push(v as u16);
        t.push((v >> 16) as u16);
    };
    let push_str = |t: &mut Vec<u16>, s: &str| t.extend(s.encode_utf16().chain(Some(0)));
    let align = |t: &mut Vec<u16>| {
        if t.len() % 2 == 1 {
            t.push(0);
        }
    };
    push_u32(
        &mut t,
        WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_MODALFRAME | DS_SETFONT | DS_CENTER,
    );
    push_u32(&mut t, 0);
    t.push(4); // items
    t.extend([0, 0, 240, 62]); // x, y, width, height in dialog units
    t.push(0); // no menu
    t.push(0); // default class
    push_str(&mut t, title);
    t.push(9); // font size
    push_str(&mut t, "Segoe UI");
    let item = |t: &mut Vec<u16>, style: u32, rect: [u16; 4], id: i32, class: u16, text: &str| {
        align(t);
        push_u32(t, style | WS_CHILD | WS_VISIBLE);
        push_u32(t, 0);
        t.extend(rect);
        t.push(id as u16);
        t.extend([0xFFFF, class]);
        push_str(t, text);
        t.push(0); // no creation data
    };
    item(&mut t, 0, [7, 7, 226, 10], -1, STATIC, prompt);
    item(
        &mut t,
        WS_BORDER | WS_TABSTOP | ES_AUTOHSCROLL,
        [7, 19, 226, 13],
        EDIT_ID,
        EDIT,
        "",
    );
    item(
        &mut t,
        WS_TABSTOP | BS_DEFPUSHBUTTON,
        [129, 40, 50, 14],
        IDOK,
        BUTTON,
        "OK",
    );
    item(
        &mut t,
        WS_TABSTOP,
        [183, 40, 50, 14],
        IDCANCEL,
        BUTTON,
        "Cancel",
    );
    // DWORD-aligned, as Windows requires.
    let words: Vec<u32> = t
        .chunks(2)
        .map(|c| c[0] as u32 | (c.get(1).copied().unwrap_or(0) as u32) << 16)
        .collect();

    INPUT.with(|i| *i.borrow_mut() = Some(initial.encode_utf16().chain(Some(0)).collect()));
    // SAFETY: the template is well formed and outlives the modal call;
    // the dialog procedure only touches its own controls.
    let result = unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(std::ptr::null()),
            words.as_ptr().cast(),
            owner,
            Some(input_proc),
            0,
        )
    };
    let text = INPUT.with(|i| i.borrow_mut().take());
    if result != IDOK as isize {
        return None;
    }
    let text = text?;
    let len = text.iter().position(|&c| c == 0).unwrap_or(text.len());
    Some(String::from_utf16_lossy(&text[..len]))
}

unsafe extern "system" fn input_proc(
    dialog: Hwnd,
    msg: u32,
    wparam: usize,
    _lparam: isize,
) -> isize {
    const WM_INITDIALOG: u32 = 0x110;
    const WM_COMMAND: u32 = 0x111;
    const EM_SETSEL: u32 = 0xB1;
    match msg {
        WM_INITDIALOG => {
            INPUT.with(|i| {
                if let Some(text) = i.borrow().as_ref() {
                    // SAFETY: the text is NUL-terminated.
                    unsafe { SetDlgItemTextW(dialog, EDIT_ID, text.as_ptr()) };
                }
            });
            // SAFETY: plain calls on the dialog's own edit control.
            unsafe {
                let edit = GetDlgItem(dialog, EDIT_ID);
                SendMessageW(edit, EM_SETSEL, 0, -1);
                SetFocus(edit);
            }
            // Focus was set here.
            0
        }
        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as i32;
            if id == IDOK {
                let mut buffer = vec![0u16; 1024];
                // SAFETY: the buffer holds `len` characters.
                unsafe { GetDlgItemTextW(dialog, EDIT_ID, buffer.as_mut_ptr(), 1024) };
                INPUT.with(|i| *i.borrow_mut() = Some(buffer));
            }
            if id == IDOK || id == IDCANCEL {
                // SAFETY: ends this modal dialog.
                unsafe { EndDialog(dialog, id as isize) };
                return 1;
            }
            0
        }
        _ => 0,
    }
}
