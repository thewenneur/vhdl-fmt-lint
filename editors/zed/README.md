# vfl for Zed

A Zed extension for VHDL, Verilog and SystemVerilog: syntax highlighting, outline,
brackets and indentation from Tree-sitter grammars, and everything else from the vfl
language server (`vfl lsp`): diagnostics as you type, quick fixes, formatting (on save,
of a selection, and on type), hover with types, signature help, completion, go to
definition, implementation and type definition, references, rename, inlay hints and
semantic highlighting. Quartus settings (`.qsf`, `.qip`) and SDC / XDC constraints
(`.sdc`, `.xdc`) are highlighted with the Tcl grammar and linted with the
[`vendor/*` rules](../../docs/lints.md#vendor). With the registry's Tcl extension
installed too, both use the grammar name `tcl`; uninstall it if Tcl files look wrong.

It is not in the Zed extension registry yet, so it installs as a dev extension, which Zed
builds itself: [rustup](https://rustup.rs) must be installed.

1. Download `vfl-zed-vX.Y.Z.zip` from the
   [latest release](https://github.com/thewenneur/vhdl-fmt-lint/releases/latest) and unzip
   it somewhere it can stay (Zed builds and loads it from there), giving a `vfl-zed`
   directory. From a clone of this repository, use `editors/zed` instead.
2. In Zed, run **zed: install dev extension** from the command palette (or
   **Extensions > Install Dev Extension**).
3. Pick that `vfl-zed` (or `editors/zed`) directory.

The extension starts, in order:

1. the binary in the `lsp.vfl.binary.path` setting, if set;
2. `vfl` on the `PATH`;
3. otherwise, the `vfl` of the
   [GitHub release](https://github.com/thewenneur/vhdl-fmt-lint/releases) of the
   extension's own version, for the platform (Linux, macOS and Windows on x86-64 and ARM).
   It downloads it once, checks the archive against the release's `SHA256SUMS`, and keeps
   only the binary in the extension's directory. A new version of the extension downloads
   its own version, so the server and the extension always match.

```json
{
  "lsp": {
    "vfl": {
      "binary": { "path": "/path/to/vfl", "arguments": ["lsp"] }
    }
  }
}
```

In a project without a `vfl.toml`, the lint preset (`none`, `recommended`, `strict`) and
the format style (`default`, `leading-comma`, `compact`, `classic`) come from the settings,
as `vfl.toml` keys under `config` (any other `vfl.toml` key works too):

```json
{
  "lsp": {
    "vfl": {
      "initialization_options": {
        "config": { "lint": { "preset": "strict" }, "format": { "style": "compact" } }
      }
    }
  }
}
```

A project's `vfl.toml` replaces them, so that it is linted and formatted the same for
everyone. To write one from the settings in effect, run the code action **Generate project
config (vfl.toml) from the settings in effect** (offered while the project has no
`vfl.toml`).

**Uninstall the "VHDL" and "Verilog" extensions from the Zed registry first.** They define
the same languages (`VHDL`, `Verilog`, `SystemVerilog`) and the same grammar names (`vhdl`,
`systemverilog`) as this extension, and Zed then mixes one extension's grammar with the
other's queries: VHDL files open as plain text, and the log (**zed: open log**) shows
`failed to load language VHDL: Error loading highlights query ... Invalid node type
"type_mark"`. This extension replaces both: it brings the languages, and vfl is their
language server.

## Keyboard shortcuts

Zed extensions cannot add commands of their own, so vfl's actions are bound in two ways.

What the language server does is one of Zed's own actions, bound in `keymap.json`
(**zed: open keymap**):

| Action | Zed action |
|---|---|
| Format document | `editor::Format` |
| Format selection | `editor::FormatSelections` |
| Quick fixes, generate testbench (on an entity) | `editor::ToggleCodeActions` |
| Generate project config (while there is no `vfl.toml`) | `editor::ToggleCodeActions` |
| Next / previous problem | `editor::GoToDiagnostic`, `editor::GoToPreviousDiagnostic` |
| Problems of the project | `diagnostics::Deploy` |
| Go to definition | `editor::GoToDefinition` |
| Go back | `pane::GoBack` |
| Find references | `editor::FindAllReferences` |
| Rename | `editor::Rename` |
| Go to symbol in file | `outline::Toggle` |
| Go to symbol in project | `project_symbols::Toggle` |
| Show type | `editor::Hover` |
| Show parameters | `editor::ShowSignatureHelp` |
| Complete | `editor::ShowCompletions` |
| Restart the server | `editor::RestartLanguageServer` |

To apply every safe fix when formatting, add to `settings.json`:

```json
{
  "languages": {
    "VHDL": { "code_actions_on_format": { "source.fixAll.vfl": true } }
  }
}
```

The rest runs `vfl` as a [task](https://zed.dev/docs/tasks). Tasks run `vfl` from the `PATH`,
not the binary the extension downloads, so install it (see the
[main README](../../README.md#installation)) to use them. They run from the root of the
worktree (`$ZED_WORKTREE_ROOT`), so that the project ones cover the whole project.
[`tasks.json`](tasks.json) has tasks to lint or fix the file, lint the project, generate the documentation or the
specification, generate a testbench and print an instantiation or a component declaration. The last three use the
selected text as the entity name, so select it first; Zed hides tasks whose variables are
missing. Copy it to `~/.config/zed/tasks.json` (**zed: open tasks**), or to `.zed/tasks.json`
in a project, and run the tasks with **task: spawn**. [`keymap.json`](keymap.json) binds them
to `ctrl-alt-v` followed by a letter (`l` lint file, `x` fix file, `p` lint project, `d`
documentation, `s` specification, `t` testbench, `r` restart the server) in VHDL and Verilog files; none of
these is taken in Zed's default keymaps. Merge its section into your `keymap.json`. A task
is bound to any key with:

```json
{ "ctrl-alt-v l": ["task::Spawn", { "task_name": "vfl: lint file" }] }
```

## Snippets

[`snippets/vhdl.json`](snippets/vhdl.json) has VHDL snippets: `entity` (an entity and its
architecture), `architecture`, `process-sync` and `process-async` (clocked processes with a
synchronous or asynchronous reset), `process-clocked`, `process-comb`, `fsm` (an
architecture with a state machine), `fsm-process`, `testbench`, `for-generate`,
`if-generate`, `package` (with its body) and `record`. Copy it to
`~/.config/zed/snippets/vhdl.json` (**snippets: configure snippets** opens that directory).
It is the VS Code extension's file without its choices and file-name variables, which
become plain placeholders.

## Grammars

The grammars are [tree-sitter-vhdl](https://github.com/jpt13653903/tree-sitter-vhdl) for VHDL and
[tree-sitter-systemverilog](https://github.com/gmlarumbe/tree-sitter-systemverilog) for
Verilog and SystemVerilog, both under the MIT License; the highlight queries are adapted
from theirs.

`./check.sh` builds the extension and checks every query against the pinned grammars;
CI runs it. See [docs/editors.md](../../docs/editors.md) for the other editors.
