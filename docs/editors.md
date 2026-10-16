# Editor setup

vfl works in any editor with a Language Server Protocol client: `vfl lsp` is the language
server, over standard input and output, for VHDL, Verilog and SystemVerilog files. What it
offers is listed in the [command-line reference](cli.md#vfl-lsp): diagnostics as you type,
quick fixes, formatting (document, range and on type), hover, signature help, completion,
go to definition, implementation, type definition and declaration, references, rename,
symbols, inlay hints, linked editing of `end` names, selection ranges, instance-count code
lenses, call hierarchy and semantic highlighting.

Install `vfl` first ([README](../README.md#installation)) and check that `vfl --version`
runs from a terminal. If the editor does not see it, use its full path in the commands below
(`~/.cargo/bin/vfl` after `cargo install`).

Every configuration below:

- starts `vfl lsp`, with no options or initialization settings: the server reads its
  settings from `vfl.toml` ([configuration reference](configuration.md)), or from the
  [editor settings](#editor-settings) in a project without one;
- uses it for `.vhd`, `.vhdl`, `.vho` and `.vht` files (language id `vhdl`), `.v` and
  `.vh` (`verilog`), and `.sv` and `.svh` (`systemverilog`); the server tells the
  languages apart by the file extension, so a language id that differs is harmless;
- takes the directory with `vfl.toml` (or else the repository) as the workspace root.
  The server finds the project root itself either way.

VHDL files that instantiate Verilog modules get their ports checked, hover and go to
definition with the default settings. Diagnostics, formatting, symbols and highlighting
*in* Verilog and SystemVerilog files need `[verilog]` `mode = "full"` in `vfl.toml`.

The files named below are in [`editors/`](../editors). The Neovim, Emacs, Vim and Helix
ones are tested in CI against real editors ([`editors/test`](../editors/test/run.sh)), and
the Zed extension is built and its queries checked against its grammars.

| Editor | How | Files |
|--------|-----|-------|
| [VS Code](#vs-code) | Extension | [`editors/vscode`](../editors/vscode/README.md) |
| [Notepad++](#notepad) | Plugin (no `vfl` needed) | [`editors/notepadpp`](../editors/notepadpp/README.md) |
| [Zed](#zed) | Extension | [`editors/zed`](../editors/zed/README.md) |
| [Neovim](#neovim) | Built-in LSP client | [`editors/neovim/lsp/vfl.lua`](../editors/neovim/lsp/vfl.lua) |
| [Vim](#vim) | vim-lsp, yegappan/lsp or coc.nvim | [`editors/vim`](../editors/vim) |
| [Emacs](#emacs) | Eglot or lsp-mode | [`editors/emacs/vfl.el`](../editors/emacs/vfl.el) |
| [Helix](#helix) | Built-in LSP client | [`editors/helix/languages.toml`](../editors/helix/languages.toml) |
| [Sublime Text](#sublime-text) | LSP package | [`editors/sublime`](../editors/sublime/LSP.sublime-settings) |
| [Kate](#kate) | Built-in LSP client | [`editors/kate/lspclient.json`](../editors/kate/lspclient.json) |
| [JetBrains IDEs](#jetbrains-ides) | LSP4IJ plugin | [`editors/jetbrains/vfl`](../editors/jetbrains/vfl/template.json) |
| [Others](#other-editors) | Any LSP client | |

## VS Code

Install the extension from the `.vsix` of the latest release: see the
[README](../README.md#vs-code) and the [extension's README](../editors/vscode/README.md)
for its commands and settings.

Every command can be given a key in **Preferences: Open Keyboard Shortcuts** (search for
`vfl`); see [Keyboard shortcuts](../editors/vscode/README.md#keyboard-shortcuts).

The **VHDL Hierarchy** view in the Explorer shows the instantiation tree of the project:
see [Design hierarchy](#design-hierarchy).

## Notepad++

The plugin has the formatter, the linter and the editor features built in, without
`vfl lsp`: see the [README](../README.md#notepad) and the
[plugin's README](../editors/notepadpp/README.md).

Every menu command, the settings included, can be given a key in **Settings > Shortcut
Mapper > Plugin commands** (filter on `vfl`); see
[Commands](../editors/notepadpp/README.md#commands).

**Show design hierarchy** lists the instantiation tree of the project: see
[Design hierarchy](#design-hierarchy).

## Zed

The [Zed extension](../editors/zed/README.md) adds the VHDL, Verilog and SystemVerilog
languages (highlighting, outline, brackets and indentation from Tree-sitter grammars) and
starts `vfl lsp` for them. It is not in the Zed extension registry yet; download
`vfl-zed-vX.Y.Z.zip` from the
[latest release](https://github.com/thewenneur/vhdl-fmt-lint/releases/latest), unzip it
somewhere it can stay, and run **zed: install dev extension**, picking the `vfl-zed`
directory (or `editors/zed` in a clone). Zed builds it with [rustup](https://rustup.rs)'s
Rust.

Uninstall the "VHDL" and "Verilog" extensions of the Zed registry first: they define the same
languages and grammar names, and with both installed Zed mixes their grammars and queries, so
VHDL files open as plain text. The vfl extension replaces them.

It uses `vfl` from the `PATH`, or downloads the binary of the release of its own version
(checked against the release's `SHA256SUMS`) when there is none.
To use another binary, set it in Zed's `settings.json`:

```json
{
  "lsp": {
    "vfl": { "binary": { "path": "/path/to/vfl", "arguments": ["lsp"] } }
  }
}
```

Zed formats on save with the language server by default. The lint preset and format style of
projects without a `vfl.toml` go in `lsp.vfl.initialization_options` (see
[Editor settings](#editor-settings)).

Zed extensions cannot add commands, so the language server's features are bound through
Zed's own actions (`editor::Format`, `editor::ToggleCodeActions`, `editor::Rename`, ...),
and the rest (lint the project, documentation, testbench) through the example
[tasks](../editors/zed/tasks.json) and [keymap](../editors/zed/keymap.json); see
[Keyboard shortcuts](../editors/zed/README.md#keyboard-shortcuts).

## Neovim

Neovim 0.11 or later. Copy [`editors/neovim/lsp/vfl.lua`](../editors/neovim/lsp/vfl.lua)
to `~/.config/nvim/lsp/vfl.lua` and add to `init.lua`:

```lua
vim.lsp.enable("vfl")
```

The file is the whole configuration, so it can also be written inline:

```lua
vim.lsp.config("vfl", {
  cmd = { "vfl", "lsp" },
  filetypes = { "vhdl", "verilog", "systemverilog" },
  root_markers = { "vfl.toml", ".git" },
})
vim.lsp.enable("vfl")
```

Neovim detects the file types itself, except for `.vht` and `.vh` files (and `.v` files
it cannot tell from V or Coq ones). To add them:

```lua
vim.filetype.add({ extension = { vht = "vhdl", vh = "verilog", v = "verilog" } })
```

Format with `vim.lsp.buf.format()`; to format on save:

```lua
vim.api.nvim_create_autocmd("BufWritePre", {
  pattern = { "*.vhd", "*.vhdl", "*.v", "*.sv", "*.svh" },
  callback = function() vim.lsp.buf.format({ name = "vfl" }) end,
})
```

With Neovim 0.10 and [nvim-lspconfig](https://github.com/neovim/nvim-lspconfig), which
has no `vfl` entry, register it first:

```lua
local configs = require("lspconfig.configs")
configs.vfl = {
  default_config = {
    cmd = { "vfl", "lsp" },
    filetypes = { "vhdl", "verilog", "systemverilog" },
    root_dir = require("lspconfig.util").root_pattern("vfl.toml", ".git"),
  },
}
require("lspconfig").vfl.setup({})
```

## Vim

[`editors/vim/vfl.vim`](../editors/vim/vfl.vim) registers vfl with
[vim-lsp](https://github.com/prabirshrestha/vim-lsp) and with
[yegappan/lsp](https://github.com/yegappan/lsp) (Vim 9), whichever is installed. Source
it from your `vimrc` after the plugins are loaded:

```vim
source /path/to/vhdl-fmt-lint/editors/vim/vfl.vim
```

For [coc.nvim](https://github.com/neoclide/coc.nvim), merge
[`editors/vim/coc-settings.json`](../editors/vim/coc-settings.json) into your
`coc-settings.json` (`:CocConfig`):

```json
{
  "languageserver": {
    "vfl": {
      "command": "vfl",
      "args": ["lsp"],
      "filetypes": ["vhdl", "verilog", "systemverilog"],
      "rootPatterns": ["vfl.toml", ".git"]
    }
  }
}
```

## Emacs

Emacs 29 or later. [`editors/emacs/vfl.el`](../editors/emacs/vfl.el) registers `vfl lsp`
with [Eglot](https://www.gnu.org/software/emacs/manual/html_node/eglot/) (built in) and
with [lsp-mode](https://emacs-lsp.github.io/lsp-mode/), for whichever is loaded, in
`vhdl-mode`, `verilog-mode` (which Emacs also uses for `.sv` files) and their tree-sitter
variants:

```elisp
(add-to-list 'load-path "/path/to/vhdl-fmt-lint/editors/emacs")
(require 'vfl)
(add-hook 'vhdl-mode-hook #'eglot-ensure)      ; or #'lsp-deferred for lsp-mode
(add-hook 'verilog-mode-hook #'eglot-ensure)
```

`vfl-server-command` (`M-x customize-group RET vfl`) changes the command. For Eglot alone,
without the file:

```elisp
(with-eval-after-load 'eglot
  (add-to-list 'eglot-server-programs
               '((vhdl-mode verilog-mode) . ("vfl" "lsp"))))
```

Format with `eglot-format-buffer` (or `lsp-format-buffer`). For lsp-mode, `vfl.el` gives
vfl a higher priority than lsp-mode's own VHDL and Verilog servers.

## Helix

Copy [`editors/helix/languages.toml`](../editors/helix/languages.toml) into
`~/.config/helix/languages.toml` (or a project's `.helix/languages.toml`):

```toml
[language-server.vfl]
command = "vfl"
args = ["lsp"]

[[language]]
name = "vhdl"
language-servers = ["vfl"]
roots = ["vfl.toml"]

[[language]]
name = "verilog"   # .v, .vh, .sv and .svh
language-servers = ["vfl"]
roots = ["vfl.toml"]
```

`hx --health vhdl` shows whether Helix finds `vfl`. `:format` formats with the server; add
`auto-format = true` to a `[[language]]` section to format on save.

## Sublime Text

Install the [LSP](https://packagecontrol.io/packages/LSP) package, and syntax packages that
define the `source.vhdl` and `source.systemverilog` (or `source.verilog`) scopes, such as
[VHDL](https://packagecontrol.io/packages/VHDL) and
[SystemVerilog](https://packagecontrol.io/packages/SystemVerilog). Then add the client in
**Preferences > Package Settings > LSP > Settings**
([`editors/sublime/LSP.sublime-settings`](../editors/sublime/LSP.sublime-settings)):

```json
{
  "clients": {
    "vfl": {
      "enabled": true,
      "command": ["vfl", "lsp"],
      "selector": "source.vhdl | source.verilog | source.systemverilog"
    }
  }
}
```

Set `"lsp_format_on_save": true` in the same file to format on save.

## Kate

Kate's LSP client already knows VHDL, with another server. In **Settings > Configure Kate >
LSP Client > User Server Settings**, paste
[`editors/kate/lspclient.json`](../editors/kate/lspclient.json), which replaces it with vfl
and adds Verilog and SystemVerilog:

```json
{
  "servers": {
    "vhdl": {
      "command": ["vfl", "lsp"],
      "rootIndicationFileNames": ["vfl.toml", ".git"],
      "highlightingModeRegex": "^VHDL$"
    },
    "verilog": {
      "command": ["vfl", "lsp"],
      "rootIndicationFileNames": ["vfl.toml", ".git"],
      "highlightingModeRegex": "^(Verilog|SystemVerilog)$"
    }
  }
}
```

## JetBrains IDEs

IntelliJ IDEA, CLion, PyCharm and the other JetBrains IDEs run language servers through
the [LSP4IJ](https://plugins.jetbrains.com/plugin/23257-lsp4ij) plugin. After installing it:

1. Open **View > Tool Windows > Language Servers**, and click **+** (New Language Server).
2. In **Template**, choose **Import from custom template...** and pick the
   [`editors/jetbrains/vfl`](../editors/jetbrains/vfl/template.json) directory of a clone.
   This fills in the command (`vfl lsp`) and the mappings of `*.vhd`, `*.vhdl`, `*.vho`,
   `*.vht`, `*.v`, `*.vh`, `*.sv` and `*.svh` files.
3. Click **OK**.

Without the template: name it `vfl`, set the command to `vfl lsp`, and in **Mappings >
File name patterns** add the patterns above with the language ids `vhdl`, `verilog` and
`systemverilog`.

## Other editors

Any other editor with an LSP client works the same way: run `vfl lsp` over stdio for the
file types above, with no initialization options needed. The server announces what it supports
in its `initialize` response, including its semantic token types and its on-type
formatting triggers (`;` and newline).

## Design hierarchy

The language server answers a custom `vfl/hierarchy` request with the instantiation tree
of the project. It is built from the analysis the server already has, only when an
editor asks for it, never on an edit.

| Editor | Where |
|---|---|
| VS Code | **VHDL Hierarchy** view in the Explorer |
| Notepad++ | **Plugins > vfl > Show design hierarchy**, a list indented by depth |
| Zed | Not available: Zed extensions cannot add tree views |
| Others | Any client that can send a custom request, see below |

The tree starts from every entity no other entity instantiates, or from a top you choose.
Under each entity are the instances of its architecture: the one the instantiation names
(`entity work.fifo(rtl)`), else the entity's last architecture, as VHDL binds the most
recently analyzed. A component instance binds to the entity of the same name (in the
instantiating unit's library first), a configuration instance to its entity and the
architecture its block configuration names. Generate statements and blocks group the
instances inside them. Instances of nothing in the project, and components no entity
binds to, are flagged. An entity that instantiates itself further up is not expanded
again, and the tree stops at 64 levels and 20000 nodes.

In VS Code, the title bar of the view picks the top (**Select Hierarchy Top...**: a
quick pick of the entities, the tops first) and refreshes it. The top is kept in the
workspace's `vfl.hierarchy.top` setting (`name` or `library.name`; empty for every top),
and `vfl.hierarchy.testbenches` (on by default) says whether entities without ports
start trees when no top is set; off, what they test does. Clicking an instance opens it;
its context menu has **Open Entity**, **Open Architecture**, **Set as Top** and
**Generate Testbench**. The view is refreshed after a VHDL or Verilog file is saved and
after the server has analyzed an edit (once its diagnostics change), debounced, and only
while it is visible.

Notepad++ shows the tree as a list rather than a docked panel: the plugin's other lists
(problems, symbols, references) are Scintilla lists, which open at the caret and filter as
you type, and a docked tree would need its own window, dialog resources and tree control
for a view that is read once and left. Choosing a line goes to the instance (a top's
entity). Testbenches are tops there too.

### The `vfl/hierarchy` request

Parameters, all optional: `top` (`name` or `library.name`) and `testbenches` (`true` by
default). The answer:

```text
{
  "roots": [
    {
      "kind": "entity", "label": "tb_uart", "name": "tb_uart", "unitKind": "entity",
      "library": "work", "architecture": "sim", "testbench": true,
      "location": { "uri": "file:///.../tb_uart.vhd", "range": { ... } },
      "entityLocation": { ... }, "architectureLocation": { ... },
      "children": [
        { "kind": "instance", "label": "dut", "name": "uart", "unitKind": "entity", ... }
      ]
    }
  ],
  "entities": [
    { "library": "work", "name": "uart", "path": "/.../uart.vhd", "top": false, "testbench": false }
  ]
}
```

`kind` is `entity` (a top), `instance`, `generate` or `block`; a generate statement has
its scheme in `detail` (`for i in 0 to 3`). `unitKind` is `entity`, `component`,
`configuration` or `module` (Verilog). `location` is the instance's label (a top's entity
name), `entityLocation` and `architectureLocation` the declarations it binds to.
`unresolved`, `recursive` and `truncated` are set when true. `entities` lists every VHDL
entity of the project with whether it is a top, to choose one from; `message` says why
`roots` is empty, such as a `top` that is not in the project.

## Editor settings

In a project without a `vfl.toml`, the server uses the settings the editor gives it: any
`vfl.toml` keys, as a JSON object under `config`, in the initialization options or later in
a `workspace/didChangeConfiguration` notification (which applies them at once). The lint
preset and the format style are the usual ones:

```json
{ "config": { "lint": { "preset": "strict" }, "format": { "style": "compact" } } }
```

Inlay hints are a matter of taste, so the editor can also turn each kind on or off for its
user, whatever `[lsp.inlay_hints]` in `vfl.toml` says, under `inlayHints` (outside
`config`); the kinds it leaves out follow `vfl.toml`:

```json
{ "inlayHints": { "parameters": false, "port_types": true } }
```

A `vfl.toml` replaces the `config` settings entirely, so that a project is linted and
formatted the same for everyone. The code action **Generate project config (vfl.toml) from the settings in effect**
(kind `source.vfl.generateConfig`, offered while the project has no `vfl.toml`) writes them
to `vfl.toml` at the project root, with the keys at their default value as comments.

| Editor | Lint preset and format style | Generate the config file |
|--------|------------------------------|--------------------------|
| VS Code | `vfl.lint.preset`, `vfl.format.style` | **vfl: Generate Project Config (vfl.toml)** |
| Notepad++ | **Lint preset without vfl.toml...**, **Format style without vfl.toml...** | **Generate project config (vfl.toml)** |
| Zed | `lsp.vfl.initialization_options`, as above ([README](../editors/zed/README.md)) | The code action |
| Neovim | `init_options` in `vim.lsp.config("vfl", { ... })` | The code action |
| Others | The client's initialization options | The code action |
