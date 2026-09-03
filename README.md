# Jairs

Jairs is a Jai-inspired systems language with compile-time execution, explicit
allocators, and no GC, RAII, or exceptions — compiled by a hand-written,
error-recovering compiler written in Rust.

## Status, honestly

**Pre-alpha.** Jairs source runs in a compile-time VM *and* compiles to a
native binary, and the two agree byte for byte — down to the line a trap
names. The language they agree about is deliberately tiny, but it now covers
structs, unions, tagged variants, enums, polymorphic procedures and structs,
compile-time reflection, `#insert`/`#code` metaprogramming, an
atomics-and-threads memory model, DWARF debug info in both native back ends,
and a Simp-shaped 2D graphics stack on SDL2. Every one of those claims has a
capability table behind it, kept honest at the end of every wave — if a table
and the code disagree, the code is right and the table has a bug.

- **1076** workspace tests (**1080** with the LLVM back end compiled in).
- **266** `.jr` corpus files, **184** accepted ADRs, **23** standard library
  modules.
- macOS arm64 is verified; Linux x86-64 is configured in CI and has never
  actually run, because `main` has never been pushed.

Read **[`docs/capabilities.md`](docs/capabilities.md)** for the full,
table-by-table inventory of what works, what is absent, and the sharp edges
you must know before you hit them. Read **[`PLAN.md`](PLAN.md)** §1.5 for
per-crate status and §7 for the current handoff, and **[`AGENTS.md`](AGENTS.md)**
for the wave-by-wave narrative of what each of those numbers cost to earn.

## What it looks like

```jr
#import "Basic";                       // module system: one module, one file

Point :: struct { x: s64; y: s64; }    // structs, one level

add :: (a: s64, b: s64) -> s64 {       // procs, single return
    return a + b;
}

MESSAGE :: "hello from Jairs\n";       // constants
COMPUTED :: #run add(2, 3);            // compile-time execution

main :: () {
    p: Point;                          // decls: typed, and inferred below
    p.x = 4;
    sum := add(p.x, COMPUTED);         // := inference
    if sum > 5  print(MESSAGE);        // if
    i := 0;
    while i < 3 { i = i + 1; }         // while
    ptr := *sum;                       // pointer take + deref
    if ptr.* == 9  print_int(9);
}
```

More in **[`examples/`](examples/)** — seven small, verified programs. They
cover structs, polymorphism, `#run`, the target-OS query, arrays and file I/O.

## Architecture

Hand-written lexer and parser over a lossless CST; HIR with module resolution;
lazy on-demand sema; a bytecode VM and a Cranelift/LLVM native path that share
one MIR; a salsa database that the LSP queries directly, not a forked
compiler. See **[`docs/architecture.md`](docs/architecture.md)** for the
pipeline diagram and the full crate-by-crate breakdown.

## Building and testing

```sh
# Requires Rust stable (pinned via rust-toolchain.toml).
cargo test --workspace

# Check formatting and lints before pushing:
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

That is two of the project's seven gates. `AGENTS.md`'s "The six gates"
section has the rest — the corpus format check, the tree-sitter drift check,
and the LLVM-gated seventh gate — plus the process traps that have bitten
before: two gates run at once and race a shared binary.

## Where to read more

- **[`PLAN.md`](PLAN.md)** — the roadmap, the wave order, and §7's current
  handoff to whoever picks this up next.
- **[`AGENTS.md`](AGENTS.md)** — working conventions, the wave rhythm, house
  style, and the detailed narrative behind every number above.
- **[`docs/capabilities.md`](docs/capabilities.md)** — what works, what is
  absent, and the sharp edges.
- **[`docs/architecture.md`](docs/architecture.md)** — the compiler pipeline
  and crate layout.
- **[`docs/adr/README.md`](docs/adr/README.md)** — all 184 accepted decision
  records.
- **[`docs/spec/`](docs/spec/)** — the language specification chapters.
- **[`examples/`](examples/)** — runnable programs, each verified.

## Licence

Licensed under either of Apache License, Version 2.0
(<https://www.apache.org/licenses/LICENSE-2.0>) or the MIT licence
(<https://opensource.org/licenses/MIT>), at your option, per `Cargo.toml`'s
`license = "MIT OR Apache-2.0"`. Neither `LICENSE-APACHE` nor `LICENSE-MIT`
is checked into this repository yet.
