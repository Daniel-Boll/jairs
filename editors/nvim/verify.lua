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

    -- Documentation highlighting, checked on the corpus file that has some. The capture
    -- is predicated on `#lua-match?`, which is Neovim's own predicate: `tree-sitter
    -- query` validates node names but knows nothing about it, so this is the only place
    -- the doc-comment capture is actually exercised.
    local docs_file = root .. "/tests/corpus/valid/026-doc-comments.jr"
    if vim.uv.fs_stat(docs_file) then
      vim.cmd.edit(vim.fn.fnameescape(docs_file))
      local docs_buf = vim.api.nvim_get_current_buf()
      local docs_ok, docs_parser = pcall(vim.treesitter.get_parser, docs_buf, "jairs")
      local query_ok2, docs_query = pcall(vim.treesitter.query.get, "jairs", "highlights")
      if docs_ok and docs_parser and query_ok2 and docs_query then
        local docs_tree = docs_parser:parse()[1]
        local seen_docs = {}
        for id, _ in docs_query:iter_captures(docs_tree:root(), docs_buf, 0, -1) do
          seen_docs[docs_query.captures[id]] = true
        end
        check(
          "a /// comment captures as @comment.documentation",
          seen_docs["comment.documentation"] == true,
          vim.inspect(vim.tbl_keys(seen_docs))
        )
        check(
          "an ordinary // comment still captures as @comment",
          seen_docs["comment"] == true
        )
      end
      -- Back to the file the LSP checks below expect.
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
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

    -- The binary the client actually launched, and *which* one. A fixed preference for
    -- `target/release` is how a stale release build silently serves an editor while the
    -- developer tests a debug build they just made: the session looks like the change had
    -- no effect and nothing says which binary answered. `PLAN.md` §7 carries it as a trap
    -- because it has cost real time twice. The rule is now "whichever is newer", so this
    -- asserts the launched path is in fact the newest of the two that exist.
    local launched = client.config.cmd[1]
    local newest, newest_time = nil, -1
    for _, profile in ipairs({ "release", "debug" }) do
      local candidate = root .. "/target/" .. profile .. "/jr"
      local stat = vim.uv.fs_stat(candidate)
      if stat and stat.mtime.sec > newest_time then
        newest, newest_time = candidate, stat.mtime.sec
      end
    end
    check(
      "the server launched is the most recently built one, not a stale profile",
      newest == nil or launched == newest or vim.fn.exepath("jr") ~= "",
      launched .. " (newest is " .. tostring(newest) .. ")"
    )

    -- Asserting the *text* of each hover, not merely that one arrived: a server that
    -- answered every hover with an empty string would pass the weaker check.
    --
    -- Line 28 (0-based) is `    sum := add(p.x, p.y);`. Column 15 is the `p` of `p.x`,
    -- column 11 the callee `add`, and column 4 the `sum` being declared. That last one
    -- used to be excluded here with a comment saying the correct answer was no hover —
    -- which was wrong, and ADR-0028 §4 is the correction: a declaration's name is not an
    -- expression, so it needs `locate_declaration`, not exclusion from the test.
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
    check(
      "hover on a local names it and its type",
      struct_hover == "```jr\n024-hello\np: Point\n```",
      struct_hover
    )
    local proc_hover = hover_at(28, 11)
    check(
      "hover on a call renders the declaration, with parameter names",
      proc_hover == "```jr\n024-hello\nadd :: (a: s64, b: s64) -> s64\n```",
      proc_hover
    )
    local decl_hover = hover_at(28, 4)
    check(
      "hover on a declaration is no longer empty",
      decl_hover == "```jr\n024-hello\nsum: s64\n```",
      decl_hover
    )

    -- The card that prompted the whole wave: container, signature, rule, prose — from
    -- another file. Line 30 is `        print(MESSAGE);`.
    local imported_hover = hover_at(30, 8)
    check(
      "hover on an imported procedure shows its module and its documentation",
      imported_hover
        == "```jr\nBasic\nprint :: (s: string)\n```\n\n---\n\nWrites a string to standard output.",
      imported_hover
    )

    -- Completion, including the snippet and the lazily-resolved documentation.
    local function complete_at(line, character)
      local items
      vim.lsp.buf_request(buf, "textDocument/completion", {
        textDocument = vim.lsp.util.make_text_document_params(buf),
        position = { line = line, character = character },
        context = { triggerKind = 1 },
      }, function(_, result)
        if result then
          items = result.items or result
        else
          items = {}
        end
      end)
      vim.wait(10000, function()
        return items ~= nil
      end, 50)
      return items or {}
    end

    local offered = complete_at(28, 14)
    local by_label = {}
    for _, item in ipairs(offered) do
      by_label[item.label] = item
    end
    check("completion offers something at all", #offered > 0, #offered)
    check("completion offers a local in scope", by_label["sum"] ~= nil)
    check("completion offers an imported procedure", by_label["print"] ~= nil)
    check("completion offers a keyword", by_label["while"] ~= nil)
    check("completion offers a builtin type", by_label["s64"] ~= nil)
    check(
      "a reserved keyword is not offered",
      by_label["cast"] == nil and by_label["enum"] == nil
    )

    local add_item = by_label["add"]
    check("completion offers a procedure", add_item ~= nil)
    if add_item then
      check(
        "a procedure completes as a call snippet",
        add_item.insertText == "add(${1:a}, ${2:b})$0",
        add_item.insertText
      )
      check(
        "the snippet is marked as one, or the client inserts it literally",
        add_item.insertTextFormat == 2,
        add_item.insertTextFormat
      )
      check(
        "the item carries the signature as its detail",
        add_item.detail == "add :: (a: s64, b: s64) -> s64",
        add_item.detail
      )
    end

    -- Field completion after `.`, which is what the `.` trigger character is for.
    local fields = complete_at(28, 18)
    local field_labels = {}
    for _, item in ipairs(fields) do
      field_labels[item.label] = true
    end
    check(
      "a dot offers the struct's fields",
      field_labels["x"] and field_labels["y"],
      vim.inspect(vim.tbl_keys(field_labels))
    )

    -- Resolve, round-tripping the item the server produced.
    local print_item = by_label["print"]
    if print_item then
      local resolved
      vim.lsp.buf_request(buf, "completionItem/resolve", print_item, function(_, result)
        resolved = result or false
      end)
      vim.wait(10000, function()
        return resolved ~= nil
      end, 50)
      local docs = resolved
        and resolved.documentation
        and (resolved.documentation.value or resolved.documentation)
      check(
        "resolve supplies the same card the hover shows",
        docs
          == "```jr\nBasic\nprint :: (s: string)\n```\n\n---\n\nWrites a string to standard output.",
        docs
      )
    end

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

    -- ---- navigation (ADR-0029, ADR-0030) ------------------------------------

    for _, capability in ipairs({
      "referencesProvider",
      "documentHighlightProvider",
      "documentSymbolProvider",
      "workspaceSymbolProvider",
    }) do
      local advertised = client.server_capabilities[capability]
      check(
        "advertises " .. capability,
        advertised ~= nil and advertised ~= false,
        vim.inspect(advertised)
      )
    end
    check(
      "advertises prepareRename, so a keyword can be refused before typing",
      client.server_capabilities.renameProvider
        and client.server_capabilities.renameProvider.prepareProvider == true,
      vim.inspect(client.server_capabilities.renameProvider)
    )

    -- ADR-0029 §2 makes a client file watcher the primary freshness mechanism, and says to
    -- verify rather than assume this editor has one. Without it the server silently falls
    -- back to re-walking on save, which nothing else here would notice.
    local watched = vim.lsp.protocol.make_client_capabilities().workspace.didChangeWatchedFiles
    check(
      "this Neovim can watch files for the server",
      watched and watched.dynamicRegistration == true,
      vim.inspect(watched)
    )

    local function request(method, params)
      local result
      vim.lsp.buf_request(buf, method, params, function(_, response)
        result = response or false
      end)
      vim.wait(20000, function()
        return result ~= nil
      end, 50)
      return result
    end
    local doc = vim.lsp.util.make_text_document_params(buf)

    -- Line 19 (0-based) is `add :: (a: s64, b: s64) -> s64 {`. Worth stating: the first
    -- draft of these checks used line 20, which is `return a + b;`, and read as two server
    -- bugs rather than as an off-by-one in the test.
    local symbols = request("textDocument/documentSymbol", { textDocument = doc })
    local by_name = {}
    for _, symbol in ipairs(symbols or {}) do
      by_name[symbol.name] = symbol
    end
    check(
      "documentSymbol outlines the file",
      by_name["Point"] and by_name["add"] and by_name["main"],
      vim.inspect(vim.tbl_keys(by_name))
    )
    check(
      "a struct's fields nest under it",
      by_name["Point"] and by_name["Point"].children and #by_name["Point"].children == 2,
      by_name["Point"] and vim.inspect(by_name["Point"].children)
    )
    check(
      "an outline entry carries the same signature the hover does",
      by_name["add"] and by_name["add"].detail == "add :: (a: s64, b: s64) -> s64",
      by_name["add"] and by_name["add"].detail
    )
    check(
      "parameters do not nest under a procedure",
      by_name["add"] and by_name["add"].children == nil,
      by_name["add"] and vim.inspect(by_name["add"].children)
    )

    local found = request("textDocument/references", {
      textDocument = doc,
      position = { line = 19, character = 0 },
      context = { includeDeclaration = true },
    })
    check(
      "references finds the declaration and the call",
      found and #found >= 2,
      found and #found
    )

    local highlights = request("textDocument/documentHighlight", {
      textDocument = doc,
      position = { line = 28, character = 11 },
    })
    check(
      "documentHighlight answers for the cursor's word",
      highlights and #highlights >= 1,
      highlights and #highlights
    )

    local prepared = request("textDocument/prepareRename", {
      textDocument = doc,
      position = { line = 19, character = 0 },
    })
    check(
      "prepareRename offers the current name",
      prepared and prepared.placeholder == "add",
      vim.inspect(prepared)
    )

    local hits = request("workspace/symbol", { query = "print" })
    check(
      "workspaceSymbol reaches into modules/Basic",
      hits and #hits > 0 and tostring(hits[1].location.uri):find("Basic") ~= nil,
      hits and vim.inspect(hits[1])
    )

    -- ---- code actions, signature help, inlay hints (ADR-0031) ---------------

    for _, capability in ipairs({
      "codeActionProvider",
      "signatureHelpProvider",
      "inlayHintProvider",
    }) do
      local advertised = client.server_capabilities[capability]
      check(
        "advertises " .. capability,
        advertised ~= nil and advertised ~= false,
        vim.inspect(advertised)
      )
    end
    check(
      "the organise-imports kind is listed, so a client can put it on its own menu",
      vim.tbl_contains(
        (client.server_capabilities.codeActionProvider or {}).codeActionKinds or {},
        "source.organizeImports"
      ),
      vim.inspect(client.server_capabilities.codeActionProvider)
    )

    -- Line 28 (0-based) is `    sum := add(p.x, p.y);`. Character 20 is inside the second
    -- argument. Stated because the last wave's two "server bugs" in these checks were both
    -- an off-by-one here rather than anything in the server.
    local help = request("textDocument/signatureHelp", {
      textDocument = doc,
      position = { line = 28, character = 20 },
    })
    check(
      "signatureHelp names the procedure being called",
      help
        and help.signatures
        and help.signatures[1]
        and help.signatures[1].label == "add :: (a: s64, b: s64) -> s64",
      vim.inspect(help)
    )
    check(
      "signatureHelp marks the argument the cursor is in",
      help and help.signatures and help.signatures[1] and help.signatures[1].activeParameter == 1,
      help and help.signatures and vim.inspect(help.signatures[1])
    )

    -- Inlay hints over the whole file. `sum := add(p.x, p.y)` must get `: s64`, and
    -- `COMPUTED :: #run add(2, 3)` must get `= 5` — the hint that makes compile-time
    -- execution visible, which is the one nothing outside this project can offer.
    local hints = request("textDocument/inlayHint", {
      textDocument = doc,
      range = {
        start = { line = 0, character = 0 },
        ["end"] = { line = 60, character = 0 },
      },
    })
    local labels = {}
    for _, hint in ipairs(hints or {}) do
      labels[#labels + 1] = type(hint.label) == "string" and hint.label or vim.inspect(hint.label)
    end
    check(
      "an inferred local gets a type hint",
      vim.tbl_contains(labels, ": s64"),
      vim.inspect(labels)
    )
    check(
      "a #run constant shows the value the VM computed",
      vim.tbl_contains(labels, " = 5"),
      vim.inspect(labels)
    )

    -- A code action on a file that needs one: `print` with no `#import "Basic";`.
    local needs_import = vim.fn.tempname() .. ".jr"
    vim.fn.writefile({ "main :: () {", '    print("hi\\n");', "}" }, needs_import)
    vim.cmd.edit(vim.fn.fnameescape(needs_import))
    local importing = vim.api.nvim_get_current_buf()
    -- Wait for the diagnostic first: a code-action request carries the client's own
    -- diagnostics, so asking before they land would test nothing.
    vim.wait(20000, function()
      return #vim.diagnostic.get(importing) > 0
    end, 100)
    local unresolved = vim.diagnostic.get(importing)[1]
    local action_result
    vim.lsp.buf_request(importing, "textDocument/codeAction", {
      textDocument = vim.lsp.util.make_text_document_params(importing),
      range = {
        start = { line = 1, character = 4 },
        ["end"] = { line = 1, character = 9 },
      },
      context = {
        diagnostics = unresolved
            and {
              {
                range = {
                  start = { line = 1, character = 4 },
                  ["end"] = { line = 1, character = 9 },
                },
                message = unresolved.message,
                code = unresolved.code,
                severity = 1,
              },
            }
          or {},
      },
    }, function(_, response)
      action_result = response or false
    end)
    vim.wait(20000, function()
      return action_result ~= nil
    end, 50)
    local action_titles = {}
    for _, action in ipairs(action_result or {}) do
      action_titles[#action_titles + 1] = action.title
    end
    check(
      "an unresolved name offers an import of the module that exports it",
      vim.tbl_contains(action_titles, "import `Basic` for `print`"),
      vim.inspect(action_titles)
    )

    -- Goto-definition on the `#import` line, at **every** column. It answered nothing at
    -- all until ADR-0035: an import is lowered with `name: None`, and `locate_declaration`
    -- skipped nameless items to keep a top-level `#run` from matching, so the one
    -- declaration in the language that names another *file* was the one you could not
    -- follow. Line 9 (0-based) of `024-hello.jr` is `#import "Basic";`.
    local import_line = 9
    local import_len = #(vim.api.nvim_buf_get_lines(buf, import_line, import_line + 1, false)[1] or "")
    local reached, missed = 0, nil
    for column = 0, import_len - 1 do
      local target = request("textDocument/definition", {
        textDocument = doc,
        position = { line = import_line, character = column },
      })
      local uri = target and (target.uri or (target[1] and target[1].uri))
      if uri and tostring(uri):find("modules/Basic/module.jr") then
        reached = reached + 1
      else
        missed = missed or column
      end
    end
    check(
      "goto-definition on an #import works at every column of the line",
      reached == import_len,
      "reached " .. reached .. "/" .. import_len .. ", first miss at column " .. tostring(missed)
    )
    check(
      "hovering an #import names the resolved module file",
      (function()
        local h = request("textDocument/hover", {
          textDocument = doc,
          position = { line = import_line, character = 3 },
        })
        local value = h and h.contents and h.contents.value or ""
        -- The resolved path is the part worth hovering for: `#import "Basic"` does not say
        -- *which* `Basic` (ADR-0035 §2).
        return value:find("modules/Basic/module.jr", 1, true) ~= nil
      end)()
    )

    -- A standalone file in a directory with no `.git` and no `modules/`: the case a marker
    -- list cannot answer. Before the `root_dir` fallback this attached with `root_dir=nil`,
    -- which means an empty workspace — so `references` reported only the declaration and
    -- `rename` would have edited only the open buffer. Three capabilities returning a
    -- confident wrong answer, and nothing on screen saying so.
    local loose_dir = vim.fn.tempname()
    vim.fn.mkdir(loose_dir, "p")
    local loose = loose_dir .. "/loose.jr"
    vim.fn.writefile({ "solo :: (a: s64) -> s64 {", "    return a;", "}" }, loose)
    vim.cmd.edit(vim.fn.fnameescape(loose))
    local loose_buf = vim.api.nvim_get_current_buf()
    local loose_attached = vim.wait(20000, function()
      return #vim.lsp.get_clients({ bufnr = loose_buf }) > 0
    end, 100)
    check("a .jr file outside any project still attaches", loose_attached)
    if loose_attached then
      local loose_client = vim.lsp.get_clients({ bufnr = loose_buf })[1]
      check(
        "a file with no project marker still gets a workspace root",
        loose_client.root_dir ~= nil and loose_client.root_dir ~= vim.NIL,
        vim.inspect(loose_client.root_dir)
      )
    end
    vim.cmd.edit(vim.fn.fnameescape(sample))
    buf = vim.api.nvim_get_current_buf()

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
