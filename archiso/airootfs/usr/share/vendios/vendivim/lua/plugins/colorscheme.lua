-- Make LazyVim use the vendiOS-generated `vendi` colorscheme. Catppuccin is
-- pulled in only as a styled fallback (e.g. if the palette file is ever broken).
return {
  {
    "catppuccin/nvim",
    name = "catppuccin",
    lazy = true,
    priority = 1000,
    opts = { flavour = "mocha" },
  },
  {
    "LazyVim/LazyVim",
    opts = {
      colorscheme = "vendi",
    },
  },
}
