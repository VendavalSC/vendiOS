-- Loaded automatically by LazyVim before lazy starts.
-- Keep this lean — LazyVim's defaults are good; only the vendiOS opinions here.
local opt = vim.opt

opt.relativenumber = true -- line jumps without counting
opt.cursorline = true
opt.scrolloff = 8 -- keep context around the cursor
opt.sidescrolloff = 8
opt.confirm = true -- ask to save instead of failing :q
opt.undofile = true -- persistent undo across sessions
opt.termguicolors = true -- truecolor, so the vendi theme renders exactly
opt.signcolumn = "yes" -- no layout shift when diagnostics appear

-- Use the system clipboard (wl-clipboard on vendiwm) for y/p.
opt.clipboard = "unnamedplus"

-- Hide the ~ markers on empty lines below the buffer. (LazyVim sets sensible
-- fold fillchars already; each field must be exactly one char, so don't blank
-- foldopen/foldclose here.)
opt.fillchars:append({ eob = " " })

-- Tabs: 2 spaces by default; language extras override per-filetype.
opt.tabstop = 2
opt.shiftwidth = 2
opt.expandtab = true
