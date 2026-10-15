-- vfl language server for Neovim 0.11+: VHDL, Verilog and SystemVerilog.
--
-- Copy this file to ~/.config/nvim/lsp/vfl.lua (or add editors/neovim to the
-- 'runtimepath'), then enable it in init.lua:
--
--   vim.lsp.enable("vfl")
--
-- See docs/editors.md.
return {
  cmd = { "vfl", "lsp" },
  filetypes = { "vhdl", "verilog", "systemverilog" },
  root_markers = { "vfl.toml", ".git" },
}
