-- Loaded automatically by LazyVim after lazy. Extra autocmds on top of
-- LazyVim's defaults.
local augroup = vim.api.nvim_create_augroup("vendivim", { clear = true })

-- Live-follow `vendi theme`: when the generated palette file changes on disk,
-- re-apply the vendi colorscheme so running editors recolor too (no relaunch).
local palette = vim.fn.expand("~/.config/vendi/nvim.lua")
local w = (vim.uv or vim.loop).new_fs_event()
if w then
  local function watch()
    w:stop()
    if (vim.uv or vim.loop).fs_stat(palette) then
      w:start(palette, {}, function()
        vim.schedule(function()
          package.loaded["vendi.palette"] = nil
          pcall(vim.cmd.colorscheme, "vendi")
        end)
        -- editors replace files (write-to-temp + rename), which drops the
        -- watch — re-arm it on the next tick.
        vim.defer_fn(watch, 50)
      end)
    end
  end
  watch()
  vim.api.nvim_create_autocmd("VimLeavePre", { group = augroup, callback = function() pcall(function() w:stop() end) end })
end
