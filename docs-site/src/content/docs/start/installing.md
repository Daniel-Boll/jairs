---
title: Installing & running
description: Build the Jairs compiler from source and run your first program.
sidebar:
  order: 2
---

Jairs has no released binaries yet. You build the compiler — a Rust workspace — from source,
and it gives you a single driver binary called `jr`.

## Prerequisites

- **Rust**, stable toolchain (the workspace pins its version through `rust-toolchain.toml`,
  so `rustup` will select the right one automatically).
- A C compiler (`cc`) on your `PATH` — `jr build` uses it to link the final executable.
- **macOS arm64** is the primary and only verified target. An x86-64 Linux target is
  configured but, as of today, has never actually been run in CI.

## Build the compiler

```sh
git clone <the jairs repository>
cd jairs
cargo build --release -p jr-cli
```

That produces the driver at `target/release/jr`. Put it on your `PATH`, or invoke it through
Cargo while developing:

```sh
cargo run -q -p jr-cli -- run examples/hello.jr
```

The rest of this documentation writes commands as `jr <subcommand>`.

## The driver

`jr` is one binary with a handful of subcommands.

| Command | What it does |
| --- | --- |
| `jr run file.jr` | Check the program, then execute it in the **bytecode VM**. |
| `jr build file.jr -o out` | Check, compile through Cranelift, and link a **native executable** at `out`. |
| `jr check file.jr` | Type-check and report diagnostics; compile nothing. Accepts directories. |
| `jr fmt [--check] paths…` | Format source canonically. `--check` exits non-zero if anything is unformatted; `--stdin` reads stdin for editor integration. |
| `jr parse file.jr` | Debug aid: dump tokens or the syntax tree. |
| `jr bench file.jr` | Report language-server latency (cold / warm / after-edit). Reports, never judges. |
| `jr lsp` | Speak LSP 3.17 over stdin/stdout for an editor. |

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
compiler tests on every build. See [Two engines, one MIR](/language/introduction/#two-engines-one-language)
in Book I for why.

## Editor support

Jairs ships a language server (`jr lsp`, LSP 3.17) and a tree-sitter grammar. The repository
packages a ready-to-use **Neovim** integration under `editors/nvim/` — two lines in your
`init.lua` and one build script, with diagnostics, hover, goto-definition, completion,
references, rename, code actions, signature help and inlay hints on stock Neovim 0.11+
keybindings. Any other LSP-speaking editor works too: point it at the `jr lsp` command. A
VS Code extension is deliberately **not** provided.
