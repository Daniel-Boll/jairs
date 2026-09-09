# Jairs

Jairs is a Jai-inspired systems language with compile-time execution, explicit
allocators, and no GC, RAII, or exceptions — compiled by a hand-written,
error-recovering compiler written in Rust.

## Install

```sh
cargo install --path crates/jr-cli
```

That is the whole procedure. The standard library is compiled into the binary, so there is nothing
to unpack, no environment variable to set, and no `-I` to remember — `jr` works from any directory,
with the source tree deleted (ADR-0202 §1).

## Start a project

```sh
jr new hello && cd hello
jr run                 # Hello from Jairs!
jr build               # ./hello
```

`jr new` writes a `jairs.toml`, a `src/main.jr` that compiles on the first try, an inert `build.jr`
showing the build-script form, and a `.gitignore`. Inside a project, `jr run`, `jr build`,
`jr check` and `jr fmt` need no arguments, from any subdirectory.

The manifest is an **override, never a requirement** — every command works without one, and an
explicit flag always outranks the file. An unrecognised key is an error rather than being ignored,
so a typo is reported instead of silently doing nothing:

```toml
[project]
name = "hello"
# entry = "src/main.jr"        # what a bare `jr build` / `jr run` / `jr check` compiles

[dependencies]
# Geometry = { path = "../geometry" }       # exactly ../geometry/module.jr
# Noise = { path = "../vendor/noise.jr" }   # exactly this file

[fmt]
indent_style = "space"         # "space" or "tab"
indent_width = 4               # spaces per level; not read when indent_style = "tab"
max_width = 100                # breaks a long argument or parameter list. Comments are
                               # never reflowed, so a longer line can still survive.

[build]
# module_paths = ["vendor"]    # legacy directory search; prefer exact dependencies.
```

In a manifest-backed project, `src/Foo.jr` and `src/Foo/module.jr` are both importable as
`#import "Foo"` without `-I`. Dependencies are exact: naming a directory exposes only its
`module.jr`, not neighbouring modules. The CLI, build driver, database, and language server all
consume the same project catalog (ADR-0213).

## Status, honestly

**Pre-alpha, current through ADR-0214.** Jairs source runs in a compile-time VM *and* compiles to a
native binary, and the two agree byte for byte — down to the line a trap
names. The language they agree about is deliberately tiny, but it now covers
structs, unions, tagged variants, enums, polymorphic procedures and structs,
compile-time reflection, `#insert`/`#code` metaprogramming, an
atomics-and-threads memory model, DWARF debug info in both native back ends,
file-scope mutable state, a **Simp-shaped 2D graphics subset** that draws through
OpenGL, and **build scripts written in the language
itself**. It **installs with one command** and carries its own standard
library, so `cargo install` is the whole procedure and `#import "Basic"` needs
no search path. Manifest projects also discover direct modules under `src` and exact named path
dependencies through one catalog shared with the editor and build driver. Every one of those claims has a capability
table behind it, kept honest at the end of every wave — if a table and the code
disagree, the code is right and the table has a bug.

Byte-oriented source scanning now has the Jai-shaped pieces it needs: a `for` can walk a
`string` directly as `u8` bytes with `s64` byte offsets, and `#char "A"` supplies a
context-typed ASCII byte for comparisons. Unicode decoding remains explicit.

**A project can be built by a Jairs program.** `jr build build.jr` compiles the script, runs it, and
performs the compilations it asked for — no flag, because importing `modules/Compiler` is what makes a
file a build script:

```jairs
Compiler :: #import "Compiler";

main :: () {
    git := Compiler.command("git");
    Compiler.argument_of(git, "rev-parse");
    _ = Compiler.run(git);                       // shells out, and reads what it said

    t := Compiler.create_target("app");
    o := Compiler.options(t);                    // read the defaults, then change what you care about
    o.output = "app";
    Compiler.set_options(t, o);
    Compiler.add_file(t, "src/main.jr");
    if !Compiler.build(t) { exit(1); }
}
```

The design came from reading 23 real `build.jai` files, because Jai's own compiler module is unpublished
— and copying Jai's shape would not have worked. Jairs now supports two honest forms: a driver-run
`main` script can execute commands and compile targets immediately, while a Jai-shaped top-level
`#run` can allocate, read and write files, run commands and record build requests, but cannot start a
compilation from inside the compiler query that is evaluating it. See
[ADR-0195](docs/adr/0195-build-script.md), ADR-0196 through ADR-0198, and
[`examples/10-build-script.jr`](examples/10-build-script.jr).

The language gained five utilities it had owed for several waves: typed constants
(`FLAG : u32 : 256`), array literals (`s64.[1, 2, 3]` — the most used construct
real Jai code has and this did not), `type_of(x)`, a pointer type as an
intrinsic's argument, and reflection over an enum's member names. Twenty casts
disappeared from `modules/GL`, and `print` now shows `BLUE` rather than `2`.

A program can report what it computed. `print("x = %, ok = %\n", 42, true)`
is written in Jairs, over the variadic and the reflection the compiler already
had — every integer width including the most negative one, floats, `bool`,
pointers, a struct by field name. Before it the library could print a string and
one non-negative integer, and the one integer it could not print was the most
negative, which is the first thing anyone tests.

That was the first thing to use four of this compiler's features at once, and
using them found four defects in code that had shipped and been believed: three
guards that hid the standard library's own types from itself, and an assumption
that the inliner could not move a global reference across files, made by the
same decision that guaranteed it could.

The graphics API was not designed here either. It follows the no-state,
immediate-mode shape observed in public vendored copies of Jai's `Simp`, but
those copies disagree on render targets, batching and text. Jairs therefore
claims a **Simp-shaped subset**, not one canonical closed-beta signature set.
The first comparison still found eight local mismatches, two non-cosmetic: the
coordinate origin was upside down, and every call took a state argument the
observed no-state family does not have.
Removing that argument needed a language feature first — a variable at the top
level of a file, which the compiler could parse and could not compile.

- **1260** workspace tests (1269 under gate 7), all seven gates green.
- **285** `.jr` corpus files, **214** accepted ADRs, **25** standard library
  modules.
- **Fast test feedback without weakening the gate.** `scripts/check fast` runs in about 13 seconds
  and `scripts/check pre-commit` in about 36 seconds on the development machine. The authoritative
  `scripts/check full` still runs ordinary Cargo with doctests; sharding its two exhaustive sweeps
  reduced the measured warm gate from 222.59 to 122.54 seconds (ADR-0209).
- **Both platforms are verified green.** macOS arm64 locally, gate by gate, and
  **x86-64 Linux in CI** — all seven jobs passing, which had never happened
  before. Getting there took eight fixes read out of eight consecutive CI runs
  (ADR-0205, ADR-0206). The last one was a **silent miscompile**: a 32-byte
  four-`float64` struct — a `CGRect` — was passed in four SSE registers where
  System V requires it on the stack, because the classification had AAPCS64's
  homogeneous-aggregate rule and System V has no such rule at all. The eighth was
  a MIR snapshot that had been wrong on x86-64 since `os()` became a compile-time
  value, because only macOS ever generated it.
- **Three games, built and run.** [`examples/games/`](examples/games/) holds Pong,
  Snake, and a sprite-and-widget demo, built by a Jairs build script
  ([`examples/games/build.jr`](examples/games/build.jr)) and runnable under a frame
  budget so a checker can drive them. Two of them keep their **rules** in a module
  that imports no graphics module, so `jr run` plays a whole match with no display
  attached — which is the one structural lesson worth copying, because a drawing
  program cannot run in the comptime VM at all. All three build, link and run, and
  no queued GL error was observed during their checked frames. That does not prove
  shader results or pixels; nobody has compared the pixels to a reference image.
- **The first `Game` facade slice exists.** One caller-owned `Game.App` now owns SDL,
  window and Simp startup, one event drain per frame, close latching, monotonic delta
  time, presentation and idempotent cleanup. It is deliberately only the lifecycle
  foundation: held input, drawing helpers, resources, PNG, text and audio remain later
  slices, and the beginner tutorial stays on the lower-level stack until PNG and text exist.
- **The documentation site has a fourth book.**
  [`docs-site/`](docs-site/) gained *Games with Jairs*: an outcome-first path from
  a headless simulation to a window, drawing, timing, textures, UI and the three
  examples, followed by a maintained inventory of what a game **cannot** do yet.
  The other three books were roughly sixty ADRs stale and have been reconciled
  against the code.

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
scripts/check fast        # broad inner-loop feedback
scripts/check pre-commit  # all but the two exhaustive corpus-wide sweeps
scripts/check full        # authoritative cargo test --workspace

# Check formatting and lints before pushing:
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

The first two lanes require `cargo-nextest`; `scripts/check full` does not.
Fast lanes are feedback, not release evidence: wave completion still requires
the unchanged `cargo test --workspace` gate. `AGENTS.md`'s "The six gates"
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
- **[`docs/jai-parity.md`](docs/jai-parity.md)** — what real Jai code uses that
  this does not, syntax and libraries, each traced to a source and probed where
  a probe was possible.
- **[`docs/jai-game-development-audit.md`](docs/jai-game-development-audit.md)** —
  the primary-source games audit, language/library gaps, and the staged `Game`
  facade plan whose foundation is now implemented.
- **[`docs/adr/README.md`](docs/adr/README.md)** — all 213 accepted decision
  records.
- **[`docs/spec/`](docs/spec/)** — the language specification chapters.
- **[`examples/`](examples/)** — runnable programs, each verified.

## Licence

**Public domain**, under [The Unlicense](UNLICENSE) — do anything you like with
this, with no conditions and no attribution required. `Cargo.toml` declares
`license = "Unlicense"`, which is the SPDX identifier.
