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
indent_width = 2               # new projects use two spaces; not read for tabs
case_block_style = "next_line" # or "same_line" for `case .TEXT; {`
struct_literal_trailing_comma = true # final comma in non-empty multiline struct literals
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

**Pre-alpha, current through ADR-0242.** Jairs source runs in a compile-time VM *and* compiles to a
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

Same-type pointers can now be subtracted: `end - start` yields the signed `s64` number of
pointee elements between them, so `*u8 - *u8` is a byte count while `*T - *T` is scaled by
`size_of(T)`. Different pointer types and zero-sized pointees are rejected. As with the existing
raw-pointer offset operation, both pointers must describe one allocation; exact divisibility and
the `s64` range are the program's responsibility.

Raw pointers can now be indexed directly: `p[i]` is the unchecked, element-scaled place
`(p + i).*`, so it works for reads, writes and address-taking, including `string.data[0]`.
`p += n` and `p -= n` advance by pointee elements; every other pointer compound remains an error.
Existing pointer-to-array/vector/view indexing keeps its bounded-container meaning. An optimiser
regression found by this feature is pinned too: `-O1` no longer mistakes an indirect pointer-index
write for a dead store to the temporary slot holding the pointer.

Structs can now be constructed as expressions. `Point.{x = 1, y = 2}` names its type;
`.{x = 1, y = 2}` inherits one from a return, annotation, assignment, argument, or containing
field. Entries may be named in any order or positional in declaration order, omitted fields are
zeroed, and initializer expressions retain source order. The same form constructs `string` through
its public `data` and `count` fields, which makes the counted-string scanners in
`ora_to_atlas_test` check cleanly. Unions, variants, views, dynamic arrays, and direct file-scope
aggregate literals remain separate decisions.

Jai-shaped control flow can now be written without a parallel implementation: `then` optionally
marks one braceless `if` statement, and `if #complete value == { case ... }` uses the same
exhaustiveness and execution path as `switch`. Enum aliases are covered by runtime value, duplicate
integer branches are rejected, and `else` must be final. When E0258 finds missing enum or tagged-
variant alternatives, the language server's preferred `add all missing cases` action inserts the
explicit empty arms in declaration order; it never hides future additions behind an `else`.

`todo;`, `todo()` and `todo("static description")` now mark an intentionally unfinished path.
They are terminal even in a valued procedure, trap only if executed, name their own source line in
the VM, Cranelift and LLVM, and do not turn failure into structured cleanup by running pending
defers. The optional literal description is escape-decoded once and reported as
`reached todo: description` byte-identically in all three engines. The same construct is legal in
compile-time code: an untaken path is inert and a reached one reports E0230 with the description.

`assert(condition)` and `assert(condition, "static message")` now provide the corresponding
source-located check. A failed assertion reports one byte-identical reason, call-site line and live
backtrace in the VM, Cranelift and LLVM, while a true assertion is inert. The intrinsic may be
shadowed by an ordinary declaration; a reached compile-time failure reports E0230.

Procedure result positions may now carry declaration-only labels:
`-> (value: s64, found: bool)`. Labels are preserved by formatting, Tree-sitter and LSP signatures,
but remain positional metadata: they create no body locals, do not change procedure type identity,
and do not alter call or return semantics. Duplicate labels are E0298. Jai's broader
unparenthesized/defaulted named-return forms remain explicit compatibility gaps.

`New(T)` now allocates one zero-initialized `T` through the active context allocator and returns
`*T`. It preserves a null allocator result, works inside same-file polymorphic procedures, and
supports representation-indirect recursive shapes such as a `Node` containing `[..]*Node`.
Release remains explicit through the matching context allocator. The allocator protocol carries
only a byte count, so `New` refuses types requiring alignment above its 16-byte guarantee.

Polymorphic calls now infer variables through parameterised nominal types:
`*Table($K, $V)` matched with `*Table(string, s64)` binds both variables from the struct instance's
type arguments. Matching uses the constructor's declaration identity, not its name or field layout,
and the constructor may itself be imported. A pure `$T` procedure declared in another module is
now specialised in that declaration file through one root-program fixed point, including nested
owner-to-owner demands; run, build, optimisation and diagnostics consume the same clone plan.
Imported `$N`/mixed templates remain E0268, and imported polymorphic `#expand` remains E0272.

Local compile-time value templates now use the same declaration-ordered argument binder as ordinary
calls. A literal may default `$N` itself—`width :: ($N: s64 = 4)`—and named arguments may reorder
pure `$N` or mixed `$T`+`$N` calls. Supplied comptime expressions are evaluated; omitted defaults are
already values, and equivalent explicit/omitted spellings share one specialization. Defaults still
cannot infer `$T`, and imported `$N`/mixed templates remain deferred.

Native dynamic arrays now have one generic operation set in `List`. A caller can keep
`stack: [..]*Node`, append pointers, inspect or pop the top, index and mutate elements, hand out the
used prefix as a view, clear while retaining capacity, and explicitly release storage. The same
procedures specialise for existing `[..]s64` callers. Empty reads return `(zero, false)`, and a
default-initialised pointer local is now correctly the typed null value rather than undefined. In
the language server, completing `stack.` offers the same public `data`, `count`, and `capacity`
pseudo-fields that sema accepts, with `data` preserving the concrete element type.

The language surface now treats that native dynamic array as a bounded sequence too. `stack[i]`
reads or writes the used prefix directly, `*stack[i]` takes an element address, and
`for node, index: stack` iterates from zero to the captured `count`. Bounds checks use `count`, not
spare `capacity`; `#no_abc` keeps its existing opt-out, and `*[..]T` auto-dereferences to this
bounded meaning before raw-pointer indexing is considered.

`Hash_Table` now supplies the other requested generic container. `Table(K, V)` is zero-ready,
including as `properties: Table(string, string)` inside `New(Node)`. Strings compare by content;
integer, enum, bool and pointer keys have built-in value policy; arbitrary keys install borrowed
hash/equality callbacks. `table_add` and pointer-returning `table_set` are safe upserts, lookup keeps
Jairs' `(value, found)` convention, explicit cursors reject structural mutation, and allocation is
transactional through the allocator captured by the first backing allocation. Keys and values are
shallow copies, iteration order is unspecified, and `deinit` is explicit and idempotent.

Structural data interfaces now constrain existing specialisation without adding a runtime interface
object: `value: $T/interface Shape` requires `T` to expose every field directly declared by `Shape`.
Field declaration order is irrelevant, extra fields are allowed, and a unique nearest field promoted
through `using` satisfies the requirement; a missing, differently typed or ambiguous field is E0299.
The same spelling composes under pointers as `*$T/interface Shape`. Template bodies see only the
declared shape, while each accepted call is rechecked and lowered with the candidate's concrete field
indices and layout.

Compile-time calls now use the same argument binding as ordinary calls. A local or imported
procedure called by `#run` may omit literal defaults or reorder named arguments at file scope or
inside a body; sema decides the positional list once and both MIR paths consume it.

A literal-defaulted parameter may now omit its annotation: `amount := 9`. The declaration fixes its
type once — integer and `#char` literals become `s64`, floats `float64`, booleans `bool`, and strings
`string` — and ordinary local/imported calls plus `#run` use the same named/default binder. `null`
still needs an explicit pointer type and non-literal defaults remain deferred.

Ordinary local/imported calls to a pure `$T` procedure now use that binder too. A fixed explicitly
typed or inferred literal default may be omitted, names are reordered before inference, and only
caller-supplied expressions bind each type variable. A default whose type is `$T` or a later bare
`T` is E0252. Local `$N` and mixed templates now accept named arguments and literal defaults,
including a default on `$N` itself; imported comptime templates remain E0268. Calling a pure
template inside `#run` is still a separate specialization gap.

Every fresh Jairs context now has a working allocator/free pair in the VM, Cranelift and LLVM;
assigning a custom pair and copying one through `push_context` remain unchanged. Whole-file
read/write/append operations live in `File`, and every successful read — including an empty file —
returns owned storage that may be released with `String.free_string`.

`Basic.String_Builder` now grows through a captured allocator, appends strings, bytes and formatted
values, and converts to independently owned text. Formatted builder output shares `print`'s renderer,
supports `%1`/`%2` argument selection, and is not capped by `format`'s 4096-byte staging buffer.
The compatibility source used to plan it is pinned locally under `references/The_Way_to_Jai`; that
submodule is research-only and normal builds do not need it.

The same pinned guide now backs a public non-overload `String` layer. `slice` is a strict borrowed
view, `copy_string` is an owned allocator-backed copy, the demonstrated compatibility names delegate
to existing algorithms, and the two-result integer/float conversions coexist with remainder-aware
parsers. `parse_int(*string)` consumes the parsed prefix. Exact byte/string overload families still
wait for general procedure overloading rather than being approximated with more ad-hoc names.

Compatibility claims now have an executable baseline rather than only a prose matrix. One strict
manifest covers all **35 example-bearing guide groups** with **37** repository-owned probes,
including second probes for chapter 11's `New(int)` and chapter 17's result labels. Six selected examples are source-compatible at the pinned guide
revision; the others pin a working port, an exact blocker, or an intentional divergence. Ordinary
tests never read the submodule. The guide has 42 numbered groups and 315 examples, so the baseline
deliberately makes no percentage claim; the full assessment and dependency-ordered plan live in
[`docs/research/way-to-jai-compatibility.md`](docs/research/way-to-jai-compatibility.md).

Formatter projects can now choose `case_block_style = "same_line"` to render an arm whose sole
statement is a block as `case .TEXT; {`, with comment-free empty blocks rendered as
`case .TAG; {}`. The default remains `"next_line"`. Newly scaffolded manifests explicitly choose
two-space indentation, and direct formatter users, manifest-free files, examples, modules and the
canonical corpus now use the same default (ADR-0220–ADR-0222). Multiline struct literals gain a
final comma by default; `[fmt] struct_literal_trailing_comma = false` removes only that final comma
without changing compact or empty literals (ADR-0239).

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

- **1436** workspace tests (**1449** under gate 7), with all seven gates green for ADR-0242.
- **325** `.jr` corpus files outside the fixture-module directory, **242** accepted ADRs, **26** standard library
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
  if ptr.* == 9  print("%\n", ptr.*);
}
```

More in **[`examples/`](examples/)** — eleven small, verified programs plus
three games. They cover structs, polymorphism, `#run`, the target-OS query,
arrays, file I/O, formatted output, language utilities and build scripts.

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
- **[`docs/adr/README.md`](docs/adr/README.md)** — all 242 accepted decision
  records.
- **[`docs/spec/`](docs/spec/)** — the language specification chapters.
- **[`examples/`](examples/)** — runnable programs, each verified.

## Licence

**Public domain**, under [The Unlicense](UNLICENSE) — do anything you like with
this, with no conditions and no attribution required. `Cargo.toml` declares
`license = "Unlicense"`, which is the SPDX identifier.
