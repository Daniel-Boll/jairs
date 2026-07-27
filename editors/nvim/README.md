# Jairs in Neovim

Diagnostics, hover, goto-definition and tree-sitter highlighting, with **no plugin
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
hover and `gd` for goto-definition once the server attaches.

## Check it works

```sh
nvim --headless -u NONE -l editors/nvim/verify.lua
```

22 checks, exiting non-zero on the first failure. It drives the real Neovim against the
real server: filetype, parser, every highlight capture it relies on, LSP attach, the
negotiated position encoding, two hovers asserted by *text*, goto-definition across an
`#import`, and a diagnostic on a deliberately broken file.

**This is verified, not gated.** It needs Neovim, which is not a build dependency of the
workspace, so making it one of the six CI gates would fail `cargo test` on a machine with
no editor installed. `PLAN.md` §1.5 records the consequence rather than implying the
integration is covered by CI.

## What you get, and what you do not

| Works | Notes |
|---|---|
| Syntax highlighting | tree-sitter, from the same `queries/*.scm` the drift gate checks — the files here are **symlinks**, so they cannot drift from the grammar's copies |
| Diagnostics | Published on open and on every change, with the stable `E0…` code attached |
| Hover | The type of the expression under the cursor. A *declaration* has no hover, which is correct rather than missing |
| Goto-definition | Locals, parameters, file-level items, and across an `#import` into `modules/` |
| Folds, indent queries | Shipped; `foldexpr`/`indentexpr` are yours to set |
| Completion, rename, references, inlay hints | **Not implemented.** Wave W9 (`PLAN.md` §2.1) |
| Formatting via LSP | **Not implemented.** Use `jr fmt`; `textDocument/formatting` is not advertised |

## How it finds things

- **The compiler**: a `jr` on `PATH` if there is one, otherwise `target/release/jr`, then
  `target/debug/jr` from this repository. That order is deliberate — someone who installed
  `jr` wants theirs, and someone hacking on the compiler wants the build they just made,
  without editing a file to switch.
- **Modules**: `--module-path <repo>/modules`, passed rather than discovered, because
  guessing a search path silently changes which module a program means. Point it somewhere
  else by copying `lsp/jairs.lua` into your own config and editing `cmd`.
- **The project root**: the nearest ancestor containing `modules/`, then `.git`. A Jairs
  project is defined by having modules to import, so vendoring one inside a git repository
  should not attach it to the outer project.

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
