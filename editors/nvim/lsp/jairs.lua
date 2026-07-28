--- The Jairs language server, for Neovim's built-in LSP client.
---
--- Neovim 0.11+ discovers `lsp/<name>.lua` anywhere on the runtimepath, so this file
--- is the whole configuration: add this directory to the runtimepath and call
--- `vim.lsp.enable("jairs")`. No plugin manager and no `nvim-lspconfig`.
---
--- `cmd` deliberately prefers a `jr` on PATH and falls back to the debug build in this
--- repository, because the two audiences are different: someone who installed `jr` wants
--- theirs, and someone hacking on the compiler wants the one they just built. Guessing
--- the other way round means editing a file to test a change.
---
--- `--module-path` is passed rather than discovered, for the reason `jr check
--- --module-path` exists: guessing a search path silently changes which module a program
--- means.

local function repo_root()
  -- This file is `<root>/editors/nvim/lsp/jairs.lua`.
  local here = debug.getinfo(1, "S").source:sub(2)
  return vim.fs.normalize(vim.fs.dirname(here) .. "/../../..")
end

local root = repo_root()

local function server_command()
  local on_path = vim.fn.exepath("jr")
  if on_path ~= "" then
    return on_path
  end
  -- **The newer of the two builds wins, not `release` unconditionally.** A fixed
  -- preference is how a four-hour-old `target/release/jr` silently serves an editor while
  -- the developer tests a `target/debug/jr` they just built — the whole session looks like
  -- the change had no effect, and nothing anywhere says which binary answered. `PLAN.md`
  -- §7 carries this as a trap because it has cost real time twice, once in a wave whose
  -- `verify.lua` run was verifying the *previous* wave.
  --
  -- Comparing mtimes is not a heuristic about which build is "better"; it is the only
  -- available answer to "which one reflects the source as it is now".
  local newest, newest_time = nil, -1
  for _, profile in ipairs({ "release", "debug" }) do
    local built = root .. "/target/" .. profile .. "/jr"
    local stat = vim.uv.fs_stat(built)
    if stat and stat.mtime.sec > newest_time then
      newest, newest_time = built, stat.mtime.sec
    end
  end
  if newest then
    return newest
  end
  -- Returned anyway: Neovim reports "command not found" against this name, which is a
  -- better message than a nil that fails somewhere inside the client.
  return "jr"
end

return {
  cmd = { server_command(), "lsp", "--quiet", "--module-path", root .. "/modules" },
  filetypes = { "jairs" },
  --- The project root, so one server serves the whole workspace.
  ---
  --- **Order is priority, not proximity** (`:h vim.fs.root`): the first marker in this
  --- list that matches anywhere up the tree wins, even if a later one matches closer. So
  --- `.git` comes first deliberately. ADR-0026 records why the original order was wrong:
  --- with `modules` first, opening `tests/corpus/valid/024-hello.jr` in this very
  --- repository rooted the server at `tests/corpus`, because `tests/corpus/modules/` is a
  --- *fixture* directory. A directory named `modules` is far too common to be a project
  --- marker.
  --- `root_dir` as the last resort, because a marker list can match **nothing**: a
  --- standalone `.jr` file in a directory with no `.git` and no `modules/` — a scratch file
  --- in `/tmp`, or a single-file experiment — leaves `root_dir` nil, and a nil root means an
  --- empty workspace. Every workspace-scoped capability then answers from nothing:
  --- `references` reports only the declaration, `rename` edits only the open buffer, and
  --- `workspaceSymbol` finds nothing. All three look like working features returning a
  --- confident wrong answer, which is the exact failure ADR-0029 §1 and ADR-0030's
  --- implementation each hit once already.
  ---
  --- The file's own directory is the honest fallback: it is what a user editing one file
  --- means by "the project", and the server already adopts an open file's directory as a
  --- root for the same reason (`adopt_root`). Verified rather than assumed — a `.jr` file in
  --- a bare directory attached with `root_dir=nil` before this.
  root_dir = function(bufnr, on_dir)
    local file = vim.api.nvim_buf_get_name(bufnr)
    local found = vim.fs.root(bufnr, { ".git", "modules" })
    on_dir(found or (file ~= "" and vim.fs.dirname(file) or nil))
  end,
  root_markers = { ".git", "modules" },
  --- UTF-8 is offered first because a byte offset is what a `jr-base` span already is
  --- (ADR-0024 §3). Neovim supports the negotiation, so this costs nothing and removes
  --- a conversion from every request.
  capabilities = {
    general = {
      positionEncodings = { "utf-8", "utf-16" },
    },
  },
}
