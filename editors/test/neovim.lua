-- Headless Neovim test of editors/neovim/lsp/vfl.lua, run by run.sh:
--   nvim --headless --clean -l neovim.lua
-- Checks that the server attaches to VHDL and SystemVerilog buffers, that
-- diagnostics arrive, and that formatting goes through the server.

local here = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":p:h")
vim.opt.runtimepath:prepend(here .. "/../neovim")
vim.lsp.enable("vfl")

local function fail(msg)
  io.stderr:write("FAIL: " .. msg .. "\n")
  os.exit(1)
end

local function open(name)
  vim.cmd.edit(here .. "/fixture/" .. name)
  local buf = vim.api.nvim_get_current_buf()
  local ok = vim.wait(20000, function()
    return #vim.lsp.get_clients({ bufnr = buf, name = "vfl" }) > 0
  end, 50)
  if not ok then
    fail("vfl did not attach to " .. name .. " (filetype " .. vim.bo[buf].filetype .. ")")
  end
  return buf
end

local vhdl = open("top.vhd")
local ok = vim.wait(20000, function()
  for _, d in ipairs(vim.diagnostic.get(vhdl)) do
    if d.message:find("unused_s", 1, true) then
      return true
    end
  end
  return false
end, 50)
if not ok then
  fail("no diagnostic for unused_s in top.vhd")
end
print("ok: VHDL diagnostics")

local sv = open("counter.sv")
local before = table.concat(vim.api.nvim_buf_get_lines(sv, 0, -1, false), "\n")
vim.lsp.buf.format({ bufnr = sv, name = "vfl", timeout_ms = 20000 })
local after = table.concat(vim.api.nvim_buf_get_lines(sv, 0, -1, false), "\n")
if before == after or not after:find("q <= q + 1;", 1, true) then
  fail("formatting counter.sv did not reformat it:\n" .. after)
end
print("ok: SystemVerilog formatting")

vim.cmd("qall!")
