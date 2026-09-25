-- Small, opinionated vendiOS tweaks on top of LazyVim's editor defaults.
return {
  -- Treesitter grammars for the languages vendiOS itself is built in, so the
  -- compositor/bar configs highlight correctly out of the box.
  {
    "nvim-treesitter/nvim-treesitter",
    opts = function(_, opts)
      opts.ensure_installed = opts.ensure_installed or {}
      vim.list_extend(opts.ensure_installed, {
        "rust",
        "lua",
        "qmljs",
        "kdl",
        "bash",
        "toml",
        "json",
        "yaml",
        "markdown",
        "markdown_inline",
      })
    end,
  },

  -- Transparent floats over the live vendiwm blur look great; keep the dashboard
  -- showing the vendiOS diamond.
  {
    "folke/snacks.nvim",
    opts = {
      dashboard = {
        preset = {
          header = table.concat({
            "",
            "              ▗▟▙▖             ",
            "             ▄████▄            ",
            "           ▄████████▄          ",
            "         ▗▟██████████▙▖        ",
            "         ▝▜██████████▛▘        ",
            "           ▀████████▀          ",
            "             ▀████▀            ",
            "              ▝▜▛▘             ",
            "",
            "        v e n d i V i m        ",
            "",
          }, "\n"),
        },
      },
    },
  },
}
