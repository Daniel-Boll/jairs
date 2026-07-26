# ADR-0016: Jairs-0 typing rules — context-typed literals, deferred `#run`

- **Status:** Accepted
- **Date:** 2026-07-26
- **Deciders:** dboll

## Context

`jr-sema` is being built, and it needs typing rules that nothing has written
down. ADR-0015 settled type *identity* and said so explicitly: "Assignability and
coercion rules remain unspecified and are explicitly out of scope here: this ADR
fixes *equality*, not conversion." This ADR is the sequel that fixes some of
that. There is no type-system chapter to appeal to — `docs/spec/` stops at
chapter 03, and `docs/spec/README.md` says later chapters are written "as their
waves land".

The more important gap is that **no corpus file expects a type error.**
`tests/corpus/invalid/` is ten files of lexical, parse, and lowering errors
(`001-missing-semicolon.jr` through `010-missing-type-after-colon.jr`), and
`tests/corpus/imports/invalid/` is three files of *resolution* errors — module
not found, ambiguous imported name, unresolved name after import. Nothing
anywhere asserts a well-formed program that sema must reject.

So the corpus constrains sema only **negatively**: every file in `valid/`,
`imports/valid/`, `tests/corpus/modules/`, and `modules/Basic/` must type-check
**silently**. That obligation is contractual, not incidental —
`tests/corpus/README.md` requires `imports/valid/` to "check cleanly" and
`modules/` to "parse and check cleanly".

That negative obligation, and not a positive spec, is what forces most of the
rules below. Several of the things it forces were never deliberate decisions
until now, and `docs/spec/README.md` is what makes the forcing binding: "If the
spec and a corpus file disagree, the corpus file is right and the spec has a
bug." The corpus outranks the prose.

## Decision

### 1. Untyped integer literals take their type from context

An integer literal has no intrinsic type. It takes the type of the context it
appears in, and it is an error if the value does not fit that type.

```jr
a: s64 = 7;
g: u8  = 255;    // legal, no cast
count := 10;     // no context: defaults to s64
```

`g: u8 = 255;` in `tests/corpus/valid/005-decl-typed.jr` is therefore legal as
written. This rule is **forced, not chosen.** The obvious alternative — literals
are `s64`, and narrowing to `u8` needs an explicit conversion — makes that line
an error, and the conversion it would need does not exist: `cast()` and `xx` are
wave W1 (`docs/spec/00-overview.md`), and `modules/Basic/module.jr` documents at
length that `cast` being reserved is exactly why `print_int` cannot yet be
written. A rule that requires a cast in a slice that has no cast is not a rule,
it is a contradiction. Since the corpus decides, the corpus decides this.

This is the model Zig gives `comptime_int` and Jai gives untyped literals, and it
is deliberately **narrower** than implicit numeric conversion: it applies to
*literals*, not to values. ADR-0015's no-coercion stance for values is untouched.
`s64` arithmetic still does not silently mix with `u8` arithmetic; only the
literal token bends.

Note the interaction with the pool: `Item::IntValue { ty, bits }`
(`crates/jr-pool/src/item.rs`) makes an integer value's type part of its key, so
"no intrinsic type" cannot mean "interned untyped". Context typing has to happen
before the value reaches the pool, which is precisely where sema sits.

### 2. Binding the result of a void procedure is an error

Given `no_args :: () {}`, the statement `x := no_args();` is an error, not a
`void`-typed local.

This **requires editing `tests/corpus/valid/025-paren-constant.jr`**, which
currently contains exactly that line (`called := no_args();`). That edit is
legal because `valid/`'s contract in `tests/corpus/README.md` requires its files
to *parse* with zero errors and round-trip through `jr fmt` — it does not require
them to check. The file was asserting something we have now decided is wrong, and
it says so honestly rather than being grandfathered.

The rationale is that binding nothing is almost always a mistake, catching it is
one comparison, and the alternative makes every later phase — MIR, the mid-end,
bytecode lowering, Cranelift — carry void-typed locals for no benefit. `void`
remains a real interned type (ADR-0015 §3); this rule is about *binding*, not
about the type existing.

### 3. A `#system_library` constant has a distinct foreign-library handle type

`libc :: #system_library "c";` appears in `tests/corpus/valid/019-foreign.jr`,
`tests/corpus/valid/025-paren-constant.jr`, and `modules/Basic/module.jr`. It is
an ordinary constant (ADR-0012) whose value is a directive, and under rule 1's
sibling premise — sema types every constant — it needs a type.

It gets an opaque **foreign-library handle** type in the InternPool. The point is
not the handle; the point is that `#foreign libc "write"` can then check that its
library operand actually *is* a library, instead of the whole FFI boundary being
untyped. Today `ForeignInfo.library` is a bare `Option<Symbol>`
(`crates/jr-hir/src/hir.rs`) that name resolution does not resolve at all. This
rule is what makes resolving it worth doing: a resolved library operand with a
library type is checkable, a resolved library operand with no type is decoration.

That boundary matters more than its size suggests. ADR-0006 puts a libffi bridge
inside the comptime VM, so a mis-declared foreign binding can reach the host
machine during compilation.

### 4. `#run` is typed in the slice but not evaluated

`#run expr` has the type of `expr`. Sema does **not** fold it. `COMPUTED :: #run
add(2, 3);` (`tests/corpus/valid/020-run-directive.jr`,
`tests/corpus/valid/024-hello.jr`) therefore type-checks as `s64` and is usable
wherever an `s64` is wanted, while the actual value arrives when the VM does.

This is the rule with a real argument behind it. PLAN.md §3.1's load-bearing
invariant is that comptime and runtime execute *the same* MIR — "Any other
arrangement guarantees `#run` and runtime silently disagree." A temporary
tree-walking constant evaluator inside `jr-sema` would be precisely the second
evaluator that invariant forbids, and it would be deleted the week `jr-vm` lands.
PLAN.md's own pipeline diagram already draws const-eval as `SEMA <--> VM`: sema
asks the VM, sema does not evaluate. Folding waits for the VM rather than being
faked now.

The cost, stated plainly: for the whole slice, `#run` results have types and no
values. Anything that needs the *value* at compile time — not the type — cannot
work yet.

### 5. Cross-file typing goes through signatures only

Typing a call into an imported module needs that module's signatures. Computing
them is therefore a **separate step that depends only on the other file's HIR**,
never on the other file's full type-check. Full checking of a file may read any
other file's signatures; it must never trigger another file's full check.

This mirrors the invariant that already keeps module resolution acyclic under
ADR-0014 §4, spelled out in `crates/jr-db/src/module_loader.rs`: "`file_exports`
depends only on `file_hir`" — `resolved(A)` calls `file_exports(B)`, and
`file_exports(B)` calls `file_hir(B)`, so it never calls back into `resolved(A)`.
Signatures take the same shape one layer up, and that is what keeps
`tests/corpus/imports/valid/005-import-cycle-is-legal.jr` working, where
`Cycle_A` and `Cycle_B` import each other and typing genuinely crosses the cycle.
Without the split, typing that file does not terminate.

The consequence to accept: a procedure's signature must be typeable **from syntax
alone**, without checking its body. That holds in Jairs-0 because parameter and
return types are always written explicitly (`docs/spec/02-declarations.md`). It
would stop holding the moment return-type inference were added, which is the
condition under which this rule gets revisited.

## Consequences

### Positive

- The negative corpus obligation becomes satisfiable. Every file in `valid/`,
  `imports/valid/`, `tests/corpus/modules/`, and `modules/Basic/` has a rule that
  makes it check, rather than checking by luck.
- Literal context typing removes the need for casts inside the slice without
  opening general coercion, so W1's `cast()`/`xx` arrive as *additions* rather
  than as a relaxation of something already loose.
- The signature/check split preserves ADR-0014's cycle tolerance for free, and it
  is also what lets the LSP type one file without checking the world — which is
  the incrementality ADR-0007 bought salsa for.
- Deferring `#run` keeps the one-evaluator invariant intact, so `jr run` and
  `jr build` cannot disagree about a comptime value because there is only ever one
  thing computing it.
- Typing `#system_library` turns the FFI boundary from unchecked into checkable at
  the cost of one pool variant.

### Negative

- Literal context typing means the *same* literal token has different types in
  different contexts. Diagnostics must therefore report the **contextual** type
  and never a literal's "own" type, or they will actively mislead.
- Rule 1 needs a fit check per target type, and Jairs-0 has exactly one signed
  and one unsigned integer type. The check has to be written against
  signedness-and-width generally — `Item::IntType { signed, bits }` already
  is — rather than special-casing `s64` and `u8`, or W1's numeric tower
  rewrites it.
- **Rule 1 relocates an existing diagnostic.** E0204 is currently emitted in
  lowering (`crates/jr-hir/src/lower.rs`) against `i64::MAX`, worded "overflows
  `s64`", and `crates/jr-pool/src/pool.rs` documents the assumption that "a
  literal too large for its type is a lowering diagnostic (E0204) that has
  already been reported by the time a value reaches the pool". Lowering does not
  know the target type, so under this rule that check is in the wrong phase and
  its message names the wrong type. The fit check belongs in sema.
- Rule 2 requires editing a corpus file. That is worth flinching at: the corpus
  is the ground truth, and this is us overruling it.
- Rule 4 means `COMPUTED` has a type but no value for the entire slice. Nothing
  downstream may assume `#run` results are available, and the comments in
  `tests/corpus/valid/020-run-directive.jr` and `docs/spec/02-declarations.md`
  that describe the folded value describe the end state, not the slice.
- Rule 5 forbids signature computation from consulting bodies. That is a standing
  constraint on every future inference feature, not a slice-local simplification.

### Follow-on work this forces

- **Into the slice:** the five rules above land in `jr-sema`. `jr-pool` gains an
  `Item` variant for the foreign-library handle type of rule 3, and a signatures
  step lands for rule 5. `tests/corpus/valid/025-paren-constant.jr` loses its
  void binding, and E0204's fit check moves from lowering into sema against the
  contextual type.
- **Into wave W1:** the full numeric tower makes rule 1's fit check load-bearing
  rather than nearly-trivial, and `cast()`/`xx` define *explicit* conversion on
  top of it — on top of literal context typing, not instead of it.
- **Into the wave that lands `jr-vm`:** `#run` folding, via MIR, per PLAN.md
  §3.1. The VM is the only evaluator that will ever exist.
- A spec chapter on the type system must eventually document all of this.
  Assignability between *non-literal* values of different types remains
  unspecified, exactly as ADR-0015 left it.

## Alternatives considered

**Literals default to `s64`; narrowing requires `cast()`.** Rejected: it makes
`g: u8 = 255;` in `tests/corpus/valid/005-decl-typed.jr` an error, and the fix it
demands does not exist in the slice — `cast()` is W1. The corpus outranks the
spec, so a rule that breaks the corpus loses.

**General implicit numeric conversion between values.** Rejected: a far larger
blast radius, and directly against ADR-0015's no-coercion stance. Literal-only
context typing is the *minimum* that satisfies the corpus, and minimum is the
right size for a rule we cannot yet test negatively.

**Allow binding a void call, giving the local type `void`.** Rejected. It needs
no corpus edit, which is its only merit, and it pays for that by propagating
meaningless locals into MIR, the mid-end, and both backends forever.

**Exempt directive-valued constants from typing.** Rejected: it leaves the FFI
boundary unchecked and `ForeignInfo.library` permanently unresolved, which given
ADR-0006's comptime libffi bridge is the one boundary least worth leaving
untyped.

**A temporary constant evaluator in `jr-sema` for `#run`.** Rejected: it is the
second evaluator PLAN.md §3.1's same-MIR invariant exists to forbid, and it would
be discarded when `jr-vm` lands. Two evaluators that agree today are two
evaluators that disagree later.

**Let full checking of one file recurse into full checking of imported files.**
Rejected: it reintroduces exactly the salsa cycle ADR-0014 and
`crates/jr-db/src/module_loader.rs` were built to avoid, and
`tests/corpus/imports/valid/005-import-cycle-is-legal.jr` would not terminate.
