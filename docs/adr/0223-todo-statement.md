# ADR-0223: `todo;` is source-located terminal control flow

- **Status:** Accepted
- **Date:** 2026-09-09
- **Deciders:** dboll
- **Amends:** ADR-0017 §MIR terminators and ADR-0020 §trap source locations.

## Context

Jairs has several ways for execution to fail, but no source construct that means “this path is
intentionally unfinished”. Library authors therefore use placeholder returns, an `exit` call, or leave a
body unable to lower. Those choices either invent a value, lose the ordinary trap backtrace, or describe a
compiler gap rather than a program decision.

Rust's `todo!()` is useful because it lets an unfinished path remain explicit and terminal. Jairs does not
have bang macros or a never type, so copying that spelling would introduce two unrelated concepts merely to
gain one control-flow operation.

MIR already ends a block with `Terminator::Unreachable(Unreachable)`. The reason distinguishes a deliberate
trap, unchecked stray jump, missing return and refused body, but the terminator carries no span. Every VM
and native-backend reader therefore assigns `MirSpan::Synthetic` to an unreachable terminator. Reusing that
shape for a source `todo` would produce the trap message ADR-0020 requires while omitting the source
location ADR-0020 exists to preserve.

The decider chose:

- a reserved statement `todo;`, rather than an intrinsic call or compiler directive;
- a distinct `todo` trap reason and source location;
- terminal control flow that satisfies return analysis;
- ordinary trap behavior: no `defer` execution and failure only when the path is reached;
- no custom message in this first form.

## Decision

### 1. `todo;` is a reserved, statement-only language construct

`todo;` is legal wherever a statement is legal, including a braceless control body. It reserves `todo` as
a keyword and has its own CST and HIR statement variants.

It is not an expression and has no type. A future expression-polymorphic form must introduce a genuine
never/bottom type rather than reusing `ERROR`, an absent type-map entry, or another recovery sentinel.

Rejected spellings:

- **`todo()` as an unresolved-name intrinsic.** Existing intrinsics may be shadowed and look like ordinary
  value-producing calls. Without a never type this spelling promises more expression behavior than the
  implementation can provide.
- **`#todo`.** Directives instruct the compiler or metaprogramming system; an ordinary runtime control-flow
  terminator is not one.
- **`todo!()`.** Jairs has no bang-macro invocation model, and adding one for a single fixed operation would
  make syntax imply an expansion mechanism that does not exist.

### 2. Reaching `todo;` terminates the current path with its own trap

MIR gains `Unreachable::Todo`. The VM and both native backends map it to a distinct static reason,
`reached todo`, rendered through the same `jr_base::trap_message` path as every other language trap.

The exit status remains 4. Runtime output includes the `todo;` source location and the ordinary live
backtrace. Under `#run` or constant evaluation, an executed `todo;` becomes the existing E0230
compile-time-evaluation failure with `reached todo` as its reason. An untaken branch containing `todo;`
remains valid.

### 3. Every MIR unreachable terminator carries a span

`Terminator::Unreachable` becomes a struct variant containing both its `reason` and `span`.

Source `todo;` stores the statement's `MirSpan`. Existing compiler-created unreachable paths store
`MirSpan::Synthetic` unless they already have an honest source span. This makes source attribution a
property of the terminator rather than a backend-side guess and gives later source-level terminal
constructs one place to put the same fact.

Rejected alternatives:

- **Put a span only inside `Unreachable::Todo`.** Smaller today, but it makes location a special property of
  one reason when the terminator is the event being located.
- **Reuse `Unreachable::Trap`.** It would print `reached a deliberate trap`, conflate `todo` with
  compiler-generated trap paths such as a failed `any_as`, and remain unlocated.
- **Have backends recover the span from nearby instructions.** A terminated block may contain no
  instruction, and three independent guesses can disagree.

### 4. `todo;` aborts; it is not structured cleanup

MIR ends the current block immediately and does not emit pending `defer` statements. That matches overflow,
bounds, assertion and other traps. Running defers would make `todo` a structured exit with behavior unlike
the failure it represents.

Because the path terminates, a valued procedure may end in `todo;` without a dead `return`. Statements
after it are unreachable and do not lower into the live CFG.

### 5. Tooling treats `todo` as an ordinary keyword statement

The formatter must preserve the complete statement and remain idempotent. Tree-sitter parses it as a leaf
statement and both editors highlight `todo` as a keyword. LSP completion offers it in the keyword set, while
hover, navigation, references and rename do not treat it as a name.

## Consequences

- An unfinished implementation is explicit, searchable and loud if reached.
- Runtime and compile-time execution share one meaning and one reason.
- The MIR terminator representation becomes more honest: source attribution travels with the terminal
  event rather than being reconstructed by each engine.
- The first form is intentionally statement-only. Expression use and custom messages remain future
  decisions, not silently partial syntax.
- The formatter, compiler parser, tree-sitter grammar, HIR, sema, MIR, VM, Cranelift, LLVM and LSP all
  participate in the wave.
- All six ordinary gates and gate 7 are required.
