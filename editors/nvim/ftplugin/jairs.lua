--- Per-buffer setup for a Jairs file.
---
--- Sourced by Neovim for every buffer whose filetype is `jairs`.

-- `//` line comments and `/* */` blocks, which nest (see `docs/spec/01-lexical.md`).
-- `commentstring` drives `gc`; `comments` drives `o`/`O` continuation and formatting.
vim.bo.commentstring = "// %s"
vim.bo.comments = "s1:/*,mb:*,ex:*/,://"

-- Four spaces, no tabs — matching what `jr fmt` produces, so that hand-typed and
-- formatted code do not disagree.
vim.bo.expandtab = true
vim.bo.shiftwidth = 4
vim.bo.softtabstop = 4
vim.bo.tabstop = 4

-- Tree-sitter highlighting, if the parser is built. `pcall` because a missing parser is
-- an ordinary state — `editors/nvim/build.sh` has not been run — and it should not throw
-- an error into every `.jr` buffer.
pcall(vim.treesitter.start)
