-- vendi — a colorscheme generated from the active vendiOS desktop palette.
-- `:colorscheme vendi` reads ~/.config/vendi/nvim.lua (written by `vendi theme`)
-- so the editor always matches the rest of the desktop, including the live
-- "dynamic" wallpaper-extracted theme. Covers core UI, treesitter, LSP
-- semantic tokens, diagnostics, and the plugins LazyVim ships.

package.loaded["vendi.palette"] = nil
local c = require("vendi.palette")

local function hex(s)
  return tonumber(s:gsub("#", ""), 16)
end
-- Blend two hex colors. amt=0 → a, amt=1 → b.
local function blend(a, b, amt)
  local ai, bi = hex(a), hex(b)
  local ar, ag, ab = math.floor(ai / 65536) % 256, math.floor(ai / 256) % 256, ai % 256
  local br, bg, bb = math.floor(bi / 65536) % 256, math.floor(bi / 256) % 256, bi % 256
  local r = math.floor(ar + (br - ar) * amt + 0.5)
  local g = math.floor(ag + (bg - ag) * amt + 0.5)
  local bl = math.floor(ab + (bb - ab) * amt + 0.5)
  return string.format("#%02x%02x%02x", r, g, bl)
end

vim.cmd("highlight clear")
if vim.fn.exists("syntax_on") == 1 then
  vim.cmd("syntax reset")
end
vim.g.colors_name = "vendi"
vim.o.background = c.dark == false and "light" or "dark"

-- Derived tones: subtle accent washes for selections / floats / matches.
local accent = c.accent
local accent_bg = blend(c.base, accent, 0.18)
local sel = blend(c.base, c.surface2, 0.55)
local visual = blend(c.base, accent, 0.22)
local search = blend(c.base, c.yellow, 0.30)
local cursorline = blend(c.base, c.surface0, 0.45)
local float_bg = c.mantle
local border = c.surface1
local diff_add = blend(c.base, c.green, 0.20)
local diff_del = blend(c.base, c.red, 0.20)
local diff_chg = blend(c.base, c.blue, 0.18)
local diff_txt = blend(c.base, c.blue, 0.34)

local hl = {
  -- ── editor UI ───────────────────────────────────────────────
  Normal = { fg = c.text, bg = c.base },
  NormalNC = { fg = c.text, bg = c.base },
  NormalFloat = { fg = c.text, bg = float_bg },
  FloatBorder = { fg = border, bg = float_bg },
  FloatTitle = { fg = accent, bg = float_bg, bold = true },
  ColorColumn = { bg = c.mantle },
  Conceal = { fg = c.overlay1 },
  Cursor = { fg = c.base, bg = accent },
  CursorLine = { bg = cursorline },
  CursorColumn = { bg = cursorline },
  CursorLineNr = { fg = accent, bold = true },
  LineNr = { fg = c.surface2 },
  SignColumn = { fg = c.surface2, bg = c.base },
  FoldColumn = { fg = c.surface2, bg = c.base },
  Folded = { fg = c.subtext0, bg = c.surface0 },
  VertSplit = { fg = c.surface0 },
  WinSeparator = { fg = c.surface0 },
  MatchParen = { fg = accent, bold = true },
  NonText = { fg = c.surface1 },
  Whitespace = { fg = c.surface0 },
  EndOfBuffer = { fg = c.base },
  SpecialKey = { fg = c.surface1 },
  Directory = { fg = c.blue },
  Title = { fg = accent, bold = true },
  Visual = { bg = visual },
  VisualNOS = { bg = visual },
  Search = { fg = c.base, bg = blend(c.base, c.yellow, 0.55) },
  IncSearch = { fg = c.base, bg = accent },
  CurSearch = { fg = c.base, bg = accent },
  Substitute = { fg = c.base, bg = c.pink },
  WildMenu = { bg = c.surface0 },
  QuickFixLine = { bg = accent_bg, bold = true },
  Pmenu = { fg = c.subtext1, bg = float_bg },
  PmenuSel = { fg = c.text, bg = accent_bg, bold = true },
  PmenuSbar = { bg = c.surface0 },
  PmenuThumb = { bg = c.surface2 },
  PmenuKind = { fg = accent, bg = float_bg },
  PmenuExtra = { fg = c.overlay1, bg = float_bg },
  StatusLine = { fg = c.subtext1, bg = c.mantle },
  StatusLineNC = { fg = c.overlay1, bg = c.mantle },
  TabLine = { fg = c.overlay1, bg = c.mantle },
  TabLineFill = { bg = c.mantle },
  TabLineSel = { fg = accent, bg = c.base, bold = true },
  WinBar = { fg = c.subtext1, bg = c.base },
  WinBarNC = { fg = c.overlay1, bg = c.base },
  ErrorMsg = { fg = c.red, bold = true },
  WarningMsg = { fg = c.yellow },
  ModeMsg = { fg = c.subtext1, bold = true },
  MsgArea = { fg = c.text },
  MoreMsg = { fg = c.green },
  Question = { fg = c.green },
  helpAlignedHeading = { fg = accent, bold = true },
  Selection = { bg = sel },

  -- ── legacy syntax groups ────────────────────────────────────
  Comment = { fg = c.overlay1, italic = true },
  Constant = { fg = c.peach or c.yellow },
  String = { fg = c.green },
  Character = { fg = c.green },
  Number = { fg = c.peach or c.yellow },
  Boolean = { fg = c.peach or c.yellow },
  Float = { fg = c.peach or c.yellow },
  Identifier = { fg = c.text },
  Function = { fg = c.blue },
  Statement = { fg = c.pink },
  Conditional = { fg = c.pink },
  Repeat = { fg = c.pink },
  Label = { fg = c.pink },
  Operator = { fg = c.teal },
  Keyword = { fg = c.pink },
  Exception = { fg = c.pink },
  PreProc = { fg = c.teal },
  Include = { fg = c.pink },
  Define = { fg = c.pink },
  Macro = { fg = c.teal },
  PreCondit = { fg = c.teal },
  Type = { fg = c.yellow },
  StorageClass = { fg = c.yellow },
  Structure = { fg = c.yellow },
  Typedef = { fg = c.yellow },
  Special = { fg = c.pink },
  SpecialChar = { fg = c.pink },
  Tag = { fg = c.blue },
  Delimiter = { fg = c.overlay1 },
  SpecialComment = { fg = c.subtext0, italic = true },
  Debug = { fg = c.red },
  Underlined = { underline = true },
  Bold = { bold = true },
  Italic = { italic = true },
  Ignore = { fg = c.surface2 },
  Error = { fg = c.red },
  Todo = { fg = c.base, bg = c.yellow, bold = true },

  -- ── diagnostics ─────────────────────────────────────────────
  DiagnosticError = { fg = c.red },
  DiagnosticWarn = { fg = c.yellow },
  DiagnosticInfo = { fg = c.blue },
  DiagnosticHint = { fg = c.teal },
  DiagnosticOk = { fg = c.green },
  DiagnosticUnderlineError = { sp = c.red, undercurl = true },
  DiagnosticUnderlineWarn = { sp = c.yellow, undercurl = true },
  DiagnosticUnderlineInfo = { sp = c.blue, undercurl = true },
  DiagnosticUnderlineHint = { sp = c.teal, undercurl = true },
  DiagnosticVirtualTextError = { fg = c.red, bg = blend(c.base, c.red, 0.10) },
  DiagnosticVirtualTextWarn = { fg = c.yellow, bg = blend(c.base, c.yellow, 0.10) },
  DiagnosticVirtualTextInfo = { fg = c.blue, bg = blend(c.base, c.blue, 0.10) },
  DiagnosticVirtualTextHint = { fg = c.teal, bg = blend(c.base, c.teal, 0.10) },

  -- ── diff / spell ────────────────────────────────────────────
  DiffAdd = { bg = diff_add },
  DiffChange = { bg = diff_chg },
  DiffDelete = { bg = diff_del },
  DiffText = { bg = diff_txt },
  diffAdded = { fg = c.green },
  diffRemoved = { fg = c.red },
  diffChanged = { fg = c.blue },
  diffOldFile = { fg = c.yellow },
  diffNewFile = { fg = c.peach or c.yellow },
  diffFile = { fg = c.blue },
  diffLine = { fg = c.overlay1 },
  SpellBad = { sp = c.red, undercurl = true },
  SpellCap = { sp = c.yellow, undercurl = true },
  SpellLocal = { sp = c.blue, undercurl = true },
  SpellRare = { sp = c.teal, undercurl = true },

  -- ── treesitter ──────────────────────────────────────────────
  ["@variable"] = { fg = c.text },
  ["@variable.builtin"] = { fg = c.red },
  ["@variable.parameter"] = { fg = c.subtext1 },
  ["@variable.member"] = { fg = c.text },
  ["@constant"] = { fg = c.peach or c.yellow },
  ["@constant.builtin"] = { fg = c.peach or c.yellow },
  ["@constant.macro"] = { fg = c.teal },
  ["@module"] = { fg = c.yellow },
  ["@label"] = { fg = c.pink },
  ["@string"] = { fg = c.green },
  ["@string.escape"] = { fg = c.pink },
  ["@string.regexp"] = { fg = c.teal },
  ["@string.special.url"] = { fg = c.blue, underline = true },
  ["@character"] = { fg = c.green },
  ["@number"] = { fg = c.peach or c.yellow },
  ["@boolean"] = { fg = c.peach or c.yellow },
  ["@float"] = { fg = c.peach or c.yellow },
  ["@function"] = { fg = c.blue },
  ["@function.builtin"] = { fg = c.blue },
  ["@function.call"] = { fg = c.blue },
  ["@function.macro"] = { fg = c.teal },
  ["@function.method"] = { fg = c.blue },
  ["@function.method.call"] = { fg = c.blue },
  ["@constructor"] = { fg = c.yellow },
  ["@parameter"] = { fg = c.subtext1 },
  ["@keyword"] = { fg = c.pink },
  ["@keyword.function"] = { fg = c.pink },
  ["@keyword.operator"] = { fg = c.teal },
  ["@keyword.return"] = { fg = c.pink },
  ["@keyword.import"] = { fg = c.pink },
  ["@keyword.conditional"] = { fg = c.pink },
  ["@keyword.repeat"] = { fg = c.pink },
  ["@keyword.exception"] = { fg = c.pink },
  ["@operator"] = { fg = c.teal },
  ["@punctuation.delimiter"] = { fg = c.overlay1 },
  ["@punctuation.bracket"] = { fg = c.overlay1 },
  ["@punctuation.special"] = { fg = c.pink },
  ["@type"] = { fg = c.yellow },
  ["@type.builtin"] = { fg = c.yellow },
  ["@type.definition"] = { fg = c.yellow },
  ["@type.qualifier"] = { fg = c.pink },
  ["@attribute"] = { fg = c.teal },
  ["@property"] = { fg = c.text },
  ["@field"] = { fg = c.text },
  ["@comment"] = { fg = c.overlay1, italic = true },
  ["@comment.todo"] = { fg = c.base, bg = c.yellow, bold = true },
  ["@comment.note"] = { fg = c.base, bg = c.teal, bold = true },
  ["@comment.warning"] = { fg = c.base, bg = c.yellow, bold = true },
  ["@comment.error"] = { fg = c.base, bg = c.red, bold = true },
  ["@tag"] = { fg = c.pink },
  ["@tag.attribute"] = { fg = c.blue },
  ["@tag.delimiter"] = { fg = c.overlay1 },
  ["@markup.heading"] = { fg = accent, bold = true },
  ["@markup.raw"] = { fg = c.green },
  ["@markup.link"] = { fg = c.blue, underline = true },
  ["@markup.link.url"] = { fg = c.blue, underline = true },
  ["@markup.strong"] = { bold = true },
  ["@markup.italic"] = { italic = true },
  ["@markup.list"] = { fg = c.teal },
  ["@diff.plus"] = { fg = c.green },
  ["@diff.minus"] = { fg = c.red },
  ["@diff.delta"] = { fg = c.blue },

  -- ── LSP semantic tokens ─────────────────────────────────────
  ["@lsp.type.namespace"] = { link = "@module" },
  ["@lsp.type.type"] = { link = "@type" },
  ["@lsp.type.class"] = { link = "@type" },
  ["@lsp.type.enum"] = { link = "@type" },
  ["@lsp.type.interface"] = { link = "@type" },
  ["@lsp.type.struct"] = { link = "@type" },
  ["@lsp.type.parameter"] = { link = "@variable.parameter" },
  ["@lsp.type.variable"] = { link = "@variable" },
  ["@lsp.type.property"] = { link = "@property" },
  ["@lsp.type.enumMember"] = { link = "@constant" },
  ["@lsp.type.function"] = { link = "@function" },
  ["@lsp.type.method"] = { link = "@function.method" },
  ["@lsp.type.macro"] = { link = "@function.macro" },
  ["@lsp.type.keyword"] = { link = "@keyword" },
  ["@lsp.type.comment"] = { link = "@comment" },
  ["@lsp.type.decorator"] = { link = "@attribute" },
  LspReferenceText = { bg = c.surface0 },
  LspReferenceRead = { bg = c.surface0 },
  LspReferenceWrite = { bg = c.surface1 },
  LspInlayHint = { fg = c.overlay1, bg = c.mantle, italic = true },
  LspCodeLens = { fg = c.overlay1, italic = true },
  LspSignatureActiveParameter = { fg = accent, bold = true },

  -- ── gitsigns ────────────────────────────────────────────────
  GitSignsAdd = { fg = c.green },
  GitSignsChange = { fg = c.blue },
  GitSignsDelete = { fg = c.red },
  GitSignsCurrentLineBlame = { fg = c.overlay1, italic = true },

  -- ── telescope / fzf-lua floats ──────────────────────────────
  TelescopeNormal = { fg = c.text, bg = float_bg },
  TelescopeBorder = { fg = border, bg = float_bg },
  TelescopeTitle = { fg = accent, bold = true },
  TelescopePromptNormal = { fg = c.text, bg = c.surface0 },
  TelescopePromptBorder = { fg = c.surface0, bg = c.surface0 },
  TelescopePromptTitle = { fg = c.base, bg = accent, bold = true },
  TelescopePromptPrefix = { fg = accent },
  TelescopePreviewTitle = { fg = c.base, bg = c.green, bold = true },
  TelescopeResultsTitle = { fg = float_bg, bg = float_bg },
  TelescopeSelection = { bg = accent_bg, bold = true },
  TelescopeSelectionCaret = { fg = accent, bg = accent_bg },
  TelescopeMatching = { fg = accent, bold = true },
  FzfLuaNormal = { fg = c.text, bg = float_bg },
  FzfLuaBorder = { fg = border, bg = float_bg },
  FzfLuaTitle = { fg = c.base, bg = accent, bold = true },

  -- ── neo-tree / snacks explorer / nvim-tree ──────────────────
  NeoTreeNormal = { fg = c.subtext1, bg = c.mantle },
  NeoTreeNormalNC = { fg = c.subtext1, bg = c.mantle },
  NeoTreeRootName = { fg = accent, bold = true },
  NeoTreeGitModified = { fg = c.blue },
  NeoTreeGitAdded = { fg = c.green },
  NeoTreeGitDeleted = { fg = c.red },
  NeoTreeGitUntracked = { fg = c.overlay1 },
  NeoTreeIndentMarker = { fg = c.surface1 },
  NeoTreeDirectoryIcon = { fg = accent },
  NeoTreeDirectoryName = { fg = c.subtext1 },
  NeoTreeFileNameOpened = { fg = accent },
  NvimTreeRootFolder = { fg = accent, bold = true },
  NvimTreeFolderIcon = { fg = accent },
  NvimTreeIndentMarker = { fg = c.surface1 },

  -- ── which-key ───────────────────────────────────────────────
  WhichKey = { fg = accent },
  WhichKeyGroup = { fg = c.blue },
  WhichKeyDesc = { fg = c.text },
  WhichKeySeparator = { fg = c.overlay1 },
  WhichKeyFloat = { bg = float_bg },
  WhichKeyBorder = { fg = border, bg = float_bg },
  WhichKeyValue = { fg = c.overlay1 },

  -- ── bufferline ──────────────────────────────────────────────
  BufferLineFill = { bg = c.crust },
  BufferLineBackground = { fg = c.overlay1, bg = c.mantle },
  BufferLineBufferVisible = { fg = c.subtext0, bg = c.mantle },
  BufferLineBufferSelected = { fg = c.text, bg = c.base, bold = true, italic = false },
  BufferLineIndicatorSelected = { fg = accent, bg = c.base },
  BufferLineModified = { fg = c.green, bg = c.mantle },
  BufferLineModifiedSelected = { fg = c.green, bg = c.base },

  -- ── completion (blink.cmp / nvim-cmp) ───────────────────────
  BlinkCmpMenu = { fg = c.subtext1, bg = float_bg },
  BlinkCmpMenuBorder = { fg = border, bg = float_bg },
  BlinkCmpMenuSelection = { bg = accent_bg, bold = true },
  BlinkCmpLabelMatch = { fg = accent, bold = true },
  BlinkCmpKind = { fg = accent },
  BlinkCmpDoc = { fg = c.text, bg = float_bg },
  BlinkCmpDocBorder = { fg = border, bg = float_bg },
  CmpItemAbbrMatch = { fg = accent, bold = true },
  CmpItemAbbrMatchFuzzy = { fg = accent },
  CmpItemKind = { fg = accent },
  CmpItemMenu = { fg = c.overlay1 },

  -- ── snacks.nvim (dashboard / notifier / indent) ─────────────
  SnacksNormal = { fg = c.text, bg = float_bg },
  SnacksBackdrop = { bg = c.crust },
  SnacksDashboardHeader = { fg = accent, bold = true },
  SnacksDashboardFooter = { fg = c.overlay1 },
  SnacksDashboardKey = { fg = c.peach or c.yellow },
  SnacksDashboardDesc = { fg = c.subtext1 },
  SnacksDashboardIcon = { fg = c.teal },
  SnacksIndent = { fg = c.surface0 },
  SnacksIndentScope = { fg = accent },
  SnacksNotifierInfo = { fg = c.blue },
  SnacksNotifierWarn = { fg = c.yellow },
  SnacksNotifierError = { fg = c.red },

  -- ── mini.indentscope / indent-blankline ─────────────────────
  MiniIndentscopeSymbol = { fg = accent },
  IblIndent = { fg = c.surface0 },
  IblScope = { fg = accent },

  -- ── notify / noice ──────────────────────────────────────────
  NotifyINFOBorder = { fg = c.blue },
  NotifyWARNBorder = { fg = c.yellow },
  NotifyERRORBorder = { fg = c.red },
  NotifyINFOTitle = { fg = c.blue },
  NotifyWARNTitle = { fg = c.yellow },
  NotifyERRORTitle = { fg = c.red },
  NoiceCmdlinePopupBorder = { fg = accent },
  NoiceCmdlineIcon = { fg = accent },

  -- ── flash.nvim labels ───────────────────────────────────────
  FlashLabel = { fg = c.base, bg = accent, bold = true },
  FlashMatch = { fg = c.text, bg = blend(c.base, accent, 0.30) },
  FlashCurrent = { fg = c.base, bg = c.yellow },
}

for group, spec in pairs(hl) do
  vim.api.nvim_set_hl(0, group, spec)
end

-- ANSI terminal palette inside :terminal — matches the desktop terminal theme.
vim.g.terminal_color_0 = c.surface1
vim.g.terminal_color_1 = c.red
vim.g.terminal_color_2 = c.green
vim.g.terminal_color_3 = c.yellow
vim.g.terminal_color_4 = c.blue
vim.g.terminal_color_5 = c.pink
vim.g.terminal_color_6 = c.teal
vim.g.terminal_color_7 = c.subtext1
vim.g.terminal_color_8 = c.surface2
vim.g.terminal_color_9 = c.red
vim.g.terminal_color_10 = c.green
vim.g.terminal_color_11 = c.yellow
vim.g.terminal_color_12 = c.blue
vim.g.terminal_color_13 = c.pink
vim.g.terminal_color_14 = c.teal
vim.g.terminal_color_15 = c.subtext0
