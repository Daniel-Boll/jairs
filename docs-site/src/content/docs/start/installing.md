---
title: Installing & running
description: Build the Jairs compiler from source and run your first program.
sidebar:
  order: 2
---

Jairs has no released binaries yet. You build the compiler — a Rust workspace — from source,
and it gives you a single driver binary called `jr`. The standard library is **compiled into
that binary**, so `jr` works from any directory and `#import "Basic";` needs no search path;
`-I` exists for the modules you write and for reading this repository's own `modules/` tree
rather than the compiled-in copy.

## Prerequisites

- **Rust**, stable toolchain (the workspace pins its version through `rust-toolchain.toml`,
  so `rustup` will select the right one automatically).
- A C compiler (`cc`) on your `PATH` — `jr build` uses it to link the final executable.
- **macOS arm64 and x86-64 Linux are both verified**: macOS locally, gate by gate, and Linux
  in CI with all seven jobs passing. Windows is unverified — the declarations are written and
  the link has never been tried.
- **SDL2**, only if you want the graphics modules. See [Book IV](/games/) for how a drawing
  program is built, and why the library's path is a flag rather than something a source file
  can state.

## Build the compiler

```sh
git clone <the jairs repository>
cd jairs
cargo build --release -p jr-cli
```

That produces the driver at `target/release/jr`. Put it on your `PATH`, install it with
`cargo install --path crates/jr-cli`, or invoke it through Cargo while developing:

```sh
cargo run -q -p jr-cli -- run examples/01-hello.jr -I modules
```

The rest of this documentation writes commands as `jr <subcommand>`.

## A project, or a loose file

Either works. `jr new my-game` creates a directory, and `jr init` scaffolds one in place;
both write a `jairs.toml`, a `src/main.jr`, a `build.jr` and a `.gitignore`. Inside a project
every command takes its defaults from the manifest, so `jr build` with no path compiles the
entry point the manifest declares, and `[build] module_paths` saves repeating `-I`.

A manifest is never required: with none, every command works exactly as it does without one,
and a missing `jairs.toml` is neither an error nor a warning. An **unrecognised key inside
one** is an error, because a silently ignored setting is worse than a refused file.

## The driver

`jr` is one binary with nine subcommands.

| Command | What it does |
| --- | --- |
| `jr new name` | Create a project in a new directory. |
| `jr init` | Scaffold a project in an existing directory. |
| `jr run file.jr` | Check the program, then execute it in the **bytecode VM**. |
| `jr build file.jr -o out` | Check, compile, and link a **native executable** at `out`. |
| `jr check file.jr` | Type-check and report diagnostics; compile nothing. Accepts directories. |
| `jr fmt [--check] paths…` | Format source canonically. `--check` exits non-zero if anything is unformatted; `--stdin` reads stdin for editor integration. |
| `jr parse file.jr` | Debug aid: dump tokens or the syntax tree. |
| `jr bench file.jr` | Report language-server latency (cold / warm / after-edit), or compile throughput with `--throughput`. Reports, never judges. |
| `jr lsp` | Speak LSP 3.17 over stdin/stdout for an editor. |

### The flags worth knowing on `jr build`

| Flag | What it does |
| --- | --- |
| `-I, --module-path DIR` | Where to look for an `#import`, searched before the compiled-in modules. Repeatable. |
| `-L, --library-path DIR` | Where to look for a `#system_library`, before the C driver's defaults. Repeatable, and how SDL2 is found. |
| `-O, --opt-level 0\|1` | How much the mid-end may rewrite. `1` is the default and runs inline, store forwarding, const-prop and DCE; `0` runs none, so a wrong answer is attributable to lowering rather than to a pass. A level may never change what a program computes; the one thing `0` changes is a backtrace, because nothing is inlined. |
| `--backend cranelift\|llvm` | Which code generator. Cranelift is the default and the verified one; LLVM needs a compiler built with `--features llvm` and is refused with a message naming the feature otherwise. |
| `--output-kind` | `executable` (default), `dynamic-library`, `static-library` or `object`. A C program can link the libraries this produces. |
| `--no-bounds-check` | Strips every bounds check. Undefined behaviour by construction, and deliberately does not change a `#no_abc` procedure or compile-time execution, where a trap is a diagnostic. |
| `--script` | Treat the file as a **build script**: compile it, run it in the VM, then perform the compilations it asked for. |
| `--linker-arg ARG` | An extra argument for the C driver, after everything the compiler generates. |

A native binary carries real **DWARF** — line tables, struct layouts and stack-resident
locals — in both back ends, so `lldb` can break on a line and print a local. One case is
still <span class="jairs-status absent">absent</span>: a register-resident local, which needs
a location list rather than a single register expression.

### Exit codes

The driver's exit codes are stable and worth knowing, because the example programs in these
books lean on them:

| Code | Meaning |
| --- | --- |
| `0` | Success. |
| `1` | The program had diagnostics (a check failure). |
| `2` | A usage error, or code generation / linking failed. |
| `3` | An I/O error. |
| `4` | The program **trapped** at run time (overflow, a bad index, a null deref…). |
| *n* | If the program itself called `exit(n)`, that status is propagated. |

Several example programs deliberately end in `exit(n)` where `n` encodes which assertions
passed — a trick the compiler's own test corpus uses so a computation is observable through
the process's exit status rather than only through printed text.

## Your first program

Save this as `hello.jr`:

```jr
#import "Basic";

main :: () {
    print("hello from Jairs\n");
}
```

Run it in the VM:

```sh
jr run hello.jr
# hello from Jairs
```

Then compile it to a native binary and run that:

```sh
jr build hello.jr -o hello
./hello
# hello from Jairs
```

Both should print exactly the same thing. That is not a coincidence — it is a property the
compiler tests on every build. See [Two engines, one language](/language/introduction/#two-engines-one-language)
in Book I for why.

## Editor support

Jairs ships a language server (`jr lsp`, using the LSP 3.17 protocol) with these feature families —
diagnostics, hover, goto-definition, completion (including a name you have not imported yet,
which comes with the `#import` line as an edit beside it), references, document highlight,
rename, code actions, signature help, inlay hints, document and workspace symbols, formatting
and semantic tokens — and a tree-sitter grammar.

Two editors are packaged:

- **Neovim**, under `editors/nvim/` — two lines in your `init.lua` and one build script, on
  stock Neovim 0.11+ client behavior and default mappings. The headless verifier exercises the
  real editor against the real server.
- **Zed**, under `editors/zed/` — a dev extension carrying the grammar and launching `jr lsp`.
  `./editors/zed/verify.sh` mechanically checks the wiring, queries, grammar-build shape and
  selected advertised capabilities; installing and exercising the extension remains manual.

Another LSP-speaking editor can be configured to launch `jr lsp`, but this repository does not
carry or verify that integration. A VS Code extension is deliberately **not** provided.

## Where to go next

[Book I](/language/introduction/) is the narrative tour. [Book IV](/games/) is the shortest
path to something on screen, and it starts by explaining why a drawing program is a
`jr build` program and never a `jr run` one.
