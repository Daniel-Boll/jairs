--- Teach Neovim that `.jr` is Jairs.
---
--- `vim.filetype.add` rather than an autocommand, because it is the mechanism Neovim's
--- own filetype detection uses and it applies to buffers opened before this file is
--- sourced as well as after.
vim.filetype.add({
  extension = {
    jr = "jairs",
  },
})
