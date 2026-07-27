# Jairs in Neovim

Diagnostics, hover, goto-definition, completion, references, rename, symbols and tree-sitter highlighting, with **no plugin
manager and no plugins**. This directory is a runtimepath entry: Neovim discovers an LSP
config in `lsp/`, a parser in `parser/`, queries in `queries/`, filetype detection in
`ftdetect/` and buffer settings in `ftplugin/` on its own.

Requires **Neovim 0.11 or newer** (for `vim.lsp.config`/`vim.lsp.enable`). Verified on
0.12-dev.

## Setup

```sh
cargo build -p jr-cli          # or `cargo build --release -p jr-cli`
./editors/nvim/build.sh        # compiles parser/jairs.so
```

Then in your `init.lua`:

```lua
vim.opt.runtimepath:append("/absolute/path/to/jairs/editors/nvim")
vim.lsp.enable("jairs")
```

Open any `.jr` file. You should get highlighting immediately, and diagnostics, `K` for
hover, `gd` for goto-definition and `<C-x><C-o>` for completion once the server attaches.

## Check it works

```sh
nvim --headless -u NONE -l editors/nvim/verify.lua
```

53 checks, exiting non-zero on the first failure. It drives the real Neovim against the
real server: filetype, parser, every highlight capture it relies on (including
`@comment.documentation`, whose `#lua-match?` predicate the tree-sitter CLI cannot
validate), LSP attach, the negotiated position encoding, the resolved workspace root, four
hovers asserted by *text* — one of them an imported procedure's full card, prose and all —
a completion list with its snippet and its lazily-resolved documentation, field completion
after a `.`, goto-definition across an `#import`, and a diagnostic on a deliberately broken
file.

**This is verified, not gated.** It needs Neovim, which is not a build dependency of the
workspace, so making it one of the six CI gates would fail `cargo test` on a machine with
no editor installed. `PLAN.md` §1.5 records the consequence rather than implying the
integration is covered by CI.

## What you get, and what you do not

| Works | Notes |
|---|---|
| Syntax highlighting | tree-sitter, from the same `queries/*.scm` the drift gate checks — the files here are **symlinks**, so they cannot drift from the grammar's copies |
| Diagnostics | Published on open and on every change, with the stable `E0…` code attached |
| Hover | A card: the module or file, the declaration in Jairs syntax with parameter names, then its `///` documentation. Falls back to the type for an expression that is not a name. **A type annotation gets nothing** — `jr_hir::TypeRef` has no span (ADR-0028 §4) |
| Completion | Locals and parameters, file items, imported module items, keywords, builtin types; fields after `.`, directives after `#`. Procedures insert as call snippets with real parameter names; documentation arrives via `completionItem/resolve`. Scope is approximated as "declared earlier in this body" rather than by block |
| Doc comments | `///` documents the declaration below it, `//!` the file. `////` is an ordinary comment. Highlighted distinctly from an aside |
| Goto-definition | Locals, parameters, file-level items, and across an `#import` into `modules/` |
| Folds, indent queries | Shipped; `foldexpr`/`indentexpr` are yours to set |
| References, document highlight | `gr` / cursor-idle highlight. A reference search covers the whole workspace; a highlight is confined to the file on purpose, since a client sends it on every cursor move |
| Rename | `grn`. Workspace-wide, and it **refuses** rather than half-renaming: on a name collision, on a syntax error in a file it would edit, or on a workspace over 10 000 files (ADR-0030 §3) |
| Document and workspace symbols | `gO` for the outline (struct fields nested), and your picker's workspace-symbol command |
| Code actions, inlay hints, `signatureHelp` | **Not implemented.** Next wave |
| Formatting via LSP | **Not implemented.** Use `jr fmt`; `textDocument/formatting` is not advertised |

## How it finds things

- **The compiler**: a `jr` on `PATH` if there is one, otherwise `target/release/jr`, then
  `target/debug/jr` from this repository. That order is deliberate — someone who installed
  `jr` wants theirs, and someone hacking on the compiler wants the build they just made,
  without editing a file to switch.
- **Modules**: `--module-path <repo>/modules`, passed rather than discovered, because
  guessing a search path silently changes which module a program means. Point it somewhere
  else by copying `lsp/jairs.lua` into your own config and editing `cmd`.
- **The project root**: `.git` first, then `modules/`. `root_markers` order is *priority*
  rather than proximity (`:h vim.fs.root`), so `.git` wins wherever it appears. ADR-0026
  records why the other order was wrong: `modules` first rooted this repository's own
  corpus files at `tests/corpus`, because `tests/corpus/modules/` is a test fixture.

## Troubleshooting

- **No highlighting** — `build.sh` has not been run, or was run before a `grammar.js`
  change. `ftplugin/jairs.lua` starts tree-sitter under `pcall`, so a missing parser is
  silent by design; `:checkhealth vim.treesitter` will say so.
- **No diagnostics** — check `:LspInfo` / `:checkhealth vim.lsp`. If the command is not
  found, build `jr`.
- **`:lua =vim.lsp.get_clients()[1].offset_encoding` says `utf-16`** — your Neovim did not
  offer `utf-8`. Everything still works; columns just go through a conversion.
- **Nothing at all** — `filetype` must be `jairs`. If it is empty, the runtimepath append
  ran too late for `ftdetect`; put it near the top of `init.lua`.
