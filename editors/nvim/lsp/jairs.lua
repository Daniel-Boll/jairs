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
  for _, profile in ipairs({ "release", "debug" }) do
    local built = root .. "/target/" .. profile .. "/jr"
    if vim.uv.fs_stat(built) then
      return built
    end
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
  --- `modules/` is the marker rather than a `.git` directory: a Jairs project is
  --- defined by having modules to import, and vendoring one inside a git repository
  --- should not attach it to the outer project.
  root_markers = { "modules", ".git" },
  --- UTF-8 is offered first because a byte offset is what a `jr-base` span already is
  --- (ADR-0024 §3). Neovim supports the negotiation, so this costs nothing and removes
  --- a conversion from every request.
  capabilities = {
    general = {
      positionEncodings = { "utf-8", "utf-16" },
    },
  },
}
