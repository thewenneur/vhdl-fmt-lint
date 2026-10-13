//! A Notepad++ plugin that formats and lints VHDL with vfl.
//!
//! The formatter and linter run inside the plugin, so no `vfl` executable
//! is needed. [`engine`] does the work and is portable; the Notepad++
//! glue only builds on Windows.

pub mod engine;

#[cfg(windows)]
mod plugin;
#[cfg(windows)]
mod win;
