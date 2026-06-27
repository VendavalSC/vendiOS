-- Resolve the active vendiOS palette.
--
-- `vendi theme <name>` writes ~/.config/vendi/nvim.lua — a plain Lua table of
-- hex colors keyed exactly like the desktop's semantic slots (base/mantle/crust,
-- text tiers, surfaces, ANSI-ish colors) plus `accent`. If that file is missing
-- (vendiVim installed before any theme switch), fall back to Catppuccin Mocha so
-- nvim still looks like vendiOS out of the box.

local mocha = {
  flavor = "mocha",
  dark = true,
  accent = "#cba6f7",
  base = "#1e1e2e",
  mantle = "#181825",
  crust = "#11111b",
  text = "#cdd6f4",
  subtext1 = "#bac2de",
  subtext0 = "#a6adc8",
  overlay1 = "#7f849c",
  surface2 = "#585b70",
  surface1 = "#45475a",
  surface0 = "#313244",
  blue = "#89b4fa",
  teal = "#94e2d5",
  green = "#a6e3a1",
  yellow = "#f9e2af",
  red = "#f38ba8",
  pink = "#f5c2e7",
}

local function load()
  local path = vim.fn.expand("~/.config/vendi/nvim.lua")
  local ok, chunk = pcall(loadfile, path)
  if ok and chunk then
    local ok2, tbl = pcall(chunk)
    if ok2 and type(tbl) == "table" and tbl.base and tbl.accent then
      -- Fill any missing slot from mocha so the colorscheme never indexes nil.
      return setmetatable(tbl, { __index = mocha })
    end
  end
  return mocha
end

return load()
