# vfl for Notepad++

A Notepad++ plugin that formats and lints VHDL with vfl, as you type.

![Problems shown under each line in Notepad++](screenshot.png)

The formatter and linter are built into the plugin, so there is nothing else to install: no
`vfl` executable on the `PATH` and no LSP client plugin. The plugin finds the project around
each file like `vfl lint` does, so names resolve across files, and it reads the same
`vfl.toml` settings.

## Features

- **Lint as you type**: problems are underlined and their messages shown under the line,
  a moment after you stop typing, and when a VHDL file is opened, switched to or saved.
  Unsaved edits in the file count; other files are read from disk.
- **Hover** over an underline to see its message.
- **Format document**, as one undo step that keeps the caret and scroll position.
  Optionally on every save.
- **Next problem** / **Previous problem** jump between problems.

## Commands

All under **Plugins > vfl**. To give them keyboard shortcuts, use **Settings > Shortcut
Mapper > Plugin commands**.

| Command | |
|---|---|
| Format document | Formats the current file with its `vfl.toml` settings. Files with syntax errors are not formatted; the caret goes to the first error. |
| Lint document | Lints the current file, whatever its language, and goes to the first problem. |
| Next problem, Previous problem | Moves to the next or previous problem and shows its message. |
| Clear problems | Removes the underlines and messages from the current file. |
| Format on save | Formats VHDL files when they are saved (off by default). With **Save All**, only the file in front is formatted. |
| Lint automatically | Lints VHDL files as described above (on by default). |
| Show messages under lines | Shows each message under its line (on by default); with it off, hover or use Next problem. |
| Reload project | Re-reads the project from disk, after files changed outside Notepad++. Saving a `vfl.toml` or `vhdl_ls.toml` in Notepad++ does this by itself. |

The three settings are kept in `plugins\Config\vfl_npp.ini`.

A file is VHDL when Notepad++ shows it as VHDL, or its extension is `.vhd`, `.vhdl`, `.vho`
or `.vht`.

## Install

Tested with Notepad++ 8.8.6, 64-bit.

1. Get `vfl_npp.dll`: from the `vfl_npp` artifact of a CI run, or build it (below).
2. Close Notepad++ and create the folder `plugins\vfl_npp` in its install directory
   (usually `C:\Program Files\Notepad++\plugins\vfl_npp`).
3. Put `vfl_npp.dll` in it and start Notepad++. **Plugins > vfl** appears.

## Build

On Windows, with Rust installed:

```sh
cargo build --release -p vfl-npp
```

The plugin is `target\release\vfl_npp.dll`. For 32-bit or ARM64 Notepad++, add
`--target i686-pc-windows-msvc` or `--target aarch64-pc-windows-msvc`.

From Linux, with `mingw-w64` installed:

```sh
rustup target add x86_64-pc-windows-gnu
cargo build --release -p vfl-npp --target x86_64-pc-windows-gnu
```

## How it works

Notepad++ has no built-in language-server support, so rather than going through a
third-party LSP client this is a native plugin. It is written in Rust and links the vfl
crates directly:

- `src/engine.rs` formats and lints a buffer. It keeps each project it has seen loaded and
  swaps in the buffer's unsaved text before analyzing. It is portable and unit-tested on
  every platform.
- `src/plugin.rs` is the Notepad++ side: the exported plugin functions, the menu, and
  showing problems with Scintilla indicators, annotations and call tips.
- `src/win.rs` declares the few Win32, Scintilla and Notepad++ definitions it uses.

Messages under lines use Scintilla annotations, which other plugins may also use; the plugin
replaces the annotations of a VHDL file each time it lints it. Turn **Show messages under
lines** off if that conflicts with another plugin.
