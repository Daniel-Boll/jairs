# Jairs

Jairs is a Jai-inspired systems language with compile-time execution, explicit
allocators, and no GC, RAII, or exceptions — compiled by a hand-written,
error-recovering compiler written in Rust.

> **Status: pre-alpha. Jairs source runs in the compile-time VM *and* compiles to
> a native binary, and the two agree.**
>
> What works today is the **front end, the mid-level IR, and the bytecode VM**.
> `jr check` parses a file, lowers it to HIR, loads the modules it imports, resolves
> names across the import boundary, type-checks it against those modules'
> signatures, lowers each procedure body to typed SSA, and reports rustc-quality
> diagnostics — including the three that need a control-flow graph: definite
> assignment, missing `return`, and a jump outside a loop. `jr run` then *executes*
> it: `jr run tests/corpus/valid/024-hello.jr` prints its output through libc
> `write`, having folded `#run add(2, 3)` at compile time. `jr build` compiles the
> same file through Cranelift, links it with `cc`, and the binary prints the same
> bytes and exits with the same status — including when it traps, down to the
> `  --> path:line:col` naming where. `jr fmt` formats
> it; `jr parse` dumps its tokens or tree. Implemented crates: `jr-base` (spans,
> interning, source map), `jr-diag` (diagnostics + renderer), `jr-syntax` (lexer,
> error-recovering parser, lossless CST, typed AST), `jr-fmt`, `jr-hir` (lowering,
> scopes, `#import` resolution), `jr-pool` (the InternPool and layout), `jr-sema`
> (signatures, types, inference), `jr-mir` (typed SSA, ADR-0017), `jr-vm` (register
> bytecode, interpreter, libffi bridge, ADR-0018), `jr-codegen` (the `Backend`
> trait), `jr-codegen-clif` (MIR → Cranelift, ADR-0019), `jr-link` (object emission
> and the `cc` driver), `jr-db` (salsa queries, the module loader, const
> evaluation), `jr-cli`.
>
> Not started: the LLVM backend (wave W8), the language server, and most of the
> standard library. `jr-mir` has no mid-end — no inliner, no DCE, no const-prop —
> so native code is correct but unoptimised, and ADR-0019 §6 records the deliberate
> deferral and what ends it. A trap still reports **no source location**, in either
> engine: `jr_mir::resolve_span` resolves one, but neither trap path calls it.
> A native build refuses an aggregate return and a call through a procedure
> pointer, both of which the VM also refuses.
> See [`PLAN.md`](PLAN.md) §1.5 for per-crate status and §7 for what happens next.

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
runtime cannot silently disagree.

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
