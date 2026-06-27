-- Loaded automatically by LazyVim after lazy. Extra keymaps on top of
-- LazyVim's. (See LazyVim docs for the full default set.)
local map = vim.keymap.set

-- Reload the vendiOS theme without restarting nvim (after `vendi theme <name>`).
map("n", "<leader>uV", function()
  vim.cmd.colorscheme("vendi")
  vim.notify("vendiVim: reloaded vendi theme", vim.log.levels.INFO)
end, { desc = "Reload vendiOS theme" })

-- Quick write — mirrors the muscle memory from most setups.
map({ "n", "x" }, "<C-s>", "<cmd>w<cr><esc>", { desc = "Save file" })

-- Center the view after big jumps / search.
map("n", "<C-d>", "<C-d>zz", { desc = "Half page down (centered)" })
map("n", "<C-u>", "<C-u>zz", { desc = "Half page up (centered)" })
map("n", "n", "nzzzv", { desc = "Next match (centered)" })
map("n", "N", "Nzzzv", { desc = "Prev match (centered)" })
