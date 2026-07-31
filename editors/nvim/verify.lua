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

    -- Arrays, on the corpus file that has them. The compiler's parser and tree-sitter
    -- must agree about the *shape*, and ADR-0010's drift gate compares error counts —
    -- which cannot see a wrong tree. `cast(u8, x)` parsed as a `call_expr` with zero
    -- ERROR nodes for a whole wave for exactly this reason, so an array's nodes are
    -- named here rather than assumed from a clean parse.
    local arrays_file = root .. "/tests/corpus/valid/030-arrays.jr"
    if vim.uv.fs_stat(arrays_file) then
      vim.cmd.edit(vim.fn.fnameescape(arrays_file))
      local arr_buf = vim.api.nvim_get_current_buf()
      local arr_ok, arr_parser = pcall(vim.treesitter.get_parser, arr_buf, "jairs")
      if arr_ok and arr_parser then
        local arr_tree = arr_parser:parse()[1]
        check("the arrays corpus file parses with no ERROR node", not arr_tree:root():has_error())
        local kinds = {}
        local function walk(node)
          kinds[node:type()] = true
          for child in node:iter_children() do
            walk(child)
          end
        end
        walk(arr_tree:root())
        check("tree-sitter produces array_type, not a wrong shape", kinds["array_type"] == true)
        check("tree-sitter produces index_expr, not a call_expr", kinds["index_expr"] == true)
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- Floats, for the same reason and found the same way: the grammar's `float_literal`
    -- required a fractional part, so `1e9` produced an ERROR node here while the compiler's
    -- lexer accepted it (ADR-0040). The drift gate sees an ERROR only once a corpus file
    -- contains one, so this pins the *kinds* as well.
    local floats_file = root .. "/tests/corpus/valid/031-floats.jr"
    if vim.uv.fs_stat(floats_file) then
      vim.cmd.edit(vim.fn.fnameescape(floats_file))
      local f_buf = vim.api.nvim_get_current_buf()
      local f_ok, f_parser = pcall(vim.treesitter.get_parser, f_buf, "jairs")
      if f_ok and f_parser then
        local f_tree = f_parser:parse()[1]
        check("the floats corpus file parses with no ERROR node", not f_tree:root():has_error())
        local f_kinds = {}
        local function fwalk(node)
          f_kinds[node:type()] = true
          for child in node:iter_children() do
            fwalk(child)
          end
        end
        fwalk(f_tree:root())
        check("tree-sitter produces float_literal", f_kinds["float_literal"] == true)
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- Enums, for the third time and the same reason. `enum` was also in the *reserved
    -- keyword* highlight match, which would have kept colouring it "arrives in a later
    -- wave" after it arrived (ADR-0041) -- the identical trap `cast` walked into. So this
    -- checks the node kinds *and* that `enum` is captured as a keyword rather than as a
    -- reserved word.
    local enums_file = root .. "/tests/corpus/valid/032-enums.jr"
    if vim.uv.fs_stat(enums_file) then
      vim.cmd.edit(vim.fn.fnameescape(enums_file))
      local e_buf = vim.api.nvim_get_current_buf()
      local e_ok, e_parser = pcall(vim.treesitter.get_parser, e_buf, "jairs")
      if e_ok and e_parser then
        local e_tree = e_parser:parse()[1]
        check("the enums corpus file parses with no ERROR node", not e_tree:root():has_error())
        local e_kinds = {}
        local function ewalk(node)
          e_kinds[node:type()] = true
          for child in node:iter_children() do
            ewalk(child)
          end
        end
        ewalk(e_tree:root())
        check("tree-sitter produces enum_type", e_kinds["enum_type"] == true)
        check("tree-sitter produces member", e_kinds["member"] == true)

        local e_query_ok, e_query = pcall(vim.treesitter.query.get, "jairs", "highlights")
        if e_query_ok and e_query then
          local saw_keyword_type = false
          local saw_reserved = false
          for id, node in e_query:iter_captures(e_tree:root(), e_buf, 0, -1) do
            local capture = e_query.captures[id]
            local text = vim.treesitter.get_node_text(node, e_buf)
            if capture == "keyword.type" and text == "enum" then
              saw_keyword_type = true
            end
            if capture == "keyword.reserved" and text == "enum" then
              saw_reserved = true
            end
          end
          check("`enum` highlights as a keyword", saw_keyword_type)
          check("`enum` is no longer highlighted as reserved", not saw_reserved)
        end
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- `enum_flags`, which was never reserved and so had nothing to remove from the reserved
    -- match — but still needs its own highlight entry, and the *keyword* is what distinguishes
    -- a flag set from an alternative (ADR-0043 §1).
    local flags_file = root .. "/tests/corpus/valid/034-enum-flags.jr"
    if vim.uv.fs_stat(flags_file) then
      vim.cmd.edit(vim.fn.fnameescape(flags_file))
      local g_buf = vim.api.nvim_get_current_buf()
      local g_ok, g_parser = pcall(vim.treesitter.get_parser, g_buf, "jairs")
      if g_ok and g_parser then
        local g_tree = g_parser:parse()[1]
        check(
          "the enum_flags corpus file parses with no ERROR node",
          not g_tree:root():has_error()
        )
        local g_query_ok, g_query = pcall(vim.treesitter.query.get, "jairs", "highlights")
        if g_query_ok and g_query then
          local saw_flags = false
          for id, node in g_query:iter_captures(g_tree:root(), g_buf, 0, -1) do
            local capture = g_query.captures[id]
            local text = vim.treesitter.get_node_text(node, g_buf)
            if capture == "keyword.type" and text == "enum_flags" then
              saw_flags = true
            end
          end
          check("`enum_flags` highlights as a keyword", saw_flags)
        end
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- Operator overloading (ADR-0048). Two things the drift gate cannot see: that `operator`
    -- highlights as a *keyword* rather than as reserved — it was never reserved, so unlike `cast`,
    -- `enum`, `union` and `xx` there was nothing to remove, and this pins that it stayed out — and
    -- that an `operator_decl` is a distinct node from a `const_decl`, which a shared rule would
    -- have hidden.
    local op_file = root .. "/tests/corpus/valid/038-operator-overloading.jr"
    if vim.uv.fs_stat(op_file) then
      vim.cmd.edit(vim.fn.fnameescape(op_file))
      local o_buf = vim.api.nvim_get_current_buf()
      local o_ok, o_parser = pcall(vim.treesitter.get_parser, o_buf, "jairs")
      if o_ok and o_parser then
        local o_tree = o_parser:parse()[1]
        check("the operator corpus file parses with no ERROR node", not o_tree:root():has_error())

        local o_kinds = {}
        local function o_walk(node)
          o_kinds[node:type()] = (o_kinds[node:type()] or 0) + 1
          for child in node:iter_children() do
            o_walk(child)
          end
        end
        o_walk(o_tree:root())
        check("`operator + :: …` produces an operator_decl node", (o_kinds.operator_decl or 0) > 0)
        check(
          "an ordinary `name :: …` still produces a const_decl node beside it",
          (o_kinds.const_decl or 0) > 0
        )

        local o_query_ok, o_query = pcall(vim.treesitter.query.get, "jairs", "highlights")
        if o_query_ok and o_query then
          local saw_kw, saw_res = false, false
          for id, node in o_query:iter_captures(o_tree:root(), o_buf, 0, -1) do
            local capture = o_query.captures[id]
            local text = vim.treesitter.get_node_text(node, o_buf)
            if text == "operator" then
              if capture == "keyword" then
                saw_kw = true
              elseif capture == "keyword.reserved" then
                saw_res = true
              end
            end
          end
          check("`operator` highlights as a keyword", saw_kw)
          check("`operator` is not highlighted as reserved", not saw_res)
        end
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- `xx` and bare `.RED` (ADR-0046). Three things the drift gate cannot see: that `xx`
    -- highlights as a keyword rather than as reserved (four for four on that trap now), that
    -- `.RED` produces a `member_expr`, and — the one that actually broke — that `p.x` still
    -- produces a `field_expr`. `member_expr` at precedence 10 won the ambiguity against
    -- `field_expr` and split `dots[1].x` in two; it sits at 1 now, and this pins the ordering
    -- because a wrong *shape* is not an ERROR node.
    local ac_file = root .. "/tests/corpus/valid/037-autocast-and-bare-members.jr"
    if vim.uv.fs_stat(ac_file) then
      vim.cmd.edit(vim.fn.fnameescape(ac_file))
      local a_buf = vim.api.nvim_get_current_buf()
      local a_ok, a_parser = pcall(vim.treesitter.get_parser, a_buf, "jairs")
      if a_ok and a_parser then
        local a_tree = a_parser:parse()[1]
        check("the autocast corpus file parses with no ERROR node", not a_tree:root():has_error())

        local a_kinds = {}
        local function a_walk(node)
          a_kinds[node:type()] = (a_kinds[node:type()] or 0) + 1
          for child in node:iter_children() do
            a_walk(child)
          end
        end
        a_walk(a_tree:root())
        check("`xx expr` produces an autocast_expr node", (a_kinds.autocast_expr or 0) > 0)
        check("`.RED` produces a member_expr node", (a_kinds.member_expr or 0) > 0)

        local a_query_ok, a_query = pcall(vim.treesitter.query.get, "jairs", "highlights")
        if a_query_ok and a_query then
          local saw_kw, saw_res = false, false
          for id, node in a_query:iter_captures(a_tree:root(), a_buf, 0, -1) do
            local capture = a_query.captures[id]
            local text = vim.treesitter.get_node_text(node, a_buf)
            if text == "xx" then
              if capture == "keyword" then
                saw_kw = true
              elseif capture == "keyword.reserved" then
                saw_res = true
              end
            end
          end
          check("`xx` highlights as a keyword", saw_kw)
          check("`xx` is no longer highlighted as reserved", not saw_res)
        end
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- `#scope_module` (ADR-0054). Three things the drift gate cannot see: that a bare
    -- `#scope_module` produces a `scope_decl` node, that `#run x;` is still a `run_decl` and not
    -- one — a `prec(3)` on the scope rule made *every* bare directive a scope marker and stranded
    -- `#run`'s expression — and that the standard library's own private section parses.
    local sc_file = root .. "/modules/Basic/module.jr"
    if vim.uv.fs_stat(sc_file) then
      vim.cmd.edit(vim.fn.fnameescape(sc_file))
      local sc_buf = vim.api.nvim_get_current_buf()
      local sc_ok, sc_parser = pcall(vim.treesitter.get_parser, sc_buf, "jairs")
      if sc_ok and sc_parser then
        local sc_tree = sc_parser:parse()[1]
        check("modules/Basic parses with no ERROR node", not sc_tree:root():has_error())

        local sc_kinds = {}
        local function sc_walk(node)
          sc_kinds[node:type()] = (sc_kinds[node:type()] or 0) + 1
          for child in node:iter_children() do
            sc_walk(child)
          end
        end
        sc_walk(sc_tree:root())
        -- Two markers are written in `Basic`: one `#scope_module` and one `#scope_export`.
        check("`#scope_module` produces a scope_decl node", (sc_kinds.scope_decl or 0) == 2)
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- `#run` must still be a `run_decl`, not a `scope_decl` — the regression a precedence-based
    -- scope rule caused, pinned at the level where it was visible only as an ERROR node.
    local run_file = root .. "/tests/corpus/valid/020-run-directive.jr"
    if vim.uv.fs_stat(run_file) then
      vim.cmd.edit(vim.fn.fnameescape(run_file))
      local rn_buf = vim.api.nvim_get_current_buf()
      local rn_ok, rn_parser = pcall(vim.treesitter.get_parser, rn_buf, "jairs")
      if rn_ok and rn_parser then
        local rn_tree = rn_parser:parse()[1]
        check("the #run corpus file parses with no ERROR node", not rn_tree:root():has_error())
        local rn_kinds = {}
        local function rn_walk(node)
          rn_kinds[node:type()] = (rn_kinds[node:type()] or 0) + 1
          for child in node:iter_children() do
            rn_walk(child)
          end
        end
        rn_walk(rn_tree:root())
        check("`#run` is still a run_decl", (rn_kinds.run_decl or 0) > 0)
        check("`#run` did not become a scope_decl", (rn_kinds.scope_decl or 0) == 0)
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- Named arguments and defaults (ADR-0053). Three things the drift gate cannot see: that
    -- `f(b = 2)` produces a `named_arg` node rather than an expression followed by a stray `=`,
    -- that a parameter's `= 10` survives as a child of `param`, and that an *ordinary* call is
    -- still not a named argument — a grammar that made every argument a `named_arg` would parse the
    -- corpus file cleanly while being wrong.
    local na_file = root .. "/tests/corpus/valid/043-named-and-default-arguments.jr"
    if vim.uv.fs_stat(na_file) then
      vim.cmd.edit(vim.fn.fnameescape(na_file))
      local na_buf = vim.api.nvim_get_current_buf()
      local na_ok, na_parser = pcall(vim.treesitter.get_parser, na_buf, "jairs")
      if na_ok and na_parser then
        local na_tree = na_parser:parse()[1]
        check(
          "the named-arguments corpus file parses with no ERROR node",
          not na_tree:root():has_error()
        )

        local na_kinds = {}
        local function na_walk(node)
          na_kinds[node:type()] = (na_kinds[node:type()] or 0) + 1
          for child in node:iter_children() do
            na_walk(child)
          end
        end
        na_walk(na_tree:root())
        check("`f(b = 2)` produces a named_arg node", (na_kinds.named_arg or 0) > 0)
        -- Twelve named arguments are written in that file. Counted rather than merely detected,
        -- because a grammar accepting the form in only the first position would still produce some.
        check("every named argument produces a node", (na_kinds.named_arg or 0) >= 12)
        -- `add(3, 4)` and the `exit` calls are positional, so not every argument is named — a
        -- grammar that swallowed positional arguments into `named_arg` would fail this.
        check(
          "an ordinary positional argument is not a named_arg",
          (na_kinds.named_arg or 0) < (na_kinds.arg_list or 0) * 4
        )
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- Multiple returns (ADR-0052). Three things the drift gate cannot see: that `-> (s64, bool)`
    -- produces a `result_list` node rather than a parenthesised type, that a destructuring statement
    -- produces a `target_list`, and that a `_` discard is inside one — a grammar admitting the
    -- target list only for real names would still parse the corpus file if `_` happened to lex the
    -- same way, so the *count* is checked.
    local mr_file = root .. "/tests/corpus/valid/042-multiple-returns.jr"
    if vim.uv.fs_stat(mr_file) then
      vim.cmd.edit(vim.fn.fnameescape(mr_file))
      local mr_buf = vim.api.nvim_get_current_buf()
      local mr_ok, mr_parser = pcall(vim.treesitter.get_parser, mr_buf, "jairs")
      if mr_ok and mr_parser then
        local mr_tree = mr_parser:parse()[1]
        check(
          "the multiple-returns corpus file parses with no ERROR node",
          not mr_tree:root():has_error()
        )

        local mr_kinds = {}
        local function mr_walk(node)
          mr_kinds[node:type()] = (mr_kinds[node:type()] or 0) + 1
          for child in node:iter_children() do
            mr_walk(child)
          end
        end
        mr_walk(mr_tree:root())
        check("`-> (s64, bool)` produces a result_list node", (mr_kinds.result_list or 0) > 0)
        check(
          "`q, ok := f()` produces a target_list node",
          (mr_kinds.target_list or 0) > 0
        )
        -- Nine destructuring statements are written in that file. Counting rather than merely
        -- checking presence, because a grammar accepting only the two-target form would still
        -- produce *some* target lists while silently failing on the three-target one.
        check(
          "every destructuring statement produces a target list",
          (mr_kinds.target_list or 0) >= 9
        )
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- `using` (ADR-0050). Three things the drift gate cannot see: that `using` highlights as a
    -- keyword rather than as reserved — seven for seven on that pairing now, and this is the last
    -- keyword to make the trip — that it appears in all three binding positions, and that an
    -- ordinary `hp: s64;` field beside a `using` one is still a plain field.
    local using_file = root .. "/tests/corpus/valid/040-using.jr"
    if vim.uv.fs_stat(using_file) then
      vim.cmd.edit(vim.fn.fnameescape(using_file))
      local us_buf = vim.api.nvim_get_current_buf()
      local us_ok, us_parser = pcall(vim.treesitter.get_parser, us_buf, "jairs")
      if us_ok and us_parser then
        local us_tree = us_parser:parse()[1]
        check("the `using` corpus file parses with no ERROR node", not us_tree:root():has_error())

        local us_query_ok, us_query = pcall(vim.treesitter.query.get, "jairs", "highlights")
        if us_query_ok and us_query then
          local kw, res = 0, false
          for id, node in us_query:iter_captures(us_tree:root(), us_buf, 0, -1) do
            local capture = us_query.captures[id]
            local text = vim.treesitter.get_node_text(node, us_buf)
            if text == "using" then
              if capture == "keyword" then
                kw = kw + 1
              elseif capture == "keyword.reserved" then
                res = true
              end
            end
          end
          -- Six `using` bindings are written in that file: two embedded fields, three parameters
          -- (one of them a pointer), and one local. Counting rather than merely checking presence,
          -- because a grammar that admitted the keyword in only *one* position would still see it.
          check("every `using` highlights as a keyword", kw >= 6)
          check("`using` is no longer highlighted as reserved", not res)
        end
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- `for`, labels, `defer` and ranges (ADR-0049). Four things the drift gate cannot see.
    --
    -- The `n: s64` one is the reason this block exists at all: `loop_label` and an ordinary
    -- typed declaration both begin `identifier ":"`, and resolving that with a `prec(1)` — the
    -- first thing `tree-sitter generate` suggests — made the label rule win *everywhere* and
    -- silently broke every declaration in the corpus. A declared GLR conflict fixes it, and
    -- this pins the fix, because the wrong resolution is a wrong *shape* rather than an ERROR.
    local for_file = root .. "/tests/corpus/valid/039-for-labels-and-defer.jr"
    if vim.uv.fs_stat(for_file) then
      vim.cmd.edit(vim.fn.fnameescape(for_file))
      local fo_buf = vim.api.nvim_get_current_buf()
      local fo_ok, fo_parser = pcall(vim.treesitter.get_parser, fo_buf, "jairs")
      if fo_ok and fo_parser then
        local fo_tree = fo_parser:parse()[1]
        check("the `for` corpus file parses with no ERROR node", not fo_tree:root():has_error())

        local fo_kinds = {}
        local function fo_walk(node)
          fo_kinds[node:type()] = (fo_kinds[node:type()] or 0) + 1
          for child in node:iter_children() do
            fo_walk(child)
          end
        end
        fo_walk(fo_tree:root())
        check("`for x: buf` produces a for_stmt node", (fo_kinds.for_stmt or 0) > 0)
        check("`outer: for` produces a loop_label node", (fo_kinds.loop_label or 0) > 0)
        check("`defer f();` produces a defer_stmt node", (fo_kinds.defer_stmt or 0) > 0)
        check("`0..4` produces a range_expr node", (fo_kinds.range_expr or 0) > 0)
        -- The label rule must not have swallowed ordinary declarations. `buf: [4]s64;` and
        -- `total := 0;` are both in this file, so a `loop_label` count above the number of
        -- labelled loops written (two) means the rule is matching declarations too.
        check(
          "an ordinary `name: T` declaration is not parsed as a loop label",
          (fo_kinds.loop_label or 0) == 2
        )

        local fo_query_ok, fo_query = pcall(vim.treesitter.query.get, "jairs", "highlights")
        if fo_query_ok and fo_query then
          local kw, res, labels = {}, {}, 0
          for id, node in fo_query:iter_captures(fo_tree:root(), fo_buf, 0, -1) do
            local capture = fo_query.captures[id]
            local text = vim.treesitter.get_node_text(node, fo_buf)
            if text == "for" or text == "defer" then
              if capture == "keyword" then
                kw[text] = true
              elseif capture == "keyword.reserved" then
                res[text] = true
              end
            end
            if capture == "label" then
              labels = labels + 1
            end
          end
          -- Five for five on this pairing now: `cast`, `enum`, `union`, `xx`, and these two.
          check("`for` highlights as a keyword", kw["for"] == true)
          check("`defer` highlights as a keyword", kw["defer"] == true)
          check("`for` is no longer highlighted as reserved", not res["for"])
          check("`defer` is no longer highlighted as reserved", not res["defer"])
          -- Two labelled loops, and two jumps that name one.
          check("a loop label and the jump naming it both highlight", labels >= 4)
        end
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- A **void-returning** procedure pointer, `(T)` with no arrow (ADR-0062 §1). This is the one
    -- shape where the grammar's ambiguity is genuine rather than a look-ahead: `-> (s64)` is both a
    -- one-element results list and a void-returning proc pointer, resolved by a declared GLR
    -- conflict. A regression that picked the wrong reading is a wrong *shape*, not an ERROR node, so
    -- gate 6 would stay green — which is why the node kinds are counted here.
    local al_file = root .. "/tests/corpus/valid/050-allocator.jr"
    if vim.uv.fs_stat(al_file) then
      vim.cmd.edit(vim.fn.fnameescape(al_file))
      local al_buf = vim.api.nvim_get_current_buf()
      local al_ok, al_parser = pcall(vim.treesitter.get_parser, al_buf, "jairs")
      if al_ok and al_parser then
        local al_tree = al_parser:parse()[1]
        check("the allocator corpus file parses with no ERROR node", not al_tree:root():has_error())

        local al_kinds = {}
        local function al_walk(node)
          al_kinds[node:type()] = (al_kinds[node:type()] or 0) + 1
          for child in node:iter_children() do
            al_walk(child)
          end
        end
        al_walk(al_tree:root())
        -- That file declares `libc_free :: (p: *u8)` and `free_through_context :: (p: *u8)` — both
        -- void-returning *declarations*, not proc-pointer types — so the `proc_type` count here
        -- comes only from what the file spells as a type. It spells none: the allocator's fields
        -- live in the compiler-declared context, not in source. So this asserts the absence, which
        -- is what tells a reader the context's fields are not user syntax.
        check("no proc_type node is spelled in the allocator file", (al_kinds.proc_type or 0) == 0)
        -- And a results list is not accidentally produced by a void-returning declaration.
        check("a void-returning declaration is not a results list", (al_kinds.result_list or 0) == 0)
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- `null` (ADR-0060). The drift gate confirms it parses as a `null` node; what it cannot see is
    -- that the formatter kept it (it lands in a literal filter that a missing arm would delete
    -- from, which happened in this wave and a unit test now pins) and that the highlighter colours
    -- it. Here we assert the node kind and the highlight, since the reserved-keyword rule that used
    -- to match `null` as an identifier is now dead.
    local nl_file = root .. "/tests/corpus/valid/049-null-and-malloc.jr"
    if vim.uv.fs_stat(nl_file) then
      vim.cmd.edit(vim.fn.fnameescape(nl_file))
      local nl_buf = vim.api.nvim_get_current_buf()
      local nl_ok, nl_parser = pcall(vim.treesitter.get_parser, nl_buf, "jairs")
      if nl_ok and nl_parser then
        local nl_tree = nl_parser:parse()[1]
        check("the null/malloc corpus file parses with no ERROR node", not nl_tree:root():has_error())

        local nl_kinds = {}
        local function nl_walk(node)
          nl_kinds[node:type()] = (nl_kinds[node:type()] or 0) + 1
          for child in node:iter_children() do
            nl_walk(child)
          end
        end
        nl_walk(nl_tree:root())
        -- `null` appears several times in that file; a presence check suffices, because the point
        -- is that it is a `null` node at all rather than a `name_expr` an identifier rule matched.
        check("`null` produces a null node", (nl_kinds.null or 0) >= 1)
        -- And it must NOT be a name_expr whose text is `null` — the old reserved-identifier match.
        local saw_as_name = false
        local function name_walk(node)
          if node:type() == "name_expr"
            and vim.treesitter.get_node_text(node, nl_buf) == "null" then
            saw_as_name = true
          end
          for child in node:iter_children() do
            name_walk(child)
          end
        end
        name_walk(nl_tree:root())
        check("`null` is not parsed as an ordinary name", not saw_as_name)
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- Procedure-pointer types (ADR-0059). Two things the drift gate cannot see.
    --
    -- The return-position one is why this block exists. A `proc_type` and a `result_list` both begin
    -- `(` in return position, and only the `->` after the `)` tells them apart — the compiler's
    -- parser looks that far ahead by hand, tree-sitter's GLR explores both. A regression that made
    -- `pick :: () -> (s64, s64) -> s64` parse its return as a results list is a wrong *shape*, not
    -- an ERROR node, so it would pass gate 6 while meaning something else.
    local pp_file = root .. "/tests/corpus/valid/048-indirect-calls.jr"
    if vim.uv.fs_stat(pp_file) then
      vim.cmd.edit(vim.fn.fnameescape(pp_file))
      local pp_buf = vim.api.nvim_get_current_buf()
      local pp_ok, pp_parser = pcall(vim.treesitter.get_parser, pp_buf, "jairs")
      if pp_ok and pp_parser then
        local pp_tree = pp_parser:parse()[1]
        check(
          "the indirect-call corpus file parses with no ERROR node",
          not pp_tree:root():has_error()
        )

        local pp_kinds = {}
        local function pp_walk(node)
          pp_kinds[node:type()] = (pp_kinds[node:type()] or 0) + 1
          for child in node:iter_children() do
            pp_walk(child)
          end
        end
        pp_walk(pp_tree:root())
        -- Two `proc_type`s are written: `apply`'s `fn` parameter and `pick`'s return type. Counting
        -- rather than merely checking presence, because the two are in *different* positions — a
        -- parameter and a return — and the return one goes through the results-list disambiguation,
        -- so a grammar that handled only the parameter position would still see one.
        check("`(T, T) -> T` produces a proc_type node", (pp_kinds.proc_type or 0) == 2)
        -- And `pick`'s `(s64, s64)` return must **not** have been read as a results list: that is
        -- the silent wrong-shape regression, so its absence is asserted directly.
        check(
          "a proc-pointer return is not parsed as a results list",
          (pp_kinds.result_list or 0) == 0
        )
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- `context` and `#c_call` (ADR-0057). This block exists because the wave shipped with the
    -- grammar knowing neither, and the *two* failures were of different kinds:
    --
    --   * `#c_call` was an ERROR node, which gate 6 caught;
    --   * `context` was **not**. It is a legal identifier, so the corpus parsed and
    --     `context.allocator` was a `field_expr` over a `name_expr` — a field access on a variable
    --     nobody declared. The two parsers disagreed about what the tree *meant* while every gate
    --     stayed green, which is the failure ADR-0025 §4 added the gate for and the one it cannot
    --     see. So the check here is on the node type, not on the absence of an error.
    local ctx_file = root .. "/tests/corpus/valid/046-context.jr"
    if vim.uv.fs_stat(ctx_file) then
      vim.cmd.edit(vim.fn.fnameescape(ctx_file))
      local cx_buf = vim.api.nvim_get_current_buf()
      local cx_ok, cx_parser = pcall(vim.treesitter.get_parser, cx_buf, "jairs")
      if cx_ok and cx_parser then
        local cx_tree = cx_parser:parse()[1]
        check("the context corpus file parses with no ERROR node", not cx_tree:root():has_error())

        local cx_kinds = {}
        local function cx_walk(node)
          cx_kinds[node:type()] = (cx_kinds[node:type()] or 0) + 1
          for child in node:iter_children() do
            cx_walk(child)
          end
        end
        cx_walk(cx_tree:root())
        -- Five `context` mentions are written in that file: `read_allocator`'s read, `bumped`'s
        -- write, and three in `main` (two reads and a write). Counted rather than merely present,
        -- because the keyword-versus-identifier ambiguity could resolve one way in one position and
        -- the other way elsewhere, and a presence check would see the good case only.
        check("`context` produces a context_expr node", (cx_kinds.context_expr or 0) == 5)
        check("`#c_call` produces a c_call_attr node", (cx_kinds.c_call_attr or 0) == 1)
        -- And `context` must **not** have been read as a name. A `name_expr` whose text is
        -- `context` is exactly the silent divergence above, so this asserts its absence directly
        -- rather than inferring it from the count.
        local saw_as_name = false
        local function name_walk(node)
          if node:type() == "name_expr"
            and vim.treesitter.get_node_text(node, cx_buf) == "context" then
            saw_as_name = true
          end
          for child in node:iter_children() do
            name_walk(child)
          end
        end
        name_walk(cx_tree:root())
        check("`context` is not parsed as an ordinary name", not saw_as_name)

        local cx_query_ok, cx_query = pcall(vim.treesitter.query.get, "jairs", "highlights")
        if cx_query_ok and cx_query then
          local kw, res, directive = 0, false, false
          for id, node in cx_query:iter_captures(cx_tree:root(), cx_buf, 0, -1) do
            local capture = cx_query.captures[id]
            local text = vim.treesitter.get_node_text(node, cx_buf)
            if text == "context" then
              if capture == "keyword" then
                kw = kw + 1
              elseif capture == "keyword.reserved" then
                res = true
              end
            end
            if text == "#c_call" and capture == "keyword.directive" then
              directive = true
            end
          end
          check("every `context` highlights as a keyword", kw == 5)
          check("`context` is not highlighted as reserved", not res)
          -- `#c_call` is a literal token rather than a `(directive)` node, so the general directive
          -- rule does not reach it: without its own capture it would have no colour at all, which
          -- is a silent loss rather than a failure.
          check("`#c_call` highlights as a directive", directive)
        end
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- `push_context` (ADR-0063). The same failure mode as `context` above, and checked the same way:
    -- `push_context` is a legal identifier, so omitting the grammar rule would not produce an ERROR
    -- node — it would parse as a `name_expr` followed by a stray block, and the keyword would
    -- mis-colour as a variable while every gate stayed green. So this asserts the node type
    -- (`push_context_stmt`), the absence of a `name_expr` reading, and the keyword highlight.
    local pc_file = root .. "/tests/corpus/valid/051-push-context.jr"
    if vim.uv.fs_stat(pc_file) then
      vim.cmd.edit(vim.fn.fnameescape(pc_file))
      local pc_buf = vim.api.nvim_get_current_buf()
      local pc_ok, pc_parser = pcall(vim.treesitter.get_parser, pc_buf, "jairs")
      if pc_ok and pc_parser then
        local pc_tree = pc_parser:parse()[1]
        check("the push_context corpus file parses with no ERROR node", not pc_tree:root():has_error())

        local pc_kinds = {}
        local function pc_walk(node)
          pc_kinds[node:type()] = (pc_kinds[node:type()] or 0) + 1
          for child in node:iter_children() do
            pc_walk(child)
          end
        end
        pc_walk(pc_tree:root())
        -- Exactly one `push_context` block is written in that file (in `main`).
        check("`push_context` produces a push_context_stmt node", (pc_kinds.push_context_stmt or 0) == 1)

        -- And `push_context` must not have been read as a name — the silent divergence the `context`
        -- check guards against, here for the keyword introduced this wave.
        local pc_saw_as_name = false
        local function pc_name_walk(node)
          if node:type() == "name_expr"
            and vim.treesitter.get_node_text(node, pc_buf) == "push_context" then
            pc_saw_as_name = true
          end
          for child in node:iter_children() do
            pc_name_walk(child)
          end
        end
        pc_name_walk(pc_tree:root())
        check("`push_context` is not parsed as an ordinary name", not pc_saw_as_name)

        local pc_query_ok, pc_query = pcall(vim.treesitter.query.get, "jairs", "highlights")
        if pc_query_ok and pc_query then
          local pc_kw, pc_res = false, false
          for id, node in pc_query:iter_captures(pc_tree:root(), pc_buf, 0, -1) do
            local capture = pc_query.captures[id]
            local text = vim.treesitter.get_node_text(node, pc_buf)
            if text == "push_context" then
              if capture == "keyword" then
                pc_kw = true
              elseif capture == "keyword.reserved" then
                pc_res = true
              end
            end
          end
          check("`push_context` highlights as a keyword", pc_kw)
          check("`push_context` is not highlighted as reserved", not pc_res)
        end
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- `switch` and `case` (ADR-0067). Same failure mode as `context` and `push_context`: both are
    -- legal identifiers, so omitting the grammar rules would not produce an ERROR node — a `switch`
    -- would parse as a name followed by a stray block, and the keywords would mis-colour as variables
    -- while every gate stayed green. So this checks the node types and the keyword highlights.
    local sw_file = root .. "/tests/corpus/valid/054-switch.jr"
    if vim.uv.fs_stat(sw_file) then
      vim.cmd.edit(vim.fn.fnameescape(sw_file))
      local sw_buf = vim.api.nvim_get_current_buf()
      local sw_ok, sw_parser = pcall(vim.treesitter.get_parser, sw_buf, "jairs")
      if sw_ok and sw_parser then
        local sw_tree = sw_parser:parse()[1]
        check("the switch corpus file parses with no ERROR node", not sw_tree:root():has_error())

        local sw_kinds = {}
        local function sw_walk(node)
          sw_kinds[node:type()] = (sw_kinds[node:type()] or 0) + 1
          for child in node:iter_children() do
            sw_walk(child)
          end
        end
        sw_walk(sw_tree:root())
        -- Four switches in that file: describe, describe_qualified, classify, from_call.
        check("tree-sitter produces switch_stmt", (sw_kinds.switch_stmt or 0) == 4)
        -- Thirteen arms: 3 + 3 + 3 (two cases and an else) + 3 (two cases and an else) = 12, plus none
        -- stray. Counted rather than merely present, because an arm rule that swallowed the next arm
        -- would still produce *some* switch_arm nodes.
        check("tree-sitter produces one switch_arm per arm", (sw_kinds.switch_arm or 0) == 12)

        -- And neither keyword may be read as an ordinary name.
        local sw_as_name = false
        local function sw_name_walk(node)
          if node:type() == "name_expr" then
            local text = vim.treesitter.get_node_text(node, sw_buf)
            if text == "switch" or text == "case" then
              sw_as_name = true
            end
          end
          for child in node:iter_children() do
            sw_name_walk(child)
          end
        end
        sw_name_walk(sw_tree:root())
        check("`switch`/`case` are not parsed as ordinary names", not sw_as_name)

        local sw_q_ok, sw_q = pcall(vim.treesitter.query.get, "jairs", "highlights")
        if sw_q_ok and sw_q then
          local saw_switch, saw_case = false, false
          for id, node in sw_q:iter_captures(sw_tree:root(), sw_buf, 0, -1) do
            local capture = sw_q.captures[id]
            local text = vim.treesitter.get_node_text(node, sw_buf)
            if capture == "keyword" and text == "switch" then
              saw_switch = true
            end
            if capture == "keyword" and text == "case" then
              saw_case = true
            end
          end
          check("`switch` highlights as a keyword", saw_switch)
          check("`case` highlights as a keyword", saw_case)
        end
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- `#no_abc` (ADR-0058 §3). Two things the drift gate cannot see.
    --
    -- The **ordering** one is why this block exists. `#c_call` and `#no_abc` are legal in either
    -- order, and a grammar using two `optional`s in a fixed order would make one spelling an ERROR
    -- node while the other parsed — so gate 6 *would* catch that. What it cannot catch is the
    -- formatter reordering them, which is not an error at all: `jr fmt` would rewrite one legal
    -- spelling into the other and gate 5 would fail only if a corpus file happened to use the
    -- second. So the corpus file uses `#no_abc #c_call`, and this asserts the tree keeps that order.
    local abc_file = root .. "/tests/corpus/valid/047-no-abc.jr"
    if vim.uv.fs_stat(abc_file) then
      vim.cmd.edit(vim.fn.fnameescape(abc_file))
      local ab_buf = vim.api.nvim_get_current_buf()
      local ab_ok, ab_parser = pcall(vim.treesitter.get_parser, ab_buf, "jairs")
      if ab_ok and ab_parser then
        local ab_tree = ab_parser:parse()[1]
        check("the `#no_abc` corpus file parses with no ERROR node", not ab_tree:root():has_error())

        local ab_kinds = {}
        local function ab_walk(node)
          ab_kinds[node:type()] = (ab_kinds[node:type()] or 0) + 1
          for child in node:iter_children() do
            ab_walk(child)
          end
        end
        ab_walk(ab_tree:root())
        -- Three `#no_abc` procedures are written in that file: `read_fast`, `sum_fast` and
        -- `raw_fast`. Counted, because the directive appears in three different shapes — with a
        -- return type, on a procedure that loops, and beside `#c_call` — and a grammar that
        -- admitted it in only one position would still see one.
        check("`#no_abc` produces a no_abc_attr node", (ab_kinds.no_abc_attr or 0) == 3)
        check("`#c_call` still parses beside it", (ab_kinds.c_call_attr or 0) == 1)

        -- The order, read off the tree. `raw_fast` is declared `#no_abc #c_call`, so the first
        -- attribute child of that `proc` must be the `no_abc_attr` — if the formatter or the
        -- grammar reordered them, this is where it shows.
        local first_of_pair = nil
        local function order_walk(node)
          if node:type() == "proc" then
            local attrs = {}
            for child in node:iter_children() do
              local t = child:type()
              if t == "no_abc_attr" or t == "c_call_attr" then
                attrs[#attrs + 1] = t
              end
            end
            if #attrs == 2 then
              first_of_pair = attrs[1]
            end
          end
          for child in node:iter_children() do
            order_walk(child)
          end
        end
        order_walk(ab_tree:root())
        check(
          "`#no_abc #c_call` keeps the order it was written in",
          first_of_pair == "no_abc_attr",
          first_of_pair
        )

        local ab_query_ok, ab_query = pcall(vim.treesitter.query.get, "jairs", "highlights")
        if ab_query_ok and ab_query then
          local coloured = 0
          for id, node in ab_query:iter_captures(ab_tree:root(), ab_buf, 0, -1) do
            if ab_query.captures[id] == "keyword.directive"
              and vim.treesitter.get_node_text(node, ab_buf) == "#no_abc" then
              coloured = coloured + 1
            end
          end
          -- A literal token, so the general `(directive)` rule does not reach it: without its own
          -- capture it would have no colour at all, which is a silent loss rather than a failure.
          check("every `#no_abc` highlights as a directive", coloured == 3, coloured)
        end
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- A field access must still be a `field_expr`, not a bare member. This is the regression
    -- `member_expr` caused at precedence 10, pinned at the level where it was invisible: the
    -- corpus still parsed, so only the *shape* showed it.
    local arrays_file = root .. "/tests/corpus/valid/030-arrays.jr"
    if vim.uv.fs_stat(arrays_file) then
      vim.cmd.edit(vim.fn.fnameescape(arrays_file))
      local fa_buf = vim.api.nvim_get_current_buf()
      local fa_ok, fa_parser = pcall(vim.treesitter.get_parser, fa_buf, "jairs")
      if fa_ok and fa_parser then
        local fa_tree = fa_parser:parse()[1]
        local fa_kinds = {}
        local function fa_walk(node)
          fa_kinds[node:type()] = (fa_kinds[node:type()] or 0) + 1
          for child in node:iter_children() do
            fa_walk(child)
          end
        end
        fa_walk(fa_tree:root())
        check("`dots[1].x` is a field_expr", (fa_kinds.field_expr or 0) > 0)
        check(
          "no bare member appears where a field access was written",
          (fa_kinds.member_expr or 0) == 0
        )
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- `union`, promoted out of the reserved-keyword match (ADR-0045). Two things the drift
    -- gate cannot see: that `union` now highlights as a *keyword* rather than as reserved —
    -- colouring a live feature "arrives in a later wave" is the trap `cast` and `enum` both hit
    -- — and that `union_type` is a distinct node kind from `struct_type`, which a shared rule
    -- would have hidden.
    local unions_file = root .. "/tests/corpus/valid/036-unions.jr"
    if vim.uv.fs_stat(unions_file) then
      vim.cmd.edit(vim.fn.fnameescape(unions_file))
      local u_buf = vim.api.nvim_get_current_buf()
      local u_ok, u_parser = pcall(vim.treesitter.get_parser, u_buf, "jairs")
      if u_ok and u_parser then
        local u_tree = u_parser:parse()[1]
        check("the unions corpus file parses with no ERROR node", not u_tree:root():has_error())

        local u_kinds = {}
        local function u_walk(node)
          u_kinds[node:type()] = (u_kinds[node:type()] or 0) + 1
          for child in node:iter_children() do
            u_walk(child)
          end
        end
        u_walk(u_tree:root())
        check("`union { … }` produces a union_type node", (u_kinds.union_type or 0) > 0)
        check(
          "`struct { … }` still produces a struct_type node beside it",
          (u_kinds.struct_type or 0) > 0
        )

        local u_query_ok, u_query = pcall(vim.treesitter.query.get, "jairs", "highlights")
        if u_query_ok and u_query then
          local saw_keyword, saw_reserved = false, false
          for id, node in u_query:iter_captures(u_tree:root(), u_buf, 0, -1) do
            local capture = u_query.captures[id]
            local text = vim.treesitter.get_node_text(node, u_buf)
            if text == "union" then
              if capture == "keyword" then
                saw_keyword = true
              elseif capture == "keyword.reserved" then
                saw_reserved = true
              end
            end
          end
          check("`union` highlights as a keyword", saw_keyword)
          check("`union` is no longer highlighted as reserved", not saw_reserved)
        end
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- `[]T` views. The drift gate proves the file *parses*; what it cannot see is that the
    -- two bracket forms produce two different node kinds. `[]s64` must be a `view_type` and
    -- `[4]s64` an `array_type`, and `buf[]` a `slice_expr` rather than an `index_expr` with a
    -- missing subscript — a grammar that reused one rule for both would parse the corpus
    -- cleanly and give a query no way to tell a view from an array (ADR-0044 §1).
    local views_file = root .. "/tests/corpus/valid/035-views.jr"
    if vim.uv.fs_stat(views_file) then
      vim.cmd.edit(vim.fn.fnameescape(views_file))
      local v_buf = vim.api.nvim_get_current_buf()
      local v_ok, v_parser = pcall(vim.treesitter.get_parser, v_buf, "jairs")
      if v_ok and v_parser then
        local v_tree = v_parser:parse()[1]
        check("the views corpus file parses with no ERROR node", not v_tree:root():has_error())

        local kinds = {}
        local function walk(node)
          kinds[node:type()] = (kinds[node:type()] or 0) + 1
          for child in node:iter_children() do
            walk(child)
          end
        end
        walk(v_tree:root())
        check("`[]T` produces a view_type node", (kinds.view_type or 0) > 0)
        check("`buf[]` produces a slice_expr node", (kinds.slice_expr or 0) > 0)
        check(
          "`[N]T` still produces an array_type node beside them",
          (kinds.array_type or 0) > 0
        )
        check("`xs[i]` still produces an index_expr node", (kinds.index_expr or 0) > 0)
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
    end

    -- Bitwise operators. The *shape* is what matters and the drift gate cannot see it:
    -- `a & 3 == 2` must group as `(a & 3) == 2`, which is Jairs's ordering and not C's
    -- (ADR-0042 §1). A grammar that used C's precedence would parse without an ERROR node
    -- and produce a different tree, so this asserts the nesting.
    local bitwise_file = root .. "/tests/corpus/valid/033-bitwise.jr"
    if vim.uv.fs_stat(bitwise_file) then
      vim.cmd.edit(vim.fn.fnameescape(bitwise_file))
      local w_buf = vim.api.nvim_get_current_buf()
      local w_ok, w_parser = pcall(vim.treesitter.get_parser, w_buf, "jairs")
      if w_ok and w_parser then
        local w_tree = w_parser:parse()[1]
        check(
          "the bitwise corpus file parses with no ERROR node",
          not w_tree:root():has_error()
        )
      end
      -- The precedence itself, on a minimal buffer so the assertion is about one expression.
      local prec_buf = vim.api.nvim_create_buf(false, true)
      vim.api.nvim_buf_set_lines(prec_buf, 0, -1, false, { "main :: () { x := a & 3 == 2; }" })
      vim.bo[prec_buf].filetype = "jairs"
      local p_ok, p_parser = pcall(vim.treesitter.get_parser, prec_buf, "jairs")
      if p_ok and p_parser then
        local p_tree = p_parser:parse()[1]
        -- Find the outermost binary_expr and check its *left* child is also one: that is
        -- `(a & 3) == 2`. Under C's ordering the *right* child would be the binary one.
        local outer = nil
        local function findbin(node)
          if node:type() == "binary_expr" and outer == nil then
            outer = node
            return
          end
          for child in node:iter_children() do
            findbin(child)
          end
        end
        findbin(p_tree:root())
        local left_is_binary = outer ~= nil
          and outer:field("lhs")[1] ~= nil
          and outer:field("lhs")[1]:type() == "binary_expr"
        check("`a & 3 == 2` groups as `(a & 3) == 2`, not C's grouping", left_is_binary)
      end
      vim.cmd.edit(vim.fn.fnameescape(sample))
      buf = vim.api.nvim_get_current_buf()
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
