# The Jairs extension for Zed

Syntax, diagnostics, completion with auto-import, and formatting — all from the same `jr` binary
that compiles the language.

## Install

```sh
cargo build --release -p jr-cli          # or have `jr` on PATH
./editors/zed/sync-grammar-rev.sh        # pin the grammar to the current commit
```

Then in Zed: **`zed: install dev extension`** and pick `editors/zed`.

Zed needs the `wasm32-wasip2` Rust target, which it installs itself when Rust came from `rustup`.
For the grammar it also needs the [wasi-sdk](https://github.com/WebAssembly/wasi-sdk), which it
downloads — point it at an existing one with `WASI_SDK_PATH` if you have it.

## What works

| Feature | How |
|---|---|
| Highlighting, brackets, indentation, outline | the tree-sitter grammar in `tree-sitter-jairs/` |
| Diagnostics, hover, goto-definition, references, rename, inlay hints, signature help, semantic tokens, code actions | `jr lsp` |
| **Completion of names you have not imported**, each inserting its own `#import` | `jr lsp` (ADR-0199) |
| **Format on save** | `jr lsp`'s `textDocument/formatting` |

Formatting arrives over the protocol rather than as an external command, so no `formatter` setting
is needed. `jr fmt --stdin` still exists if you want to configure one anyway.

## Module search paths

`jr lsp` looks for `#import`ed modules where it is told, plus its own bundled `modules/`. The
extension passes `<worktree>/modules`, so a project laid out that way needs no configuration.

For anything else, name the paths yourself:

```json
{
  "lsp": {
    "Jairs": {
      "binary": {
        "path": "/absolute/path/to/jr",
        "arguments": ["lsp", "--module-path", "/absolute/path/to/modules"]
      }
    }
  }
}
```

Supplying `arguments` replaces the defaults entirely, so include `lsp` and every `--module-path` you
need. Paths must be absolute: a server's working directory is whatever the editor happened to have,
and a relative search path fails silently rather than loudly (ADR-0025 §5).

## Layout

```
extension.toml                  manifest: the language server and the grammar
Cargo.toml                      the WASM crate; outside the workspace on purpose
src/jairs.rs                    supplies the `jr lsp` command — the only Rust Zed requires
languages/jairs/config.toml     name, suffixes, comments, brackets
languages/jairs/highlights.scm  GENERATED from tree-sitter-jairs/queries by generate-queries.sh
languages/jairs/brackets.scm    Zed-only
languages/jairs/indents.scm     Zed-only dialect (@outdent, not @dedent)
languages/jairs/outline.scm     Zed-only
generate-queries.sh             translates the Neovim highlights query into Zed's dialect
sync-grammar-rev.sh             pins the grammar revision Zed clones
```

`highlights.scm` is generated so that the two editors cannot disagree about which node is a keyword —
that query has had to learn a construct seven times, and a hand-made copy would have gone stale on
the first one. Gate 6 regenerates it and fails on drift.

## Why the grammar revision needs pinning

Zed clones the grammar repository at a revision and compiles `src/parser.c` with `clang`. It never
runs `tree-sitter generate`, which is why that file is tracked here (reversing ADR-0025 §3 — see
ADR-0199 §10) and why a `grammar.js` change needs `sync-grammar-rev.sh` and a re-install.
