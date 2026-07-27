--- Headless check that this directory actually configures Neovim.
---
--- Run it with:
---
---     nvim --headless -u NONE -l editors/nvim/verify.lua
---
--- # Why this is a script and not a `cargo test`
---
--- Because it needs Neovim, and Neovim is not a build dependency of this workspace.
--- Making it one of the six gates would make `cargo test` fail on a machine that has no
--- editor installed, which is the wrong trade for a packaging directory. So this is
--- checked in, runnable in one line, and named in `editors/nvim/README.md` — and the
--- honest consequence, recorded in `PLAN.md` §1.5, is that editor integration is
--- *verified* rather than *gated*.
---
--- It exits non-zero on the first failure so it is usable from a shell or a CI job that
--- does have Neovim.

local failures = 0

local function check(name, ok, detail)
  if ok then
    io.write("ok   ", name, "\n")
  else
    failures = failures + 1
    io.write("FAIL ", name, detail and ("  (" .. tostring(detail) .. ")") or "", "\n")
  end
end

-- Resolved to a real absolute path, not just normalised. `nvim -l` reports this script's
-- source as the *relative* path it was invoked with, so `here .. "/../.."` normalises to
-- something like `.` — which every path built from it still happens to work with, and
-- which silently fails to equal the absolute `root_dir` an LSP client reports.
local here = vim.uv.fs_realpath(vim.fs.dirname(debug.getinfo(1, "S").source:sub(2)))
local root = vim.uv.fs_realpath(here .. "/../..")

vim.opt.runtimepath:append(here)
vim.opt.packpath = ""

-- `-u NONE` starts with filetype detection *off*, and a runtimepath appended after
-- startup has had neither its `plugin/` nor its `ftdetect/` scripts sourced. A real
-- user's config has detection on already and Neovim sources both for them; here both
-- have to be asked for explicitly, or this script would report a failure that only
-- exists because of how it was launched.
vim.cmd("filetype plugin indent on")
vim.cmd("runtime! ftdetect/*.lua")

-- ---------------------------------------------------------------------------
-- Filetype
-- ---------------------------------------------------------------------------

local sample = root .. "/tests/corpus/valid/024-hello.jr"
vim.cmd.edit(vim.fn.fnameescape(sample))
local buf = vim.api.nvim_get_current_buf()
check("a .jr file gets filetype=jairs", vim.bo[buf].filetype == "jairs", vim.bo[buf].filetype)
check("the comment string is set", vim.bo[buf].commentstring == "// %s", vim.bo[buf].commentstring)
check("indentation matches jr fmt", vim.bo[buf].shiftwidth == 4, vim.bo[buf].shiftwidth)

-- ---------------------------------------------------------------------------
-- Tree-sitter
-- ---------------------------------------------------------------------------

local has_parser = pcall(vim.treesitter.language.inspect, "jairs")
check("the tree-sitter parser is installed", has_parser, "run editors/nvim/build.sh")

if has_parser then
  local ok, parser = pcall(vim.treesitter.get_parser, buf, "jairs")
  check("the buffer parses", ok and parser ~= nil)
  if ok and parser then
    local tree = parser:parse()[1]
    check("the parse has no ERROR node", not tree:root():has_error())

    -- The queries are the point of shipping this directory at all: a parser with no
    -- highlights query colours nothing.
    local query_ok, query = pcall(vim.treesitter.query.get, "jairs", "highlights")
    check("the highlights query loads", query_ok and query ~= nil)
    if query_ok and query then
      local seen = {}
      for id, _ in query:iter_captures(tree:root(), buf, 0, -1) do
        seen[query.captures[id]] = true
      end
      -- Named individually rather than counted, so that losing one is a named failure
      -- instead of a number that drifted.
      for _, capture in ipairs({ "comment", "string", "number", "keyword" }) do
        check("highlights capture @" .. capture, seen[capture] == true)
      end
    end
  end
end

-- ---------------------------------------------------------------------------
-- LSP
-- ---------------------------------------------------------------------------

local config = vim.lsp.config and true or false
check("this Neovim has vim.lsp.config (0.11+)", config, tostring(vim.version()))

if config then
  vim.lsp.enable("jairs")
  -- Re-trigger FileType so the freshly enabled config attaches to the open buffer.
  vim.api.nvim_exec_autocmds("FileType", { buffer = buf })

  local attached = vim.wait(20000, function()
    return #vim.lsp.get_clients({ bufnr = buf }) > 0
  end, 100)
  check("the server attaches to the buffer", attached, "is `jr` built? cargo build -p jr-cli")

  if attached then
    local client = vim.lsp.get_clients({ bufnr = buf })[1]
    check("the client is ours", client.name == "jairs", client.name)
    check(
      "the negotiated encoding is utf-8",
      client.offset_encoding == "utf-8",
      client.offset_encoding
    )
    -- Pinned because it was wrong, and wrong in a way only a real client showed:
    -- `root_markers` order is priority rather than proximity, so listing `modules` first
    -- rooted this repository's own corpus files at `tests/corpus`, where the fixture
    -- directory `tests/corpus/modules/` lives (ADR-0026).
    check(
      "the workspace root is the repository, not a fixture directory",
      client.root_dir == root,
      client.root_dir
    )

    -- Asserting the *text* of each hover, not merely that one arrived: a server that
    -- answered every hover with an empty string would pass the weaker check.
    --
    -- Line 28 (0-based) is `    sum := add(p.x, p.y);`. Column 15 is the `p` of `p.x`
    -- and column 11 is the callee `add`. Deliberately *not* column 4, the `sum` in
    -- `sum := …`: that is a declaration rather than an expression, so the correct answer
    -- there is no hover at all, and a test that expected one would be asserting a bug.
    local function hover_at(line, character)
      local text
      vim.lsp.buf_request(buf, "textDocument/hover", {
        textDocument = vim.lsp.util.make_text_document_params(buf),
        position = { line = line, character = character },
      }, function(_, result)
        text = (result and result.contents and result.contents.value) or ""
      end)
      vim.wait(10000, function()
        return text ~= nil
      end, 50)
      return text
    end

    local struct_hover = hover_at(28, 15)
    check("hover names a struct by its declared name", struct_hover == "```jr\nPoint\n```", struct_hover)
    local proc_hover = hover_at(28, 11)
    check(
      "hover renders a procedure signature",
      proc_hover == "```jr\n(s64, s64) -> s64\n```",
      proc_hover
    )

    -- Goto-definition across the `#import`, which is the one that shows the module
    -- system resolved rather than merely type-checked. Line 30 is `print(MESSAGE);`.
    local target
    vim.lsp.buf_request(buf, "textDocument/definition", {
      textDocument = vim.lsp.util.make_text_document_params(buf),
      position = { line = 30, character = 8 },
    }, function(_, result)
      if result then
        local first = result.uri and result or result[1]
        target = first and (first.uri or first.targetUri)
      end
    end)
    vim.wait(10000, function()
      return target ~= nil
    end, 50)
    check(
      "goto-definition crosses into modules/Basic",
      target and target:find("modules/Basic/module.jr") ~= nil,
      target
    )

    -- Diagnostics, on a file written to be wrong. Published as a notification, so this
    -- waits for them to land rather than requesting them.
    local broken = vim.fn.tempname() .. ".jr"
    vim.fn.writefile({ "main :: () {", "    x: bool = 1;", "}" }, broken)
    vim.cmd.edit(vim.fn.fnameescape(broken))
    local bad = vim.api.nvim_get_current_buf()
    local got = vim.wait(20000, function()
      return #vim.diagnostic.get(bad) > 0
    end, 100)
    check("a type error becomes a diagnostic", got, "none published")
    if got then
      local first = vim.diagnostic.get(bad)[1]
      check("the diagnostic is an error", first.severity == vim.diagnostic.severity.ERROR)
      check("it carries a stable code", first.code ~= nil, vim.inspect(first.code))
      check("it is on the offending line", first.lnum == 1, first.lnum)
    end
  end
end

io.write("\n", failures == 0 and "all checks passed\n" or (failures .. " check(s) failed\n"))
vim.cmd(failures == 0 and "cquit 0" or "cquit 1")
