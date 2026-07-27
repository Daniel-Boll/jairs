# ADR-0025: Editor integration as a runtimepath directory, verified rather than gated

- **Status:** Accepted
- **Date:** 2026-07-27
- **Deciders:** dboll

## Context

ADR-0024 built `jr lsp`, and `crates/jr-cli/tests/lsp_stdio.rs` proved it speaks the
protocol. Nothing made an editor *use* it. `PLAN.md` §1.4 has two editor boxes — VS Code
diagnostics/hover/goto-def, and Neovim tree-sitter highlighting — and the second one has
read "grammar and queries exist and the drift gate is green; editor packaging is not
done" since the tree-sitter wave.

**This ADR was written after the code, deliberately, and that is a change from this
project's rhythm.** The rhythm exists because design forks are expensive to undo, and
four consecutive ADRs have had a claim corrected by reading a dump or a file. For a
*packaging* wave the forks are cheap to undo and the facts are all empirical: whether
Neovim discovers a config in `lsp/`, whether it loads `parser/jairs.so` without a
registration call, whether `-u NONE` sources `ftdetect`. Guessing those into an ADR and
then discovering them would have produced a fifth correction. So the decisions below are
recorded having been run.

Four facts decided the shape, and every one was verified against Neovim 0.12-dev rather
than assumed.

**Neovim needs no plugin for any of this.** 0.11+ discovers `lsp/<name>.lua` anywhere on
the runtimepath, loads `parser/<lang>.so` with no registration call, reads
`queries/<lang>/*.scm`, sources `ftdetect/` when filetype detection is on, and sources
`ftplugin/<ft>.lua` per buffer. A directory with those five subdirectories is a complete
integration.

**`src/parser.c` is generated and git-ignored**, and `.gitignore` explains why
`src/scanner.c` is not: nested block comments cannot be expressed as a token regex, so
the scanner is hand-written. So a parser has to be built locally, which is a script
rather than a committed binary.

**Nothing validates the queries.** The corpus-drift gate runs `tree-sitter generate` and
`tree-sitter parse`. A `highlights.scm` referencing a node the grammar does not have
would have shipped, and highlighting would silently stop working. `tree-sitter query`
exits 1 with `Invalid node type` on exactly that, which was confirmed by adding a bogus
node and watching it fail.

**A relative `--module-path` silently broke cross-file goto-definition.** A `Location`
needs a `file:` URI; `jr_lsp::uri::from_path` correctly refuses a relative path; so the
handler returned `None` and the editor showed "nothing here". Found by starting the real
server with `--module-path modules`, which is what a person types first.

## Decision

### 1. `editors/nvim/` is a runtimepath directory, not a plugin

Five subdirectories, each the location Neovim already looks in: `lsp/jairs.lua`,
`parser/jairs.so`, `queries/jairs/*.scm`, `ftdetect/jairs.lua`, `ftplugin/jairs.lua`.
Setup is two lines in `init.lua` — append the path, call `vim.lsp.enable("jairs")` — and
requires Neovim 0.11 or newer.

**Rejected: an `nvim-lspconfig` entry.** It is how most language servers are configured
and it works on older Neovim. Rejected because it makes a plugin a prerequisite for
trying the compiler, and because upstreaming a config for a language nobody has is a
patch to someone else's repository that this project cannot land or version. The README
says which two lines to change if a reader wants lspconfig instead.

**Rejected: an `nvim-treesitter` parser entry.** Same objection, plus `nvim-treesitter`
would fetch and build the grammar from a git remote — and the whole point of building it
from this checkout is that the parser matches the `grammar.js` in the working tree.

### 2. The queries are symlinks, so they cannot drift

`editors/nvim/queries/jairs/highlights.scm` is a symlink to
`tree-sitter-jairs/queries/highlights.scm`, and likewise for folds, indents and locals.

Copying them would create a second source of truth for the thing the drift gate exists to
protect, in a project that has already lost a wave to a doc claiming something the code
had changed. A symlink makes the duplication unrepresentable rather than merely
discouraged.

**Rejected: put `tree-sitter-jairs/queries` on the runtimepath directly.** No copy and no
symlink. It does not work: Neovim wants `queries/<lang>/highlights.scm`, and the
grammar's layout is `queries/highlights.scm` — there is no `jairs` directory to find.

**Rejected: generate the query files at setup time.** A `setup()` that wrote files would
make an editor config mutate the repository, and the failure mode is a stale generated
file that looks hand-written.

### 3. The parser is built by a script, at the version the drift gate pins

`editors/nvim/build.sh` runs `tree-sitter generate` at `tree-sitter-cli@0.26.11` — the
same version §6's gate uses, so a parser built for an editor and a parse checked in CI
cannot disagree about the grammar — then compiles `parser.c` and `scanner.c` into
`parser/jairs.so`. The `.so` is git-ignored.

**Rejected: commit the `.so`.** It is a platform-specific binary that would be wrong for
every reader on another architecture, and stale the moment `grammar.js` changes.

**Rejected: commit the generated `parser.c`.** `.gitignore` already argues this: all
generated output is regenerated by the gate, and only hand-written sources are tracked.

### 4. Query validation joins the corpus-drift gate

The gate now runs `tree-sitter query` over each of the four query files as well as
`tree-sitter parse` over the corpus. This closes a real hole rather than adding
thoroughness: a query naming a node the grammar does not have was previously
undetectable, and the failure it produces is *silent* — highlighting simply stops.

### 5. `jr lsp` absolutises its search paths

Once, at startup. A server's working directory is whatever the editor happened to have,
so a relative search path is meaningless to it — and the failure was silent rather than
loud. `a_relative_module_path_still_resolves_across_an_import` pins it.

**Rejected: make `uri::from_path` accept a relative path.** It would produce a URI
relative to nothing, which is worse: the editor would jump somewhere wrong instead of
nowhere. Refusing is right; having a relative path at all is the bug.

### 6. The Neovim integration is verified by a script, and not by a CI gate

`editors/nvim/verify.lua` drives the real Neovim against the real server: filetype,
parser, every highlight capture it depends on, LSP attach, the negotiated position
encoding, two hovers asserted by *text*, goto-definition across an `#import`, and a
diagnostic on a deliberately broken file. Twenty-two checks, exiting non-zero on the
first failure. It is run with one command and named in the README.

It is **not** one of the six gates, because Neovim is not a build dependency of this
workspace and making it one would fail `cargo test` on a machine with no editor
installed. `PLAN.md` §1.5 records the consequence in those words rather than letting a
reader assume CI covers it.

**Rejected: assert on the Lua in a `cargo test`.** A test that parsed `lsp/jairs.lua`
without running Neovim would check that the file says what it says. The three failures
this script actually caught on its first run — filetype detection off under `-u NONE`, a
hover asserted at a *declaration* where the correct answer is no hover, and the relative
`--module-path` bug — were all things only a running editor could show.

**Rejected: no verification, just a README.** Instructions nobody has executed are the
same shape as the plan claims ADR-0024 had to correct.

## Consequences

### Positive

- §1.4's Neovim box is tickable, and it is the first of the three to close in four waves.
- Trying the compiler in an editor is `cargo build`, one script, two lines of config.
- The queries have exactly one source of truth, enforced by the filesystem.
- The query gate closes a silent-failure hole that predates this wave.
- A silent goto-definition bug is fixed and pinned.

### Negative

- Neovim 0.11+ only. A reader on 0.10 needs `nvim-lspconfig`, and the README says so
  rather than pretending otherwise.
- The parser must be rebuilt after a `grammar.js` change, and `ftplugin` starts
  tree-sitter under `pcall`, so forgetting is *silent* — no highlighting, no error.
  `:checkhealth vim.treesitter` is the answer and the README says which command.
- Symlinks are awkward on Windows, which this project does not support anyway
  (`jr-lsp`'s `uri` module refuses Windows paths with a `compile_error!`).
- Editor integration is verified on one machine and one Neovim version. That is one more
  machine than before, and less than a gate.
- VS Code still has nothing.

### Follow-on work this forces

- **A VS Code extension**, which is the remaining half of §1.4's first box.
- **A Linux x86-64 CI run**, still the last platform criterion.
- **Into wave W9:** `foldexpr` and `indentexpr` wiring, and the capabilities this server
  does not advertise — completion, rename, references, inlay hints.
- **Into whichever wave has Neovim in CI:** promoting `verify.lua` from verified to
  gated. It exits non-zero already, so the work is the runner and not the script.

## Alternatives considered

Each fork's rejected alternatives are argued at its own point of decision. One
alternative spans the whole ADR.

**Ship nothing and document the LSP's existence.** ADR-0024 built a server that
`lsp_stdio.rs` proves speaks the protocol, so a reader could wire it up themselves. It is
rejected on this project's own evidence: `PLAN.md` §7 spent three waves asserting that
what remained was "packaging", and the reason nobody noticed `jr-lsp` was an empty crate
is that no one had tried to *use* it. An integration that has been run is the only kind
that tells you the thing works.
