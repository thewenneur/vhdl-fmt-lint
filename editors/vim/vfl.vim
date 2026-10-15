" vfl language server for Vim and Neovim LSP plugins: VHDL, Verilog and
" SystemVerilog. Source this file from your vimrc after the plugin is loaded;
" it registers vfl with whichever of vim-lsp (prabirshrestha/vim-lsp) and
" yegappan/lsp is installed. For coc.nvim, see coc-settings.json next to it.
" See docs/editors.md.

if !executable('vfl')
  finish
endif

" vim-lsp
augroup vfl_vim_lsp
  autocmd!
  autocmd User lsp_setup call lsp#register_server({
        \ 'name': 'vfl',
        \ 'cmd': {server_info -> ['vfl', 'lsp']},
        \ 'root_uri': {server_info -> lsp#utils#path_to_uri(
        \     lsp#utils#find_nearest_parent_file_directory(
        \         lsp#utils#get_buffer_path(), ['vfl.toml', '.git/']))},
        \ 'allowlist': ['vhdl', 'verilog', 'systemverilog'],
        \ })
augroup END

" yegappan/lsp (Vim 9)
if exists('*LspAddServer')
  call LspAddServer([{
        \ 'name': 'vfl',
        \ 'filetype': ['vhdl', 'verilog', 'systemverilog'],
        \ 'path': exepath('vfl'),
        \ 'args': ['lsp'],
        \ 'rootSearch': ['vfl.toml', '.git/'],
        \ }])
endif
