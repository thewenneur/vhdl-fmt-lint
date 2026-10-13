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
}

pub const MB_OK: u32 = 0x0;
pub const MB_ICONWARNING: u32 = 0x30;
pub const MB_ICONINFORMATION: u32 = 0x40;

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

/// The start of `SCNotification`; the plugin reads no further fields.
#[repr(C)]
pub struct ScNotification {
    pub nmhdr: NotifyHeader,
    pub position: isize,
    pub ch: i32,
    pub modifiers: i32,
    pub modification_type: i32,
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
pub const SCI_LINEFROMPOSITION: u32 = 2166;
pub const SCI_REPLACETARGET: u32 = 2194;
pub const SCI_CALLTIPSHOW: u32 = 2200;
pub const SCI_CALLTIPCANCEL: u32 = 2201;
pub const SCI_CALLTIPACTIVE: u32 = 2202;
pub const SCI_CALLTIPSETHLT: u32 = 2204;
pub const SCI_ENSUREVISIBLE: u32 = 2232;
pub const SCI_SETMOUSEDWELLTIME: u32 = 2264;
pub const SCI_GETMOUSEDWELLTIME: u32 = 2265;
pub const SCI_AUTOCGETTYPESEPARATOR: u32 = 2285;
pub const SCI_AUTOCSETTYPESEPARATOR: u32 = 2286;
pub const SCI_FINDCOLUMN: u32 = 2456;
pub const SCI_STYLEGETBACK: u32 = 2482;
pub const SCI_STYLEGETSIZE: u32 = 2485;
pub const SCI_STYLEGETFONT: u32 = 2486;
pub const SCI_SETINDICATORCURRENT: u32 = 2500;
pub const SCI_INDICATORFILLRANGE: u32 = 2504;
pub const SCI_INDICATORCLEARRANGE: u32 = 2505;
pub const SCI_GETCHARACTERPOINTER: u32 = 2520;
pub const SCI_ANNOTATIONSETTEXT: u32 = 2540;
pub const SCI_ANNOTATIONSETSTYLE: u32 = 2542;
pub const SCI_ANNOTATIONCLEARALL: u32 = 2547;
pub const SCI_ANNOTATIONSETVISIBLE: u32 = 2548;
pub const SCI_ANNOTATIONSETSTYLEOFFSET: u32 = 2550;
pub const SCI_ALLOCATEEXTENDEDSTYLES: u32 = 2553;
pub const SCI_SETFIRSTVISIBLELINE: u32 = 2613;
pub const SCI_SETTARGETRANGE: u32 = 2686;

pub const SC_CP_UTF8: isize = 65001;
pub const STYLE_DEFAULT: usize = 32;
pub const INDIC_SQUIGGLEPIXMAP: isize = 13;
pub const ANNOTATION_STANDARD: usize = 1;
pub const SC_TIME_FOREVER: isize = 10_000_000;
pub const SC_MOD_INSERTTEXT: i32 = 0x1;
pub const SC_MOD_DELETETEXT: i32 = 0x2;

pub const SCN_CHARADDED: u32 = 2001;
pub const SCN_UPDATEUI: u32 = 2007;
pub const SCN_MODIFIED: u32 = 2008;
pub const SCN_DWELLSTART: u32 = 2016;
pub const SCN_DWELLEND: u32 = 2017;

// Notepad_plus_msgs.h

const WM_USER: u32 = 0x400;
const NPPMSG: u32 = WM_USER + 1000;
pub const NPPM_GETCURRENTSCINTILLA: u32 = NPPMSG + 4;
pub const NPPM_SETMENUITEMCHECK: u32 = NPPMSG + 40;
pub const NPPM_GETPLUGINSCONFIGDIR: u32 = NPPMSG + 46;
pub const NPPM_GETFULLPATHFROMBUFFERID: u32 = NPPMSG + 58;
pub const NPPM_GETCURRENTBUFFERID: u32 = NPPMSG + 60;
pub const NPPM_GETBUFFERLANGTYPE: u32 = NPPMSG + 64;
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

pub fn message_box(owner: Hwnd, text: &str, kind: u32) {
    let text = wide(text);
    let caption = wide("vfl");
    // SAFETY: both strings are NUL-terminated and live across the call.
    unsafe { MessageBoxW(owner, text.as_ptr(), caption.as_ptr(), kind) };
}
