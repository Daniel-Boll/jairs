# Jairs — a Jai-like systems language in Rust

**Working name:** Jairs · **Source extension:** `.jr` · **Host language:** Rust (2024 edition)

> [!NOTE]
> **Strategy: tracer bullet, then thickening waves.**
> We do *not* build the compiler layer by layer. We first drive one absurdly small language subset all the
> way through **every** component — lexer, parser, CST, HIR, Sema, MIR, VM, Cranelift, linker, FFI, stdlib
> module, LSP, tree-sitter, formatter — until `hello.jr` is a native macOS binary and the LSP gives hover on
> it. Everything works, badly. *Then* we thicken it one feature-slice at a time.

---

## 0. Locked decisions

| # | Decision | Choice |
|---|---|---|
| 1 | Fidelity | **Jai-inspired, own language.** You own the spec. |
| 2 | Backend | **Cranelift first, LLVM later** behind a `Backend` trait. |
| 3 | Compile-time execution | **Bytecode VM over our own MIR.** Host-independent, trappable. |
| 4 | Parser | **One hand-written error-recovering parser** (rowan lossless CST), shared by compiler + LSP. tree-sitter is a *separate* editor grammar, CI-gated against drift. |
| 5 | Stdlib | **Written in Jairs itself**, like Jai. Core + OS + tooling first, graphics stack later. |
| 6 | Scope | **Solo, long-term serious, macOS arm64 first.** Linux x86-64 green in CI from the first native binary. |
| 7 | Build order | **Vertical slice first, then feature waves.** No component is "phase 2". |

### 0.1 Design questions — now resolved

| Question | Decision | Consequence |
|---|---|---|
| `context` ABI | **Hidden trailing parameter**; `#c_call` procedures opt out | Encoded in MIR's calling convention from day one. Function *types* carry a context flag. |
| Integer overflow | **Always trap** | Needs explicit wrapping operators (`+%`, `-%`, `*%`) or hash functions, PRNGs, and checksums become unwritable. **Added to Tier A.** Trap = a real runtime panic with a source location, and a *compile error* when detectable at comptime. |
| Bounds checks | **Like Jai: a build setting**, visible in the IR | MIR carries explicit `bounds_check` ops that a build-config pass strips. `#no_abc` opts out locally. |
| String representation | `{data: *u8, count: s64}`, **not** NUL-terminated | `to_c_string()` bridges to `#foreign` via temporary storage. |
| Polymorph identity | **Structural, on interned comptime arguments** — *my call* | An instantiation is keyed by the tuple of resolved comptime arg IDs in the InternPool, so `sort(Entity)` from two files dedupes to one function. Errors *display* nominally (`sort($T = Entity)`) so users see intent, not a key. Structural is required anyway once `Type` is a first-class comptime value. |
| Comptime FFI | **Yes**, gated behind `#foreign_at_comptime` | VM needs libffi-style dynamic calls. Non-negotiable given build scripts must read files. |
| Error handling | **Start exactly like Jai** — multiple returns + `#must` | But: reserve an **effect row slot in the function type representation** now, so an effects system can be added later without re-typing every signature. Costs nothing today; saves a rewrite. |

---

## 1. The vertical slice — "Jairs-0"

> [!IMPORTANT]
> **Definition of done for the slice:** `jr build examples/hello.jr` produces a signed native arm64 binary
> that prints when run; `jr run` executes the same file in the VM; opening it in VS Code gives diagnostics,
> hover types, and goto-definition; Neovim highlights it via tree-sitter; `jr fmt` round-trips it; and the
> printing itself comes from a stdlib module **written in Jairs** that calls libc via `#foreign`.

### 1.1 The Jairs-0 language subset

Deliberately tiny. Everything else is a later wave.

```jr
#import "Basic";                       // module system: one module, one file

Point :: struct { x: s64; y: s64; }    // structs, one level

add :: (a: s64, b: s64) -> s64 {       // procs, single return
    return a + b;
}

MESSAGE :: "hello from Jairs\n";       // constants
COMPUTED :: #run add(2, 3);            // one trivial comptime call

main :: () {
    p: Point;                          // decls: typed, and inferred below
    p.x = 4;
    sum := add(p.x, COMPUTED);         // := inference
    if sum > 5  print(MESSAGE);        // if
    i := 0;
    while i < 3 { i = i + 1; }         // while
    ptr := *sum;                       // pointer take + deref
    print(MESSAGE);
}
```

Included: `s64`, `u8`, `bool`, `string`, `*T`, `struct`, procs, `:=` / `: T` / `::`, `if`, `while`, `return`,
arithmetic (**trapping**), comparison, assignment, field access, pointer take/deref, `#import`, `#foreign`,
one `#run`.

> [!NOTE]
> `u8` is in the slice, not deferred to W1, because the string representation
> (`{data: *u8, count: s64}`, ADR-0004) and the libc `write` signature are both spelled in `*u8`. Choosing
> "stdlib in Jairs" forced this: you cannot express the bottom of the standard library without it. `u8`
> arithmetic and the rest of the numeric tower still wait for W1.

Excluded from the slice: arrays, `for`, `defer`, `using`, enums, unions, polymorphs, macros, `#insert`,
overloading, multiple returns, `context`, RTTI, floats, and the remaining numeric types. Each arrives as a
wave.

### 1.2 The slice's stdlib — `modules/Basic/`, in Jairs

This is the proof that decision #5 works. Roughly 40 lines of Jairs:

```jr
// modules/Basic/module.jr
libc :: #system_library "c";

write :: (fd: s64, buf: *u8, count: s64) -> s64 #foreign libc "write";

print :: (s: string) {
    write(1, s.data, s.count);
}
```

> [!CAUTION]
> **Correction found while building this: `print_int` is NOT implementable in Jairs-0.**
> Turning a digit into a byte for `write` needs an `s64` → `u8` conversion, and `cast` is reserved
> until W1. Every alternative needs something the slice also lacks — a `[N]u8` buffer (W1), pointer
> arithmetic (not in the subset, and no type checker yet to stop it being written by accident), or
> libc `printf` (variadic, and the arm64 variadic ABI passes variadic arguments on the stack, so a
> non-variadic `#foreign` declaration would silently produce garbage). Integer printing therefore
> lands with `cast` in **W1**, and the slice's exit criterion prints strings only.
>
> This is exactly the kind of constraint the tracer-bullet ordering exists to expose, and it cost
> minutes instead of being discovered during W7's stdlib push.

> [!WARNING]
> This forces `#foreign`, the string ABI, and the module loader into the **first** slice. That is the
> intended consequence of "stdlib in Jairs" — you cannot defer FFI to a later milestone if the bottom of
> your standard library is a syscall.

### 1.3 Slice work breakdown

Every crate gets created and does real work. Nothing is stubbed out with `todo!()` at the architectural level.

```mermaid
flowchart LR
    subgraph SLICE["Jairs-0 vertical slice — all of this, minimally"]
        direction TB
        A["jr-base<br/>spans, interning, IDs"] --> B["jr-syntax<br/>lexer + parser + rowan CST"]
        B --> C["jr-fmt"]
        B --> D["jr-hir<br/>lower + resolve + modules"]
        D --> E["jr-pool<br/>InternPool: types + values"]
        E --> F["jr-sema<br/>check + infer"]
        F --> G["jr-mir<br/>SSA + inliner"]
        G --> H["jr-vm<br/>bytecode + FFI bridge"]
        G --> I["jr-codegen-clif"]
        I --> J["jr-link<br/>cc driver + codesign"]
        F --> K["jr-db<br/>salsa queries"]
        K --> L["jr-lsp"]
        B --> M["tree-sitter-jairs"]
        H --> N["modules/Basic in Jairs"]
    end
```

| Component | Slice scope | Why it can't wait |
|---|---|---|
| `jr-base` | Spans, `FileId`, `lasso` interner, `slotmap` arenas, newtype IDs | Everything depends on it |
| `jr-diag` | Diagnostic model + `annotate-snippets` rendering | Error quality is a design constraint, not a polish task |
| `jr-syntax` | Hand lexer (all tokens, trivia preserved), recursive-descent parser for the subset, **error recovery**, rowan CST, typed AST accessors | Recovery shape is hard to retrofit; LSP depends on it |
| `jr-fmt` | Formatter over the CST | Cheapest proof the CST is genuinely lossless |
| `jr-hir` | Lowering, module graph, `#import` resolution, scopes | Establishes the resolution model |
| `jr-pool` | InternPool for types **and** comptime values | Retrofitting interning is a rewrite. Steal Zig's design. |
| `jr-sema` | Lazy, on-demand checking; `:=` inference; one `#run` | Lazy-from-day-one is the whole reason M6-class comptime is possible later |
| `jr-mir` | Typed SSA, mem2reg, DCE, **a real inliner** | Cranelift has no inlining — it must exist before any backend |
| `jr-vm` | Register bytecode + interpreter + libffi bridge | It *is* the comptime engine; also gives `jr run` before any backend works |
| `jr-codegen` | `Backend` trait | Defining it now keeps LLVM from becoming a fork |
| `jr-codegen-clif` | Cranelift lowering, AArch64 AAPCS, Mach-O | The native path |
| `jr-link` | `.o` via `cranelift-object`, then shell out to `cc`; ad-hoc codesign | macOS binaries don't launch unsigned |
| `jr-db` | salsa 0.28 queries: file → tokens → CST → HIR → types | **The** reason the LSP won't be a fork of the compiler |
| `jr-lsp` | `lsp-server` loop: diagnostics, hover, goto-def | Proves the salsa boundary is real |
| `tree-sitter-jairs` | `grammar.js` + `highlights.scm` + corpus | Establishes the drift gate |
| `modules/Basic` | `print` / `print_line` via `#foreign` to libc `write` | Proves stdlib-in-Jairs |
| `tests/corpus` | ~20 `.jr` files: spec examples = parser tests = tree-sitter tests | One corpus, two parsers, CI-enforced |

### 1.4 Slice exit criteria

- [x] `jr fmt` byte-identically round-trips every corpus file — enforced by the
      `jairs-fmt` CI gate over `valid/`, `imports/valid/`, `tests/corpus/modules/`
      and `modules/`. `invalid/` is excluded on purpose: those files do not parse,
      and `jr fmt` correctly refuses to format input it could not parse.
- [x] `jr check broken.jr` recovers and reports multiple errors with rustc-grade
      rendering — `invalid/009-multiple-independent-errors.jr` asserts at least
      four independent errors from one file.
- [x] `jr run hello.jr` executes in the VM — `jr run tests/corpus/valid/024-hello.jr`
      prints both lines and exits 0. Asserted by `jr-cli`'s
      `run_executes_the_slice_exit_criterion`.
- [ ] `jr build hello.jr && ./hello` — native arm64, launches, correct output
- [x] `COMPUTED :: #run add(2,3)` folds at compile time — folded by `jr-db`'s
      `file_consts` query (ADR-0018 §3) and interned, so it is indistinguishable
      from a literal: the MIR snapshot for `020-run-directive.jr` now reads
      `5_s64 + 1_s64`. **VM and native agreeing is still open**, because there is
      no native.
- [x] `print` comes from `modules/Basic` written in Jairs, via `#foreign` to libc
      `write` — executes, through libffi (ADR-0018 §4). ADR-0004's `{data, count}`
      is handed to `write` with no copy.
- [ ] Integer overflow traps, in both the VM and native, with a source location —
      the VM traps (ADR-0002, `jr-vm`'s `execute.rs` pins every operator), but
      **without a source location**: MIR carries HIR ids rather than spans
      (ADR-0013), and nothing resolves a `MirSpan` back yet. Native is open.
- [ ] VS Code: diagnostics + hover + goto-def
- [ ] Neovim: tree-sitter highlighting — `grammar.js` and `queries/*.scm` exist
      and the drift gate is green; editor packaging is not done.
- [ ] CI green on macOS arm64 **and** Linux x86-64 — the matrix is configured for
      both; only macOS arm64 has been verified locally.
- [x] CI drift gate: every corpus file parses cleanly in *both* the compiler and
      tree-sitter
- [ ] Differential test harness exists: every corpus program's output must match
      under VM and native

**Estimated: 10–14 weeks solo.** This is the milestone that decides whether the project is real.

### 1.5 Where the slice actually is

Status of each slice component, so this is answerable without reading the tree.
"Done" means implemented, tested, and green under every CI gate — not polished.

| Component | Status | Notes |
|---|---|---|
| `jr-base` | **Done** | Spans, `FileId`, `lasso` interning, `newtype_index!`, source map |
| `jr-diag` | **Done** | Diagnostic model + `annotate-snippets` renderer |
| `jr-syntax` | **Done** | Lexer, error-recovering parser, rowan CST, typed AST |
| `jr-fmt` | **Done** | Formatter; corpus is canonical under it, CI-enforced |
| `jr-hir` | **Done** | Lowering, name resolution, flat import merge (ADR-0014) |
| `jr-pool` | **Done** | Types + comptime values in one pool (ADR-0015, ADR-0016 §3) |
| `jr-sema` | **Done** | Signatures + checking (ADR-0016). No const-eval: that is `jr-vm` |
| `jr-db` | **Done** | salsa queries: module loader, sema, MIR, const-eval, run (ADR-0007, ADR-0014, ADR-0018 §3) |
| `jr-cli` | **Done** | `jr check` (with `--module-path`), `jr fmt`, `jr parse`, `jr run` |
| `tree-sitter-jairs` | **Done** | Grammar + queries; drift gate green |
| `tests/corpus` | **Done** | 69 files, incl. `type-errors/` and `cfg-errors/` — one file per diagnostic |
| `modules/Basic` | **Done** | Written, resolving, type-checking and **executing**; MIR snapshotted |
| `jr-mir` | **Done** | Typed SSA, Braun construction, CFG diagnostics (ADR-0017). No mid-end |
| `jr-vm` | **Done** | Register bytecode, interpreter, libffi bridge (ADR-0018). No JIT tier |
| `jr-codegen`, `-clif`, `jr-link` | **Not started** | **Next.** The native half of §1.4 |
| `jr-driver`, `jr-lsp` | **Not started** | |

Accepted ADRs: 0001–0018. See [`docs/adr/README.md`](docs/adr/README.md).
Spec chapters written: 00 (overview), 01 (lexical), 02 (declarations),
03 (scoping and resolution). A type-system chapter is owed: ADR-0015 and ADR-0016
plus `jr-sema`'s crate docs are the only record of the typing rules today.

`jr-mir` has **no mid-end**: no inliner, no DCE, no const-prop, and no `mem2reg`
(ADR-0017 §2 makes the last one unnecessary rather than deferred). The wave that
adds one is §2.1's, and §5 puts the inliner in MIR deliberately. Its absence is
visible in a MIR dump: unreachable blocks survive, and `print_line` in
`modules/Basic` keeps a spill slot it never reads.

Layout now exists, in `jr-pool` (ADR-0018 §2). **`jr-codegen-clif` must call it
rather than computing its own** — that is the obligation ADR-0018 §2 exists to
create, and no verifier can enforce it.

---

## 2. Thickening waves

> [!IMPORTANT]
> **The rule that makes this work: a wave is not done until it has been pushed through every layer.**
> Every wave's checklist is the same eight items. If a feature parses but the LSP doesn't understand it, or
> tree-sitter can't highlight it, or the VM and native backend disagree about it, the wave is not done.

### 2.0 Per-wave definition of done

- [ ] **Spec** chapter written in `docs/spec/`
- [ ] **Corpus** files added (they *are* the spec examples)
- [ ] **Parser** + error recovery + `fmt`
- [ ] **Sema** + diagnostics with good spans
- [ ] **MIR** lowering + **VM** + **Cranelift**, verified equal by differential test
- [ ] **LSP** understands it (hover, completion, goto where applicable)
- [ ] **tree-sitter** grammar + highlight queries updated; drift gate green
- [ ] **Stdlib** uses it where it should (dogfooding is the acceptance test)

### 2.1 Wave order

| Wave | Content | Notes | Est. |
|---|---|---|---|
| **W1 — Data** | Full numeric tower (`s8..s64`, `u8..u64`, `float32/64`), wrapping ops `+% -% *%`, `enum`, `enum_flags`, `union`, `[N]T`, `[]T` views, `[..]T` dynamic arrays, `cast()`, `xx` autocast, operator overloading | Dynamic arrays need allocators → pulls `context` forward | 8–10 wks |
| **W2 — Flow & scope** | `for` with `it`/`it_index`, `for <`, labeled `break`/`continue`, `defer`, `using` (namespace + field promotion), multiple return values, named/default args, `#scope_*` visibility | `using` is the first genuinely hard resolution problem | 6–8 wks |
| **W3 — Runtime core** | `context` (hidden param, `#c_call` opt-out), allocators, temporary storage, bounds-check build config, panics/traps with backtraces | Unlocks a real stdlib | 6–8 wks |
| **W4 — Comptime** | Full `#run` (arbitrary code), aggressive const folding, RTTI (`Type` values, `type_info()`, `Any`), `#insert`, `#code`, the `Code` type | **Hardest wave.** Sema ↔ VM become mutually recursive; cycle detection with readable errors is the deliverable | 10–14 wks |
| **W5 — Polymorphism** | `$T`, `$$T`, `#modify`, `#bake_arguments`, `#expand` macros + hygiene, instantiation caching, **instantiation backtraces** in diagnostics | Depends on W4's InternPool value identity | 8–12 wks |
| **W6 — Metaprogram** | Workspaces, compiler message loop, `#run build()` build scripts replacing makefiles, plugin hooks, `@note` attributes | The Jai superpower. Build scripts become the build system. | 6–8 wks |
| **W7 — Stdlib** | In Jairs: `Basic`, `String`, dynamic array / hash table / bucket array, `Sort`, `Math` (vec/mat/quat), `Random`, `File`, `File_Utilities`, `Process`, `Thread` + atomics, `Time`, `Socket`, `JSON`, `Compiler` | Runs partly in parallel with W5/W6; each module is a wave-acceptance test | 14–18 wks |
| **W8 — Performance** | LLVM backend via `inkwell` (`--release`), inliner maturity, `#soa`, SIMD vectors, `#align`/`#place`, parallel Sema + parallel codegen, published compile-throughput number | Three-way differential testing: VM ≡ Cranelift ≡ LLVM | 10–14 wks |
| **W9 — Tooling depth** | Full LSP surface (completion, refs, rename, signature help, semantic tokens, **inlay type hints**, code actions), richer DWARF (locals, struct layouts) for lldb, VS Code + Neovim + Zed packaging | Incremental all along; this is the "make it excellent" pass | 8–10 wks |
| **W10 — Graphics, in Jairs** | `Window_Creation` (Cocoa via `#foreign`), GPU layer (Metal, then Vulkan), immediate-mode 2D renderer, image decode, immediate-mode UI, audio (CoreAudio/ALSA) | All *library* work, written in Jairs — no compiler changes. Gated on W5+W7. | 6+ months |

### 2.2 Wave dependency graph

```mermaid
flowchart LR
    S["Jairs-0<br/>slice"] --> W1["W1 Data"]
    S --> W2["W2 Flow & scope"]
    W1 --> W3["W3 Runtime core<br/>context, allocators"]
    W2 --> W3
    W3 --> W4["W4 Comptime<br/>#run, RTTI, #insert"]
    W4 --> W5["W5 Polymorphism<br/>$T, macros"]
    W5 --> W6["W6 Metaprogram<br/>build scripts"]
    W3 --> W7["W7 Stdlib in Jairs"]
    W5 --> W7
    W5 --> W8["W8 Perf + LLVM"]
    W7 --> W10["W10 Graphics in Jairs"]
    W5 --> W10
    S --> W9["W9 Tooling depth"]
    W9 -.->|"incremental,<br/>every wave"| W7
```

---

## 3. Architecture

### 3.1 Pipeline

```mermaid
flowchart TD
    SRC["Source .jr"] --> LEX["Lexer"]
    LEX --> PAR["Parser — recursive descent,<br/>error-recovering"]
    PAR --> CST["Lossless CST (rowan)"]
    CST --> FMT["jr fmt"]
    CST --> AST["Typed AST accessors"]
    AST --> HIR["HIR — desugar, module graph"]
    HIR --> RES["Name resolution<br/>scopes, using, imports"]
    RES --> SEMA["Sema — lazy, on-demand<br/>types + inference + const-eval"]
    SEMA <--> POOL["InternPool<br/>types AND comptime values"]
    SEMA <--> VM["Bytecode VM<br/>#run / #insert / folding<br/>+ libffi bridge"]
    SEMA --> MIR["MIR — typed SSA,<br/>monomorphized"]
    MIR --> OPT["Own mid-end:<br/>inline, mem2reg, DCE, const-prop"]
    OPT --> BC["Bytecode lowering"] --> VM
    OPT --> CLIF["Cranelift"]
    OPT --> LLVM["LLVM (W8)"]
    CLIF --> OBJ["Object emit"]
    LLVM --> OBJ
    OBJ --> LINK["cc driver + codesign"] --> EXE["Native binary"]
    SEMA --> MSG["Compiler message queue"] --> META["Build metaprogram"] --> SEMA
    SEMA -.-> DB[("salsa DB")]
    CST -.-> DB
    DB --> LSP["LSP server"]
```

> [!IMPORTANT]
> **The load-bearing invariant:** comptime and runtime execute *the same* MIR. The VM consumes bytecode
> lowered from the identical MIR that Cranelift consumes. Any other arrangement guarantees `#run` and
> runtime silently disagree. This is Zig's model (ZIR → Sema → AIR) and it is correct.

### 3.2 Crate layout

```
jairs/
├── Cargo.toml                  # workspace
├── crates/
│   ├── jr-base/                # spans, FileId, interning, arenas, newtype IDs
│   ├── jr-diag/                # diagnostic model + annotate-snippets rendering
│   ├── jr-syntax/              # lexer, parser, SyntaxKind, rowan CST, typed AST
│   ├── jr-fmt/                 # formatter over the CST
│   ├── jr-hir/                 # desugared tree, module graph, scopes, resolution
│   ├── jr-pool/                # InternPool: types + comptime values
│   ├── jr-sema/                # checking, inference, polymorph instantiation
│   ├── jr-mir/                 # typed SSA IR + mid-end (incl. inliner)
│   ├── jr-vm/                  # bytecode, interpreter, comptime FFI bridge
│   ├── jr-codegen/             # Backend trait + shared lowering
│   ├── jr-codegen-clif/        # Cranelift
│   ├── jr-codegen-llvm/        # LLVM (feature = "llvm")
│   ├── jr-link/                # object emit + linker driver + codesign
│   ├── jr-db/                  # salsa database — shared by driver and LSP
│   ├── jr-driver/              # workspaces, message loop, build metaprograms
│   ├── jr-lsp/                 # language server
│   └── jr-cli/                 # the `jr` binary
├── modules/                    # standard library, written in Jairs
├── examples/
├── tests/corpus/               # shared syntax corpus (compiler + tree-sitter)
├── tree-sitter-jairs/          # separate editor grammar
└── docs/{spec,adr}/
```

### 3.3 Infrastructure choices

Versions verified 2026-07-25. **Pin exact versions for `cranelift-*` and `salsa`.**

| Concern | Choice | Version | Why |
|---|---|---|---|
| CST | `rowan` | 0.16.1 | rust-analyzer's red/green tree. `cstree` 0.14 only if `Send` trees are needed later. |
| Lexer | hand-written | — | `logos` is regex-per-token; nested comments + `#`-directive context are awkward. ~400 lines you own. |
| Parser | hand-written | — | Error-recovery quality *is* LSP quality. No generator delivers this. |
| Diagnostics | `annotate-snippets` | 0.12.16 | Literally rustc's renderer → rustc-identical output. |
| Incrementality | `salsa` | 0.28.1 | From the slice, not later. RA-proven. API churns — pin exactly. |
| Backend | `cranelift-*` | 0.134.2 | x86-64 + aarch64 with Apple Silicon CC. **APIs are not semver-stable.** |
| Objects / DWARF | `object`, `gimli` | 0.39.1 / 0.34.0 | Emit `.o`, shell out to `cc`. Never hand-roll Mach-O linking. |
| LSP transport | `lsp-server` | 0.10.0 | Sync, you own threading — correct pairing with salsa. `tower-lsp` is stale (2023). |
| LSP types | `lsp-types` | 0.97.0 | Stale but standard; LSP 3.17. |
| Golden tests | `insta` | 1.48.0 | Snapshot CST/HIR/MIR/diagnostics. |
| Fuzzing | `cargo-fuzz` + `arbitrary` | 0.13.2 / 1.4.2 | Parser fuzzing from the slice. |
| Interning | `lasso`, `slotmap`, `bumpalo` | 0.7.3 / 1.1.1 / 3.20 | rustc style: intern everything, index by `u32` newtypes. |
| tree-sitter | `tree-sitter` + CLI | 0.26.11 | Editor grammar only. |
| Comptime FFI | `libffi` (Rust bindings) | — | Required by `#foreign_at_comptime`. |

> [!WARNING]
> **Cranelift has no function inlining**, and only limited loop optimization (LICM/GVN/const-fold via its
> egraph mid-end). Codegen is ~14% slower than LLVM, but it compiles ~10× faster. The inliner must live in
> *your* MIR mid-end — which you need anyway for `#expand` macros and comptime.

---

## 4. Timeline

| Phase | Cumulative (solo, serious) |
|---|---|
| **Jairs-0 vertical slice** — everything works, badly | **~3 months** |
| W1–W3 — a real, usable procedural language | ~8–10 months |
| W4–W5 — comptime + polymorphism (the Jai soul) | **~16–20 months** |
| W6–W7 — build metaprograms + stdlib in Jairs | ~24–30 months |
| W8–W9 — performance parity + excellent tooling | ~32–40 months |
| W10 — graphics stack, game-dev viable | **~42–48 months** |

> [!NOTE]
> These are honest numbers. Zig took ~8 years with a funded team; Odin has had a decade. The slice-first
> structure exists precisely so you have **a real, native, LSP-supported language in ~3 months** rather than
> a half-built type checker in 12.

---

## 5. Top risks

| Risk | Why it bites | Mitigation |
|---|---|---|
| **Sema ↔ comptime recursion** | `#run` yields types; types need Sema; Sema may need `#run`. Pass-ordered checkers can't express this. | Lazy on-demand Sema over salsa **in the slice**. Explicit dependency graph + cycle diagnostics. Copy Zig's Sema/InternPool. |
| **No inlining in Cranelift** | Macro- and comptime-heavy code is slow; `#expand` assumes inlining. | Inliner in MIR during the slice, before any backend exists. |
| **Always-trap overflow with no wrapping ops** | Hash functions, PRNGs, checksums become unwritable — discovered while writing the stdlib. | `+% -% *%` promoted into **W1**. |
| **Stdlib-in-Jairs pulls FFI early** | The bottom of the stdlib is a syscall. | `#foreign` is in the **slice**, by design. |
| **Errors inside polymorph instantiations** | #1 reason generics feel bad. | Instantiation backtraces designed into `jr-diag` in the slice, not retrofitted in W5. |
| **LSP as a fork of the compiler** | Batch compilers assume "parse all then check"; IDEs need incremental + error-tolerant. | salsa in the slice; the LSP is a *consumer* of the same queries. Never two frontends. |
| **tree-sitter drift** | Two grammars always diverge. | Shared `tests/corpus/`; CI gates both parsers; grammar changes require a corpus file. |
| **Cranelift API churn** | Explicitly not semver-stable. | Pin exactly; confine all contact to `jr-codegen-clif` behind `Backend`. |
| **macOS arm64 specifics** | Codesigning, `ld-prime`, DWARF quirks. | Always link through `cc`; ad-hoc codesign in `jr-link`; keep Linux green as a sanity oracle. |
| **Scope creep into graphics** | Most tempting, most destabilizing. | Hard gate: W10 starts only after W7. It requires *zero* compiler changes — that's the test of readiness. |
| **Solo burnout** | The real killer of language projects. | Every wave ends in a runnable demo. `fmt` and LSP ship in month 3. |

---

## 6. Prior art to mine before W4

| Project | Impl | Backend | Comptime | Steal |
|---|---|---|---|---|
| **Zig** | Zig | LLVM + own x64/aarch64/wasm | **Sema interprets ZIR → AIR, global InternPool** | *The* reference. InternPool, lazy Sema, "interpret the same IR you compile". Read `Sema.zig` + `InternPool.zig`. |
| **roc** | **Rust** | Cranelift (dev) + LLVM (release) | — | Dual-backend split behind one trait; Rust codegen patterns; monomorphization. |
| **rust-analyzer** | Rust | — | — | rowan CST, salsa design, `lsp-server` loop, error-recovering parser structure. |
| **Odin** | C | LLVM | Limited | Clean multi-pass checker; package/scope model. |
| **starlark-rust** | Rust | Bytecode interp | — | Fast bytecode eval + freezing, for `jr-vm`. |
| **rune / koto** | Rust | Register/stack VM | — | VM value model + instruction encoding. |
| **gleam** | Rust | Erlang/JS | — | Best-in-class CLI ergonomics and diagnostics polish. |

> [!NOTE]
> No production-grade open-source Jai reimplementation exists — only toys. **Zig is the reference for
> comptime; roc is the reference for a Rust-hosted dual-backend compiler.**

---

## 7. Immediate next actions

Everything through the **VM** is done: workspace scaffolding, the ADRs, spec chapters
00–03, the corpus plus the drift gate, the lexer→parser→CST→`jr fmt` inch, HIR and
name resolution, the module loader, the InternPool, `jr-sema`, `jr-mir`, and `jr-vm`.
See §1.5 for component status.

**`jr run tests/corpus/valid/024-hello.jr` prints its two lines and exits 0.** That is
half of §1.4's exit criterion; the other half is native.

### What `jr-vm` landed

- [x] **ADR-0018**, five decisions with their rejected alternatives: a register
      bytecode addressed by `ValueId`; layout in `jr-pool` with the target passed in;
      const evaluation as a `jr-db` query rather than a fold in `jr-sema`; foreign
      calls through libffi behind an execution *mode*; and `Callee::Direct` widened to
      a `(FileId, ProcId)` pair.
- [x] **Layout** (`jr-pool`'s `layout` module). Size, alignment and field offsets, with
      `TargetLayout` a parameter so nothing reads the host implicitly. **This is where
      ADR-0004 stopped being prose**: `string` is a real `{data: *u8, count: s64}` with
      computed offsets, so MIR's `StringData`/`StringCount` are numbers now.
- [x] **The bytecode and its lowering** (`jr-vm`'s `code.rs`, `lower.rs`). Blocks
      linearised in `reverse_postorder()`, every projection resolved to a byte offset,
      and block parameters replaced by *parallel* copies on edges — which ADR-0017 §1's
      no-critical-edges invariant is what makes unambiguous.
- [x] **The interpreter** (`interp.rs`), with ADR-0002's trapping arithmetic done in
      `i128` and range-checked, so `+` traps at the destination type's boundary and
      `+%` is the opt-out. `memory.rs` is one non-moving linear region, addressed by
      offset, with frames as a stack mark.
- [x] **The libffi bridge** (`ffi.rs`), with `Mode::Comptime` refusing a foreign call
      until wave W6's `#foreign_at_comptime` — ADR-0006's distinction, finally with
      somewhere to live.
- [x] **Const evaluation** (`jr-db`'s `file_consts`), a fixpoint that lowers thunks for
      `#run` and file-level constants, runs them, and interns the results.
- [x] **`jr run`** (`jr-cli`), and `jr-db`'s `run_main` which assembles every reachable
      file so a cross-file call has a callee.
- [x] **All three MIR refusals are gone.** `crates/jr-db/tests/mir_corpus.rs` now
      asserts that *no* body in the valid corpus is refused, and the snapshot diff that
      deleted the three `poisoned:` lines is the proof.

Four things were decided or discovered that the plan had not anticipated:

- **A second silent miscompile was found and fixed, in `modules/Basic`'s `print`.**
  A field of an aggregate *parameter* had no place, so `s.data` and `s.count` lowered
  to `Rvalue::Undef` — with no diagnostic and no refusal, because `Undef` is a
  well-typed value rather than poison and the verifier had nothing to object to.
  `write` would have been handed a garbage pointer. Fixed by spilling an aggregate
  parameter to a slot at entry, and — more importantly — by making a `None` from a
  place or callee helper **refuse the body** (`Lower::give_up`) instead of emitting a
  placeholder. That is the same shape as the previous wave's braceless-body bug, and
  the class is now closed rather than the instance.
- **The corpus could not have caught it.** `modules/Basic` is not in
  `tests/corpus/valid/` and `file_mir` is per file, so the stdlib's own bodies never
  appeared in any snapshot. There is now a `basic_module_mir` snapshot.
- **PLAN.md §7 was self-contradictory**, and ADR-0018 §5 resolves it: `jr run` plus
  §1.4's exit criterion were in scope while the cross-file-call refusal was assigned
  to the inliner, and `024-hello.jr` needs both. `Callee::Direct` now carries a
  `ProcRef`, resolved from the callee's *signature* — never its body — so ADR-0017 §3's
  rule that the built-MIR query has no cross-body dependencies still holds.
- **`exit` is not the host `exit`.** Calling it would terminate the compiler mid-build,
  so the VM returns `VmError::Exited(status)` and `jr run` turns it into the process
  status. It is the one symbol whose C behaviour the VM deliberately does not reproduce.

Diagnostic codes: **E0231 is the first free code.** E0230 is `jr-db`'s const-eval
failure; E0227–E0229 are `jr-mir`'s. Beware that `jr-syntax`' parser still illegally
emits E0200/E0201/E0202 for "arrives in wave Wn" errors, colliding with `jr-hir` — do
not filter tests by those.

### Next: implement `jr-codegen-clif` and `jr-link`

This is the other half of §1.4: the same MIR, through Cranelift, to a native binary
whose output matches the VM's byte for byte.

#### Read first, in this order

1. This section, then §1.5 for status and §3.1 for the same-MIR invariant.
2. **ADR-0018, all of it**, and especially §2. The VM is the *first* consumer of
   layout; you are the second, and the whole reason layout went into `jr-pool` is that
   you must not compute your own.
3. **ADR-0017**, which is `jr-mir`'s specification.
4. `crates/jr-vm/src/lower.rs` — it already turns MIR into a linear instruction
   stream with resolved offsets. Cranelift wants a different shape, but every question
   about *what MIR means* is answered there first.
5. ADR-0009 (`cranelift-*` is `=`-pinned and must stay inside `jr-codegen-clif`),
   ADR-0002 (overflow traps), ADR-0003 (bounds checks are a build setting).

#### What is already done for you

- `MirBody::reverse_postorder()` is the block order; `predecessors()` is cached.
- **Block parameters map 1:1 onto `append_block_param`**, which is exactly why
  ADR-0017 §1 chose them over phi statements — there is no unphi pass to write.
- **Slots map onto Cranelift stack slots** with `stack_addr`, which is why ADR-0017 §2
  put escaped locals in memory during lowering.
- `jr_pool::{layout_of, field_offset, string_data, string_count}` give you every byte
  offset, and the VM already agrees with them.
- `jr_db::run_main` shows how to assemble every reachable file; a native build needs
  the same walk.
- `jr-vm`'s `tests/execute.rs` is 34 assertions about what each construct *means*.
  It is the differential oracle §1.4 asks for: a native build that disagrees with any
  of them is wrong.

#### Work items, in dependency order

- [ ] **The `Backend` trait in `jr-codegen`**, so `jr-codegen-clif` and the eventual
      `jr-codegen-llvm` are interchangeable and CONTRIBUTING's rule that Cranelift API
      contact stays inside `jr-codegen-clif` is structural rather than remembered.
- [ ] **MIR → Cranelift IR.** Blocks, block parameters, slots, the arithmetic, and the
      traps. ADR-0002 means `+` compiles to a checked add plus a trap block, not a bare
      `iadd`.
- [ ] **Layout via `jr-pool`.** Do not write a second one. See ADR-0018 §2.
- [ ] **`#foreign` as a real relocation**, rather than the process-local `dlsym` the VM
      uses. `ForeignInfo::library` is *still* an unresolved `Option<Symbol>`; this is
      the third independent resolution of it, which ADR-0018 §4 names as the signal to
      intern the answer beside `Item::ForeignLibraryValue`.
- [ ] **`jr-link`** and `jr build`.
- [ ] **The differential harness** §1.4 asks for: every corpus program's output must
      match under VM and native.
- [ ] **A source location on a trap.** ADR-0013 deferred `AstIdMap`, so `MirSpan`
      carries HIR ids and nothing resolves one back to a span. Both back ends need this
      and neither has it; §1.4 lists it as unmet for exactly this reason.

#### Traps

- **Do not compute layout.** Said three times because it is the one mistake that
  produces a *silent* comptime/runtime divergence, which is the failure ADR-0018 §2
  exists to prevent.
- **`Rvalue::Undef` is not poison.** It is a well-typed value that was never assigned.
  The VM traps on use; a native build may do anything, but it must not silently read
  zero — that is what hides E0227.
- **`Terminator::Unreachable` has three reasons** and only `Trap` is a program the
  compiler believes is well-formed.
- **`Pool::is_type(PoolId::ERROR)` is `true`.** Never use `is_type` as an error gate.
- **The arena trap, still.** `FileHir::exprs` and every `Body::exprs` start at 0;
  `MirSpan::Expr` carries an `ExprScope` for exactly that reason, and `ConstValues`
  keys `#run` values the same way.
- **A dead `ValueId` is normal**, and so is a dead slot: `print_line` spills a
  parameter it never projects, because the spill is unconditional. A DCE pass would
  remove both.
- **A cross-file call is now representable but a cross-file `#run` is not.** The const
  query is per file; evaluating a `#run` that calls into another module needs the
  cross-body read ADR-0017 §3 keeps out of the built-MIR query.

#### Decisions to put to the decider before writing code

Every wave so far has settled its design forks *first*, via an ADR, and every one of
them was expensive to undo. These are the forks this wave has, stated with the options
so the conversation can start from them rather than from a blank page. **ADR-0019 is
the next free number.**

1. **What shape is the `Backend` trait?** One `compile_body(&MirBody) -> ()` call with
   the backend owning module state, or a finer interface (declare, define, finalise)
   that the driver sequences. The finer one is what incremental recompilation
   eventually wants and what `cranelift-module`'s `Module` trait already looks like;
   the coarse one is smaller today. Whichever is chosen, ADR-0009 and CONTRIBUTING
   require that *no* `cranelift-*` type appear in the trait.
2. **How does an ADR-0002 trap become Cranelift IR?** Three real options: `trapif`-style
   flag checks after each arithmetic op; an explicit compare-and-branch to one shared
   trap block per procedure; or a call into a runtime helper that reports and aborts.
   The third is the only one that can carry a *message*, which matters because §1.4
   wants a source location on a trap — and the VM currently produces one and native
   would produce none. The first two are faster and mute.
3. **Where does `AstIdMap` land?** ADR-0013 deferred it, and both back ends now need it
   for the same reason: `MirSpan` names an HIR node and nothing resolves one back to a
   span. Options: build it in `jr-hir` as ADR-0013 anticipated; make it a `jr-db` query
   over the CST; or give MIR real spans after all and accept the invalidation cost
   ADR-0013 rejected. This is the last unmet §1.4 criterion that is not "there is no
   native", so it is not deferrable much further.
4. **How is `#foreign` resolved for a native build?** The VM uses a process-local
   `dlsym`, which a linked binary cannot. This is the *third* independent resolution of
   `ForeignInfo::library`, and ADR-0018 §4 already names a third as the signal to intern
   the answer in the pool beside `Item::ForeignLibraryValue`. So the fork is really
   "intern it now, or resolve it a third time and intern it on the fourth".
5. **Does `jr-codegen-llvm` stay a stub?** It is a declared crate with no contents and
   wave W8 owns it. Worth confirming it is out of scope rather than assuming.

#### Where things are, as of this handoff

New since the MIR wave, so a reader knows which file answers which question:

| Path | What it owns |
|---|---|
| `crates/jr-pool/src/layout.rs` | Size, align, field offsets, `TargetLayout`. **The one layout.** |
| `crates/jr-mir/src/inputs.rs` | `ConstValues`, `ImportedProcs` — the two maps lowering is handed |
| `crates/jr-mir/src/thunk.rs` | A file-level expression → a runnable `MirBody` |
| `crates/jr-vm/src/code.rs` | The bytecode ISA, `PlacePlan`, `Routine`, `ForeignProc` |
| `crates/jr-vm/src/lower.rs` | MIR → bytecode: linearise, resolve offsets, kill block params |
| `crates/jr-vm/src/interp.rs` | `Program`, `Vm`, `Mode`, the instruction loop, ADR-0002 arithmetic |
| `crates/jr-vm/src/memory.rs` | One non-moving linear region; frames as a stack mark |
| `crates/jr-vm/src/value.rs` | `Value`, `IntKind`, the `i128` range checks |
| `crates/jr-vm/src/ffi.rs` | libffi bridge, `dlsym`, the comptime refusal |
| `crates/jr-vm/src/assemble.rs` | HIR + MIR + signatures → a `Program` |
| `crates/jr-vm/src/error.rs` | `VmError` (trap / unsupported / internal / exhausted / exited) |
| `crates/jr-db/src/consts.rs` | `file_consts`: the const-eval fixpoint, E0230 |
| `crates/jr-db/src/run.rs` | `run_main`, `main_of`, the reachable-file walk |
| `crates/jr-db/src/mir.rs` | `file_mir`, `imported_procs`, the ADR-0017 §4 gate |
| `crates/jr-cli/src/commands/run.rs` | `jr run` and its exit codes |

Tests worth knowing about before changing anything:

| Path | What it pins |
|---|---|
| `crates/jr-vm/tests/execute.rs` | 34 assertions about what each construct *means*. The differential oracle. |
| `crates/jr-db/tests/mir_corpus.rs` | No body in `valid/` is refused; the exit criterion lowers; `modules/Basic` is snapshotted |
| `crates/jr-mir/tests/lowering.rs` | ADR-0017's decisions, and the two silent-miscompile regressions |
| `crates/jr-cli/tests/integration.rs` | `jr run` exit codes, including that a file with errors is not executed |

#### Gates — all six must pass

`cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`;
`cargo test --workspace` (596 tests as of this handoff); `RUSTDOCFLAGS="-D warnings"
cargo doc --workspace --no-deps`; `cargo run -q -p jr-cli -- fmt --check
tests/corpus/valid tests/corpus/imports/valid tests/corpus/type-errors
tests/corpus/cfg-errors tests/corpus/modules modules`; corpus-drift via
`npx --yes tree-sitter-cli@0.26.11` (tree-sitter is not installed locally).

House style is enforced by the first four: `[lints] workspace = true` and no
crate-level `#![warn]`, `missing_docs` is a warning workspace-wide so every public
item including enum variants and struct fields needs a `///`, private `mod` plus a
curated `pub use` in `lib.rs`, module `//!` docs that argue *why* and name the
rejected alternative, and exhaustive matches rather than `matches!` or guards so
that a new variant is a compile error.

One process note, repeated because it cost real time on two waves running:
**subagents were unreliable.** Write the modules that define an API yourself;
delegate only single-file work with the consumed signatures stated verbatim.

### Known latent issues, none blocking

- The parser's E0200/E0201/E0202 collision described above.
- `Stmt::Item` and `FieldId` are declared but never constructed. Both are matched
  exhaustively anyway, so the day one is constructed the arm is the thing to change.
- An imported module's signatures are recomputed once per importer inside the
  signature phase. ADR-0016 §5 forbids the obvious fix, and interning is idempotent,
  so the cost is time rather than correctness. `file_consts` compounds it: it lowers
  the file once per fixpoint round, which is one or two in practice.
- A name that an *imported* module itself imported is invisible to the importer's
  signature phase and resolves to poison.
- The most negative value of a signed type cannot be written as a literal: the HIR
  stores a magnitude and `-1` is negation applied to `1`.
- `Pool` never shrinks; `jr-pool`'s docs record the remap-pass escape hatch.
- A default-initialised local of *pointer* type is treated as uninitialised rather
  than null, because the pool interns no null pointer. `Memory` reserves address 0 for
  it, so interning one is all that is missing.
- A `#run` producing a struct is refused: ADR-0015's `Item` has no aggregate-value
  variant. A `#run` producing a *string* works, by copying the bytes out of VM memory
  before the VM is dropped.
- Calling through a procedure pointer is refused: the pool interns a procedure as an
  `Item::ProcValue { decl }`, a `DeclId`, and nothing maps a `DeclId` to a `ProcRef`.
- The VM's memory is 1 MiB and never grows, because growing would move the base and
  dangle a host pointer held across a foreign call. Frame size is `value_count()`
  rather than a live range, so a body with many short-lived values over-allocates.
- `jr-mir` has no mid-end, so nothing folds constants or removes dead blocks.

### After the native back end

§1.4's remaining criteria: the LSP (diagnostics, hover, goto-def), editor packaging,
and CI verified on Linux x86-64 as well as macOS arm64.
