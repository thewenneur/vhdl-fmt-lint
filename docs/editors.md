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

## Notepad++

The plugin has the formatter, the linter and the editor features built in, without
`vfl lsp`: see the [README](../README.md#notepad) and the
[plugin's README](../editors/notepadpp/README.md).

Every menu command, the settings included, can be given a key in **Settings > Shortcut
Mapper > Plugin commands** (filter on `vfl`); see
[Commands](../editors/notepadpp/README.md#commands).

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
