# Jairs

Jairs is a Jai-inspired systems language with compile-time execution, explicit
allocators, and no GC, RAII, or exceptions — compiled by a hand-written,
error-recovering compiler written in Rust.

> **Status: pre-alpha.** Jairs source runs in the compile-time VM *and* compiles to
> a native binary, and the two agree byte for byte — including where a trap
> happened. The language it agrees about is deliberately tiny. The tables below say
> exactly how tiny, and are updated at the end of every wave; if they and the code
> disagree, the code is right and the tables are a bug.
>
> See [`PLAN.md`](PLAN.md) §1.5 for per-crate status, §2.1 for the wave order, and §7
> for what happens next.

---

## Status, honestly

Last updated after the **editor integration** wave. 696 workspace tests; six CI gates
green on macOS arm64, plus 22 Neovim checks that are verified rather than gated.

### What you can actually do

| You can | How | Caveat |
|---|---|---|
| Compile and run a program in the comptime VM | `jr run file.jr` | Register bytecode interpreter, no JIT tier |
| Compile to a native executable | `jr build file.jr -o out` | arm64 macOS verified; x86-64 Linux configured in CI but **never run** |
| Get rustc-grade diagnostics | `jr check file.jr` | 59 codes across lexer, parser, HIR, sema, MIR and const-eval |
| Format source canonically | `jr fmt [--check] paths…` | The corpus is canonical under it, CI-enforced |
| Inspect tokens or the CST | `jr parse file.jr` | Debug aid |
| Call libc from Jairs | `#foreign` / `#system_library` | Through libffi at comptime, a real call natively |
| Fold a compile-time call | `COMPUTED :: #run add(2, 3)` | One *trivial* `#run`: a call or a constant expression, same file only |
| Import a module | `#import "Basic";` | One module = one file, flat imports, cycles legal |
| Edit in Neovim, with highlighting, diagnostics, hover and goto-definition | `editors/nvim/` | Two lines in `init.lua` and one build script; no plugin manager. Neovim **0.11+**. See [`editors/nvim/README.md`](editors/nvim/README.md) |
| Get the same in another editor | `jr lsp` | Speaks LSP 3.17 over stdio; **no VS Code extension ships yet**, so you wire it up yourself |

### The language today

Everything in the left column is implemented end to end — parsed, formatted,
type-checked, lowered, executed in the VM, compiled natively, and asserted equal in
both engines. Everything in the right column is absent, with the wave that adds it.
The authoritative version of this list is
[`docs/spec/00-overview.md`](docs/spec/00-overview.md).

| Works | Absent (wave) |
|---|---|
| `s64`, `bool`, `string`, `*T` | rest of the numeric tower, `float32/64` (**W1**) |
| `u8` in type position only, for `*u8` and FFI | general `u8` arithmetic, `cast()`, `xx` (**W1**) |
| `struct { … }`, one level, nominal | `enum`, `enum_flags`, `union` (**W1**) |
| procedures, single return value | multiple returns, named/default args (**W2**) |
| `::` constant, `:=` inferred, `: T = v` typed, `---` uninit | |
| `if` / `else if` / `else`, `while`, `break`, `continue`, `return` | `for`, labelled break, `defer`, `using` (**W2**) |
| blocks and block scope, shadowing | `#scope_*` visibility (**W2**) |
| `+ - * / %` trapping, `+% -% *%` wrapping, unary `-` | bitwise `& \| ^ ~ << >>` (**W1**) |
| `== != < <= > >=`, `&& \|\| !` short-circuiting | operator overloading (**W1**) |
| `=` and compound `+= -= *= /= %= +%= -%= *%=` | |
| `a.b.c` field access, auto-deref through pointers | arrays `[N]T`, views `[]T`, dynamic `[..]T` (**W1**) |
| calls, nested; a discarded call is a statement | |
| integer literals (dec/hex/bin/oct, `_`), string literals + escapes | usable float literals — they lex, the parser rejects them (**W1**) |
| nesting block comments | |
| one trivial `#run` | arbitrary `#run`, RTTI, `#insert`, `#code` (**W4**) |
| `#import`, `#foreign`, `#system_library` | polymorphs `$T`, `#expand` macros (**W5**) |
| overflow traps with a source location (ADR-0002, ADR-0020) | `context`, allocators, temp storage, backtraces (**W3**) |

There is **no error-handling model yet** — ADR-0008 reserves the slot, nothing fills
it. There is no GC and no RAII, which is a design value rather than a missing feature.

### Compiler internals

| Stage | Status | Honest note |
|---|---|---|
| Lexer, parser, CST, typed AST | **Works** | Hand-written, error-recovering, trivia-preserving |
| Formatter | **Works** | Pure function over the CST |
| HIR, name resolution, module loader | **Works** | Flat import merge (ADR-0014) |
| InternPool (types, comptime values, layout, arithmetic) | **Works** | One layout computation and one integer evaluator, shared (ADR-0018 §2, ADR-0022 §2) |
| Sema (signatures, checking, inference) | **Works** | E0212–E0226; no const-eval here — ADR-0018 §3 puts it in the VM |
| MIR (typed SSA, Braun construction) | **Works** | Block parameters, not phis (ADR-0017); CFG diagnostics E0227–E0229 |
| Mid-end | **Four passes** | Inliner, store-to-load forwarding, const-prop, DCE, to a bounded fixed point (ADR-0021 – ADR-0023). Forwarding is block-local, so a value read across a loop stays in memory; no SROA; the SSA value arena is never compacted |
| Bytecode VM + libffi | **Works** | Per-instruction spans, so a trap names its line. No JIT |
| Cranelift back end + linker | **Works** | Refuses an aggregate return and a call through a procedure pointer — so does the VM |
| salsa incremental database | **Works** | Built *and* optimized MIR staged (ADR-0021 §1); invalidation is at file grain |
| Differential harness | **Works** | Compares stdout, stderr and exit status of both engines as subprocesses |
| LLVM back end | **Not started** | Wave W8 |
| Language server | **Works** | `jr lsp`: diagnostics, hover, goto-definition (incl. across an `#import`), on a worker thread with salsa cancellation (ADR-0024). No completion, rename or inlay hints — W9 owns those |
| Neovim integration | **Works** | `editors/nvim/` (ADR-0025), verified against the real editor by a 22-check script — **not** by CI, which has no Neovim |
| VS Code integration | **Not started** | The server is ready; nothing launches it |
| Compilation driver / workspaces | **Not started** | `jr-driver` is a one-line stub |
| Debug info | **Not started** | No DWARF at all; a native binary is not debuggable |
| Optimisation levels | **Not started** | No `--release`, no `opt_level`; one code path |

### Things it is easy to over-read

- **No published performance number.** ADR-0019 §6 says a number taken without a
  mid-end measures the missing mid-end. The mid-end now exists, so a number is finally
  honest to take — and it has not been taken. Nothing in this repo has been benchmarked
  against anything.
- **The two engines agreeing is *tested*, not assumed.** They share MIR, which makes
  agreement likely; `crates/jr-cli/tests/differential.rs` is what makes it checked.
  Both of this project's silent miscompiles were places where a plausible argument
  stood in for a check.
- **Only two of fifteen executable corpus programs print anything**, so the corpus
  differential largely compares silence with silence. That is why it also drives
  computations out through `exit` — arithmetic, precedence, loops, block parameters,
  pointers, struct offsets and both traps.
- **A cross-file `#run` does not work**, and ADR-0021 §2 now depends on that. Enabling
  it requires more than removing the refusal.
- **`u8` is not a supported integer type.** It exists so `*u8` and byte-sized FFI
  arguments can be spelled.
- **Optimisation is real but shallow.** Four passes run, and `024-hello.jr` now folds
  its struct away entirely, collapses an `if` and deletes the dead arm. But forwarding is
  one walk per basic block, so anything read across a loop boundary stays in memory, and
  a whole-struct store never feeds a field read — which is why `modules/Basic`'s `print`
  still keeps its slot.
- **ADR-0002's arithmetic has two implementations, not one.** `jr-pool` owns the one
  both *evaluators* share; `jr-codegen-clif` keeps its own because it emits code rather
  than evaluating. The pair is held equal by `differential.rs` and nothing else.
- **Neovim integration is verified on one machine, not gated.** The 22 checks need an
  editor, and Neovim is not a build dependency of this workspace, so `cargo test` cannot
  run them. VS Code has nothing at all.
- **The tree-sitter parser must be rebuilt after a grammar change**, and highlighting
  fails *silently* if you forget — `ftplugin` starts tree-sitter under `pcall`, because a
  missing parser is an ordinary state rather than an error.
- **Hover shows a type and nothing else.** No documentation, no signature rendering.
- **Nothing here is self-hosted.** The compiler is Rust; only `modules/Basic` is Jairs.

---

## What it looks like

```jr
#import "Basic";                       // module system: one module, one file

Point :: struct { x: s64; y: s64; }   // structs, one level

add :: (a: s64, b: s64) -> s64 {      // procs, single return
    return a + b;
}

MESSAGE :: "hello from Jairs\n";      // constants
COMPUTED :: #run add(2, 3);           // one trivial comptime call

main :: () {
    p: Point;                         // decls: typed, and inferred below
    p.x = 4;
    sum := add(p.x, COMPUTED);        // := inference
    if sum > 5  print(MESSAGE);       // if
    i := 0;
    while i < 3 { i = i + 1; }        // while
    ptr := *sum;                      // pointer take + deref
    if ptr.* == 9  print_line("ok");  // `print_int` needs `cast`, which is W1
}
```

---

## Strategy

The compiler is built as a **vertical tracer-bullet slice** ("Jairs-0") that
drives one tiny language subset all the way through every component — lexer,
parser, CST, HIR, Sema, MIR, VM, Cranelift, linker, FFI, stdlib module, LSP,
tree-sitter, formatter — until `hello.jr` is a signed native arm64 binary and
the LSP gives hover on it. Everything works, badly. Then the language is
thickened one feature wave at a time.

See [`PLAN.md`](PLAN.md) for the full roadmap, wave order, and architecture
decisions.

---

## Architecture

```
Source .jr
  → Lexer (hand-written, trivia-preserving)
  → Parser (hand-written, error-recovering, recursive descent)
  → Lossless CST (rowan)          ← jr fmt consumes this directly
  → Typed AST accessors
  → HIR (desugar, module graph, #import resolution, scopes)
  → Sema (lazy on-demand: types, inference, const-eval, polymorphs)
      ↔ InternPool (canonical IDs for every type and comptime value)
      ↔ Bytecode VM (#run / #insert / comptime FFI)
  → MIR (typed SSA, monomorphized)
  → Mid-end (inliner, mem2reg, DCE, const-prop)
  → Cranelift backend  →  object file  →  cc driver + codesign  →  native binary
  → LLVM backend (W8, behind --release)
  → salsa DB  →  LSP server (diagnostics, hover, goto-def)

tree-sitter-jairs  — separate editor grammar, CI-gated against drift
```

The LSP is a **consumer of the same salsa queries** as the batch compiler, not
a second frontend. The VM and Cranelift both consume the same MIR so `#run` and
runtime cannot silently disagree — and the mid-end is required to keep that literally
true, not merely approximately: the inliner refuses to rewrite any body compile-time
evaluation can reach (ADR-0021 §2), so every body both engines might execute is
bit-identical in each.

---

## Crate layout

| Crate | Responsibility |
|---|---|
| `jr-base` | Foundational types: source spans, `FileId`, string interning, arenas, newtype IDs |
| `jr-diag` | Diagnostic model (severity, spans, notes, instantiation backtraces) and rustc-identical renderer |
| `jr-syntax` | Lexer, `SyntaxKind`, error-recovering recursive-descent parser, lossless `rowan` CST, typed AST accessors |
| `jr-fmt` | Canonical formatter — a pure function over the lossless CST |
| `jr-hir` | Desugared high-level IR: module graph, `#import` resolution, scopes, name binding |
| `jr-pool` | `InternPool`: canonical identities for every type and every compile-time value, plus the layout both back ends share (ADR-0018 §2) |
| `jr-sema` | Lazy on-demand semantic analysis: type checking, inference, const-evaluation, polymorph instantiation |
| `jr-mir` | Typed SSA mid-level IR and optimisation passes, including the inliner Cranelift does not provide |
| `jr-vm` | Bytecode compile-time execution engine: lowering from MIR, interpreter, comptime FFI bridge |
| `jr-codegen` | `Backend` trait and lowering helpers shared by every native backend |
| `jr-codegen-clif` | Cranelift backend — all Cranelift API contact is confined here (ADR-0009) |
| `jr-codegen-llvm` | LLVM backend for optimised release builds — feature-gated, lands in wave W8 |
| `jr-link` | Object-file emission and system linker driver, including macOS ad-hoc codesigning |
| `jr-db` | salsa query database — single source of truth shared by the batch driver and the LSP |
| `jr-driver` | Compilation orchestration: workspaces, compiler message queue, build metaprograms |
| `jr-lsp` | Language server — a consumer of `jr-db` queries, never a second frontend |
| `jr-cli` | The `jr` binary (`jr build`, `jr run`, `jr fmt`, `jr check`) |

---

## Building and testing

```sh
# Requires Rust stable (pinned via rust-toolchain.toml).
cargo test --workspace

# Check formatting and lints before pushing:
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

macOS arm64 is the primary development target. Linux x86-64 is kept green in CI
as a sanity oracle.

---

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT licence ([LICENSE-MIT](LICENSE-MIT) or
  <https://opensource.org/licenses/MIT>)

at your option.
