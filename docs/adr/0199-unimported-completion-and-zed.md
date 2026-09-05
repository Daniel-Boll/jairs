# ADR-0199: Completion of unimported names, formatting over LSP, and a Zed extension

- **Status:** Accepted
- **Date:** 2026-09-04
- **Deciders:** dboll
- **Amends:** ADR-0028 §5 (the completion source list), ADR-0025 §3 (the generated parser),
  ADR-0036 §3 (packaging for a second editor), ADR-0033 §3 (the declined exported-name index)

## Context

Typing `create_window` in a file that has not imported `modules/Window` offered **nothing**. The
editor was silent about exactly the name a person was reaching for, and the workaround — knowing which
module exports it, writing the `#import` by hand, then typing the name — is the work an editor exists
to remove.

ADR-0028 §5 fixed the candidate sources as "locals and parameters in scope, then file items, then
imported module items, then keywords and builtin types". Unimported names were outside that list by
design, so this is a scope extension rather than a bug fix. ADR-0033 §3 had separately **declined** an
exported-name index pending a latency measurement, calling auto-import "the most keystroke-adjacent
claimant" — an argument that is stronger for completion, which fires far more often.

## Decision

### §1. `jr lsp` gets the bundled module directory, and the feature was inert without it

**The prerequisite was not in the completion path at all.** `jr lsp` was the one subcommand of six
that never pushed `bundled_module_dir()` — `check`, `run`, `build` and `bench` (twice) all do. With no
explicit `--module-path`, the server's search paths were **empty**, and `module_file` probes only the
search paths, so it could resolve no `#import` whatsoever.

The existing auto-import quick fix was therefore already dead in a default invocation, and its failure
was silent: an absent offer reads as "there is nothing to import". It worked in the one editor shipping
a config here only because `editors/nvim` passes `--module-path` explicitly.

`jr lsp` also gains the `-I` short form every other subcommand has. It was the only one taking a module
path without it, so the form a reader had already learned failed there alone.

### §2. `module_name_of` moves into `jr-db`

It was `jr-lsp`'s renderer, with four callers, and `jr-db` now needs it too. It belongs beside
`module_file`, whose probe it inverts: that function turns `Basic` into `<dir>/Basic/module.jr`, and
this turns either spelling back into `Basic`. Two copies disagreeing about that correspondence would
make `#import "X";` import something other than the file a feature had inspected.

### §3. `module_index`, a query over two inputs

ADR-0029:15-17 recorded that "nothing enumerates". This is the index, and its shape is forced: a
directory walk is untracked I/O and **must not live in a query** (ADR-0029 §2), so it derives from
`ModuleSearchPaths` and `WorkspaceFiles`, both already walked outside the database. Invalidation is
then free and correct, and editing one module re-runs that module's `file_exports` leg alone.

A discovered path becomes a module only when `module_name_of` gives a name that `module_file` resolves
**back to that very path** — the round-trip the quick fix has always done. Without it a `helpers.jr`
outside every search path would be offered as `#import "helpers";`, resolving to nothing or to a
different file of that name.

### §4. Completion joins `needs_whole_workspace`

For the reason that list's own comment already gives for `codeAction`: discovery yields paths, and
`file_exports` needs a loaded `SourceFile`. Without it a keystroke sees only the files the editor
happens to have open, and the offer is missing rather than wrong.

The **input** rather than the list is threaded into the job, because that is what the query is keyed
on — and it travels like `ModuleSearchPaths` for the same reason: both are created once and re-`set`
thereafter, so the id stays valid across every snapshot.

### §5. Every unimported item carries its own import, and sorts last

`additional_text_edits` is the LSP's "and also change this elsewhere", and it is right here rather
than a `command`: the client applies it in the same undo step, so accepting the completion and
importing the module are one action to undo. The edit uses the **same** `import_insertion_point` the
quick fix uses — two insertion rules would put the line in different places in the same file, and only
one of them can be after the `//!` module docs.

`sort_text` is `~` + the label. `~` is `0x7E`, above every letter and digit, and an item with no
`sort_text` sorts by its label — so in-scope names keep their order and every unimported one lands
after all of them. Neither field was populated anywhere in the crate before.

**Snippet parameter completion already existed**, tested since ADR-0028 §5, so the new items simply
share it: `create_window(${1:width}, ${2:height}, …)$0` with the declaration's real parameter names,
and the whole signature in `detail`. That half of the request needed no work, which is worth recording
because it was reported as missing — it was missing *for unimported names*, because there was no item
at all to carry it.

### §6. Two defects in the imported half, which the new source must not clone

**An aliased import contributed bare names.** `Simp :: #import "Simp";` makes the module reachable only
as `Simp.name` (ADR-0179 §1), and the pattern discarded `alias`, so completion offered names that do
not resolve — accepted, and the file then failed to check.

**Module-private names were offered.** Names came from the other file's raw HIR items rather than
`file_exports`, so `#scope_module` was ignored and sema rejected what the editor had just suggested.
The code-action path had always filtered correctly, so the two disagreed about what a module offers.
`file_exports` was called exactly once in the whole crate before this.

### §7. Measured, because ADR-0033 §3 said to

| completion | before | after |
|---|---|---|
| cold | 0.58 ms | **4.09 ms** |
| warm | 0.062 ms | 0.034 ms |
| after an edit | 0.376 ms | 0.345 ms |

One 4 ms hit at the first completion of a session, and nothing measurable thereafter. The separate
`workspace_load` cost (22.5 ms, once) is now triggered by whichever of completion and code action comes
first, where before only the latter paid it.

`jr bench` measures completion **with** the workspace input, deliberately: passing `None` would report
a latency the real server never has, which is the wrong question rather than an optimistic answer.

### §8. `None` for the workspace means "discovery has not run"

Not "the workspace is empty". Every pre-existing completion test passes `None` and therefore still
measures the in-scope surface, rather than silently competing with several hundred importable names.
A test asserting that is included, because the distinction is the kind that decays quietly.

### §9. The server formats

`textDocument/formatting`, whole-document. `jr fmt` has existed since ADR-0027 and every editor had to
be told how to shell out to it; a server that formats needs no such configuration, and it was the one
capability whose absence forced per-editor setup.

Whole-document only: the formatter reprints from the CST, so it has no notion of a range, and any
finer edit list would have to be *derived* by diffing — a second formatter's worth of decisions about
what moved. A file that does not parse is declined with `None` rather than an error, because the buffer
usually does not parse while it is being edited and `Format on save` must not reprint a guess.

### §10. `tree-sitter-jairs/src/parser.c` is tracked, reversing ADR-0025 §3

That section rejected committing generated output because "all generated output is regenerated by the
gate". **The premise expired.** Zed builds a grammar by cloning the repository at a revision and
running `clang` over `src/parser.c` — verified by reading its `extension_builder.rs`, which never runs
`tree-sitter generate`. A repository without the generated parser cannot supply a grammar to Zed at
all.

Tracking it also **strengthens** gate 6. That gate regenerates and checks `git status` for drift; while
these files were ignored, drift could never be reported for them, so a `grammar.js` change with a stale
parser beside it was invisible. `grammar.json` and `node-types.json` stay ignored — nothing reads them,
and they would be churn.

### §11. The extension crate is outside the workspace

It compiles to `wasm32-wasip2` and depends on `zed_extension_api`, neither of which the compiler has
any use for. Joining the workspace would put a WASM-only target in `cargo build --workspace` and a
crate in the six gates that cannot run under them.

### §12. The Zed highlights query is generated, and both editors' queries are gated

Zed's theme vocabulary is flatter than Neovim's and its query engine has no `#lua-match?`. The
*structure* is shared and only the dialect is translated, by `editors/zed/generate-queries.sh`, because
a hand-made copy would go stale the first time a keyword became real — which, per that file's own
comments, has happened seven times.

Gate 6 now compiles **both** editors' queries and regenerates the Zed one. Writing the three
Zed-specific queries by hand is how the query half earned its keep immediately: the first
`brackets.scm` named `"\""`, which is not a node here because a `string_literal` is one token, and the
first `outline.scm` used `field name:` where the grammar spells that identifier positionally. Both
failed to compile and neither would have been visible any other way.

### §13. ADR-0036 §3 is reversed, on request

It said "Zed, Helix and the rest are the same answer" — any editor speaking LSP can use `jr lsp`, and
`editors/` is not a promise to package for every host. The decider asked for Zed, so it is packaged.

The reasoning that ADR gave for VS Code does not transfer: it declined because semantic tokens were
unimplemented and a TextMate grammar would be a third source of syntactic truth. Semantic tokens
shipped in ADR-0159, and Zed consumes the **tree-sitter** grammar that already exists, so there is no
third source.

## Consequences

- A name from any `#import`able module is offered, with its import, its snippet and its signature.
- The language server formats, so no editor needs a formatter command.
- `editors/zed/` is a second supported editor, verified by `editors/zed/verify.sh` — 19 checks,
  including a replication of Zed's own grammar build. The one manual step is `install dev extension`.
- The generated parser is tracked, and gate 6's drift check covers two artefacts where it covered none.
- `jr lsp` resolves the standard library without being told to, which also un-breaks the existing
  auto-import quick fix.
- No new diagnostic code. **E0296 is still the first free one.**

## Rejected alternatives

- **A dedicated `ModuleIndexInput` walked separately.** More precise — it would not depend on the
  search paths happening to be inside a discovery root — but a second staleness surface and a second
  thing the file watcher must trigger. `workspace_roots` already seeds the walk with the search paths.
- **Reserving placeholder arguments in `add_phi_operands`.** Wrong for a different feature and worth
  not repeating: see ADR-0198 §2.
- **An aliased import suppressing the unimported offer.** The alias makes the bare name unreachable,
  so `create_window` genuinely does need a bare `#import "Window";` to be written bare. Offering it is
  the honest answer, and the person can decline and write `W.create_window`.
- **A copy of the Neovim highlights query in `editors/zed/`.** Two answers to "which node is a
  keyword", and the copy is the one that goes stale.
- **A separate published grammar repository.** It is what real Zed extensions do, and it would let
  `parser.c` stay ignored here — at the cost of a second repository whose revision must be kept in step
  with a compiler that has to agree with it exactly.
- **`is_incomplete: true` with server-side filtering.** It would shrink each payload and make the
  client re-ask on every keystroke, trading one 4 ms index build for a request per character. The
  measurement says the index is not the expensive part.
