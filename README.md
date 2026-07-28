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

Last updated after the **`#import` navigation** wave (ADR-0035). 837 workspace
tests; six CI gates green on macOS arm64, plus 67 Neovim checks that are verified rather than
gated.

### What you can actually do

| You can | How | Caveat |
|---|---|---|
| Compile and run a program in the comptime VM | `jr run file.jr` | Register bytecode interpreter, no JIT tier |
| Compile to a native executable | `jr build file.jr -o out` | arm64 macOS verified; x86-64 Linux configured in CI but **never run** |
| Get rustc-grade diagnostics | `jr check file.jr` | 61 codes across lexer, parser, HIR, sema, MIR and const-eval. E0218 and E0212 suggest a near name; E0231 is the one *warning* — an unused `#import` |
| Format source canonically | `jr fmt [--check] paths…` | The corpus is canonical under it, CI-enforced |
| Inspect tokens or the CST | `jr parse file.jr` | Debug aid |
| Measure language-server latency | `jr bench file.jr` | Reports min/median/p95 cold, warm and after an edit. **Reports, never judges** — no threshold, not a gate (ADR-0033) |
| Call libc from Jairs | `#foreign` / `#system_library` | Through libffi at comptime, a real call natively |
| Fold a compile-time call | `COMPUTED :: #run add(2, 3)` | One *trivial* `#run`: a call or a constant expression, same file only |
| Import a module | `#import "Basic";` | One module = one file, flat imports, cycles legal |
| Edit in Neovim, with highlighting, diagnostics, hover, goto-definition, completion, rename, code actions, signature help and inlay hints | `editors/nvim/` | Two lines in `init.lua` and one build script; no plugin manager. Neovim **0.11+** — every capability is on a stock 0.11 default binding (`K`, `gd`, `gra`, `grn`, `grr`, `gO`, `<C-s>`), so there are no keymaps to add. Works on a standalone `.jr` file too, not only inside a checkout. See [`editors/nvim/README.md`](editors/nvim/README.md) |
| Use any other LSP editor | `jr lsp` | Speaks LSP 3.17 over stdio. The repository packages for Neovim only and **will not ship a VS Code extension** (ADR-0036) — point your client at the command yourself |

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
| nesting block comments; `///` and `//!` doc comments, shown on hover | doc generation (`jr doc`) — nothing consumes docs but the language server |
| one trivial `#run` | arbitrary `#run`, RTTI, `#insert`, `#code` (**W4**) |
| `#import`, `#foreign`, `#system_library` | polymorphs `$T`, `#expand` macros (**W5**) |
| overflow traps with a source location (ADR-0002, ADR-0020) | `context`, allocators, temp storage, backtraces (**W3**) |

There is **no error-handling model yet** — ADR-0008 reserves the slot, nothing fills
it. There is no GC and no RAII, which is a design value rather than a missing feature.

### Compiler internals

| Stage | Status | Honest note |
|---|---|---|
| Lexer, parser, CST, typed AST | **Works** | Hand-written, error-recovering, trivia-preserving. Doc comments are trivia, so they cannot change what parses (ADR-0027) |
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
| Language server | **Works** | `jr lsp`, twelve capabilities: diagnostics, hover, goto-definition, completion + resolve, references, documentHighlight, rename (workspace-wide, refuses rather than half-renaming), documentSymbol, workspaceSymbol, code actions, `signatureHelp`, inlay hints (ADR-0024, ADR-0028, ADR-0030, ADR-0031). Dispatches a read only after every write, because the reverse silently lost `didOpen`'s diagnostics (ADR-0032). No semantic tokens |
| Neovim integration | **Works** | `editors/nvim/` (ADR-0025), verified against the real editor by a 67-check script — **not** by CI, which has no Neovim |
| VS Code integration | **Will not be built** | ADR-0036: the maintainer does not use it, and a packaging target for an unused editor rots. `jr lsp` is editor-agnostic, so any LSP client works |
| Compilation driver / workspaces | **Partly** | `jr-driver` is still a one-line stub; the workspace *file list* exists in `jr-db::workspace` (ADR-0029): the search paths plus the root tree, walked and watched, bounded at 10 000 files |
| Debug info | **Not started** | No DWARF at all; a native binary is not debuggable |
| Optimisation levels | **Not started** | No `--release`, no `opt_level`; one code path |

### Things it is easy to over-read

- **There is still no published *compile-throughput* number.** ADR-0019 §6 says a number
  taken without a mid-end measures the missing mid-end; the mid-end now exists, so one is
  finally honest to take, and it has not been taken. What *has* been measured is
  language-server latency (`jr bench`, ADR-0033) — a different question, and no substitute.
- **The latency numbers, so they are not overstated.** On a synthetic 36 000-line, 302-file
  workspace: every operation is under **1 ms** cold except `references` and `rename`, which
  cost **55 ms** because they parse the workspace, and `workspace_load` at **41 ms**. A
  40-line corpus file puts everything under 0.6 ms. These are one machine, one synthetic
  tree, and a floor rather than a promise. `jr bench` also reports two rows that are not
  client requests — `parse_all_files` and `resolve_all_files` — because they are what turned
  "references is slow" into "parsing is slow" (ADR-0034).
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
- **Neovim integration is verified on one machine, not gated.** The 67 checks need an
  editor, and Neovim is not a build dependency of this workspace, so `cargo test` cannot
  run them. No other editor is packaged for, deliberately (ADR-0036).
- **The tree-sitter parser must be rebuilt after a grammar change**, and highlighting
  fails *silently* if you forget — `ftplugin` starts tree-sitter under `pcall`, because a
  missing parser is an ordinary state rather than an error.
- **Hover on an `#import` shows which file it resolved to**, because `#import "Basic"` does
  not say *which* `Basic` — the module search-path order decides, so the answer depends on how
  the server was configured. It also shows the module's `//!` documentation. Both were
  unreachable before ADR-0035, behind an `ItemKind::Import` arm whose comment claimed
  otherwise.
- **Hover does not work on a type annotation.** The `Point` in `p: Point` gets nothing,
  and no care in the language server can fix it: `jr_hir::TypeRef::Name` carries a symbol
  and no span, so there is no position to match a cursor against. A test pins the
  limitation and fails the day it stops being one (ADR-0028 §4).
- **Completion's idea of scope is "declared earlier in this body"**, not block scope. It
  over-offers — a local from a sibling block that has already closed — and never
  under-offers, which is the direction that would make the list feel broken.
- **`references` and `rename` cost 55 ms on their first call, and a reverse index would not
  help.** Both scan every workspace file, because ADR-0029 §3 discovers paths rather than
  loading them — but the split says where the time goes: **31 ms parsing, 24 ms lowering and
  resolving, 0.5 ms actually searching**. It is a cold-start cost paid once per session, and
  an index would have optimised the last 1% (ADR-0034) — which is what the previous handoff
  had already promised to build. Warm it is 0.53 ms, and 0.10 ms after an edit. The live lead,
  if this ever matters, is parsing the files in parallel.
- **A rename can refuse, and it will.** It refuses on a name collision, on a syntax error in
  any file it would edit, on a non-identifier, and when the workspace exceeded 10 000 files.
  That is deliberate (ADR-0030 §3) — a rename that half-completes leaves a broken build, and
  one that resolves a collision by shadowing leaves code that compiles and means something
  else — but it does mean the feature says no more often than a Rust user expects.
- **Completion's scope, and rename's, are not the same notion.** Completion offers locals
  "declared earlier in this body"; rename resolves them properly through `ResolveMap`. The
  first is an approximation, the second is not.
- **Nothing checks that a doc comment is true**, and nothing but the language server reads
  one. There are no doc tests and no `jr doc`.
- **A "did you mean" suggestion is a guess, and stays silent rather than guessing badly.**
  E0218 and E0212 offer the nearest field or type name within an edit distance that scales
  with length — and *nothing* for a name under three characters, because at that length every
  identifier is within reach of every other and the suggestion would carry no information.
  A missing suggestion is the common case.
- **The unused-import warning is a language-design position, not a lint.** Jai does not warn
  about one; Jairs does, because ADR-0014's flat import merge means an unused import silently
  enlarges the name space every identifier resolves against, and can turn a later declaration
  into an ambiguity error from a module the file never uses. It is deliberately conservative:
  an import is reported only when nothing in the file uses a name it provides, in either
  expression *or* type position.
- **A "flaky test" turned out to be a real bug that lost your diagnostics.** For several
  waves `opening_a_broken_file_publishes_diagnostics` hung intermittently and was recorded as
  flaky. It was not: the server queued the diagnostics job and *then* re-walked the workspace,
  and that write cancelled the job, which published nothing because a comment claimed the
  canceller would queue a replacement — true of an edit, false of a re-walk. Any client
  without a file watcher, which includes a plain `nvim`, silently got no diagnostics on open.
  Fixed and pinned by ADR-0032: **11 failures in 16 loaded runs before, 0 in 16 after**. It
  stayed hidden because it never reproduced on an idle machine, and because a test with no
  timeout does not fail — it waits.
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
