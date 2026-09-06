---
title: Tooling
description: The compiler driver, the language server, the formatter, and editor integration.
sidebar:
  order: 20
---

Jairs ships as one driver binary, `jr`, plus a language server and a tree-sitter grammar. This
chapter is a quick tour of the tooling around the language.

## The driver

`jr` is a single binary with nine subcommands (see [Installing &
running](/start/installing/) for the full table and exit codes):

- `jr new` — scaffold a new project directory: `jairs.toml`, `src/main.jr`, `build.jr`, `.gitignore`.
- `jr init` — scaffold a project in a directory that already exists.
- `jr run` — check, then execute in the bytecode VM.
- `jr build -o out` — check, compile, and link a native executable.
- `jr check` — type-check and report diagnostics; accepts directories.
- `jr fmt` — format source canonically.
- `jr parse` — dump tokens or the syntax tree (a debug aid).
- `jr bench` — report language-server latency, or compile throughput with `--throughput`.
- `jr lsp` — speak LSP over stdin/stdout for an editor.

Inside a project — anywhere under a `jairs.toml` — `jr run`, `jr build`, `jr check` and `jr fmt`
need no file argument at all: the manifest supplies the entry point.

## Backends and optimisation

`jr build --backend cranelift|llvm` chooses the code generator: Cranelift is the default and the
one verified continuously; LLVM is available when the compiler was built with its feature on, for
a more optimised release build. `--opt-level 0|1` chooses how hard either one optimises — `0`
disables the mid-end pass entirely, `1` (the default) runs the bounded pipeline every build ran
before the flag existed. There is still no `--release` shorthand: the two flags are named for
what they do rather than bundled into one. A three-way differential test asserts the VM,
Cranelift and LLVM agree, byte for byte, at both optimisation levels.

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

## Configuring a project

A `jairs.toml` at a project's root is an override, never a prerequisite — every command still
works with no manifest, and a missing file is neither an error nor a warning. Where one exists,
an explicit flag still outranks it: a flag is an instruction, the file is a default.

```toml
[fmt]
indent_style = "space"   # "space" or "tab"
indent_width = 4         # spaces per level; not read when indent_style = "tab"
max_width = 100          # breaks a long argument or parameter list; comments are never reflowed
```

`indent_style` and `indent_width` mirror EditorConfig's own vocabulary rather than inventing one.
`max_width`'s scope is narrow by measurement, not by taste: across the whole tree, 99% of the
lines that exceed it are comments, which this formatter never reflows, and a boolean chain or a
string literal that would cross it is left alone too — only a call's argument list or a
procedure's parameter list wraps, one item per line with a trailing comma.

An unrecognised key — a typo, or a setting from an older manifest — is an **error**, not a
silently ignored one: a configuration file whose typos are ignored is worse than a missing one,
because nothing then distinguishes a key that does nothing from one that has not been built yet.

## The language server

`jr lsp` speaks LSP 3.17 over stdio and provides fourteen capabilities:

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
- semantic tokens (classified by CST context first — sixteen token types and two modifiers)
- formatting (`textDocument/formatting`, whole-document — the server reprints from the same
  lossless CST `jr fmt` does, so no editor needs a separate formatter command)

Crucially, the language server is a **consumer of the same queries** as the batch compiler, not
a second front end — so a diagnostic you see in your editor is the diagnostic `jr check`
produces. Completion of an unimported name carries its own `#import` edit, applied in the same
undo step as accepting it.

## Editors

- **Neovim** is packaged directly, under `editors/nvim/`: two lines in your `init.lua` and one
  build script, no plugin manager. Every capability lands on a stock Neovim 0.11+ default
  keybinding (`K` for hover, `gd` for definition, `grn` for rename, and so on), so there are no
  keymaps to add. It works on a standalone `.jr` file, not only inside a checkout.
- **Zed** is packaged too, under `editors/zed/`: a tree-sitter-based extension with all fourteen
  LSP capabilities, format-on-save, and its own 19-check `verify.sh`. An earlier decision had
  declined a second editor; that decision is reversed.
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

The tooling is real and used, but the project is pre-alpha and honest about its edges:
**both platforms are verified green** — macOS arm64 locally, gate by gate, and x86-64 Linux in
CI, all seven jobs passing — and a native binary now carries real DWARF (line tables, struct
layouts, stack-resident locals) in both back ends, so it is debuggable in a normal debugger. One
item is still open: a register-resident local's location is not yet described. The latency
numbers `jr bench` reports are from one machine on a synthetic tree — a floor, not a promise. The
next chapter, [What's absent (and why)](/language/whats-absent/), lays out the larger picture of
what is and isn't done.
