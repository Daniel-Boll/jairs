---
title: Tooling
description: The compiler driver, the language server, the formatter, and editor integration.
sidebar:
  order: 19
---

Jairs ships as one driver binary, `jr`, plus a language server and a tree-sitter grammar. This
chapter is a quick tour of the tooling around the language.

## The driver

`jr` is a single binary with a handful of subcommands (see [Installing &
running](/start/installing/) for the full table and exit codes):

- `jr run` — check, then execute in the bytecode VM.
- `jr build -o out` — check, compile through Cranelift, and link a native executable.
- `jr check` — type-check and report diagnostics; accepts directories.
- `jr fmt` — format source canonically.
- `jr parse` — dump tokens or the syntax tree (a debug aid).
- `jr bench` — report language-server latency.
- `jr lsp` — speak LSP over stdin/stdout for an editor.

## Diagnostics

`jr check` produces rustc-grade diagnostics: a message, a source span, and notes, rendered
with the same clarity you would expect from a modern compiler. There are over a hundred
diagnostic codes across the lexer, parser, name resolution, semantic analysis, the mid-level
IR, and const-evaluation. A couple of them (`E0218`, `E0212`) suggest a near name when you
misspell one — and stay silent rather than guessing badly for very short names. A couple are
*warnings* rather than errors: an unused `#import` (`E0231`) and a body the compiler could not
lower.

## The formatter

`jr fmt` is a pure function over the lossless syntax tree: it re-emits your source in a
canonical form. `jr fmt --check` exits non-zero if anything is unformatted (for CI), and
`jr fmt --stdin` reads stdin and writes stdout (for editor format-on-save). Because the
formatter works from a *lossless* tree, comments and doc comments survive formatting intact.

## The language server

`jr lsp` speaks LSP 3.17 over stdio and provides twelve capabilities:

- diagnostics
- hover (including a type's docs, and which file an `#import` resolved to)
- goto-definition
- completion (with resolve)
- references
- document highlight
- rename (workspace-wide; it **refuses** rather than half-completing on a collision, a syntax
  error, or a huge workspace)
- document symbols
- workspace symbols
- code actions
- signature help
- inlay hints

Crucially, the language server is a **consumer of the same queries** as the batch compiler, not
a second front end — so a diagnostic you see in your editor is the diagnostic `jr check`
produces. There are no semantic tokens (highlighting comes from tree-sitter instead).

## Editors

- **Neovim** is packaged directly, under `editors/nvim/`: two lines in your `init.lua` and one
  build script, no plugin manager. Every capability lands on a stock Neovim 0.11+ default
  keybinding (`K` for hover, `gd` for definition, `grn` for rename, and so on), so there are no
  keymaps to add. It works on a standalone `.jr` file, not only inside a checkout.
- **Any other LSP editor** works too — point it at the `jr lsp` command yourself.
- **VS Code** is deliberately not supported: a packaging target for an editor the maintainer
  doesn't use would rot. The server is editor-agnostic, so nothing stops a VS Code client from
  launching `jr lsp`.

## The tree-sitter grammar

Editor syntax highlighting comes from a separate tree-sitter grammar (`tree-sitter-jairs`),
kept in step with the compiler by a shared corpus of example programs — the same programs that
serve as the compiler's parser tests and as [Book II](/by-example/)'s examples. A grammar
change without a corpus file is rejected, which is how "the highlighter drifted from the
compiler" is prevented.

## A note on maturity

The tooling is real and used, but the project is pre-alpha and honest about its edges: it has
been verified on macOS arm64, the six local quality gates are green *locally* (CI has not run
on the repository), and the latency numbers `jr bench` reports are from one machine on a
synthetic tree — a floor, not a promise. The next chapter, [What's absent (and
why)](/language/whats-absent/), lays out the larger picture of what is and isn't done.
