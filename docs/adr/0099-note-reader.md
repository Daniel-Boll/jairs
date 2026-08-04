# ADR-0099: `has_note` and `note_value` read a declaration's notes at compile time

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W6 sub-wave 2.** ADR-0098 shipped `@note` **without a reader**, and said the next sub-wave would be the
  mechanism that lets a metaprogram ask for them. This is that mechanism, in its smallest honest form: two
  intrinsics over a named declaration.

## Context

ADR-0098's closing claim was that notes ship before their reader deliberately, because the message loop's
message shape should be designed against data that already exists. That data now exists, so the question is
what the reader *is*.

**The full Jai answer is a message loop** — `compiler_wait_for_message()` handing a metaprogram each
declaration as the compiler sees it. That needs three things this project has not decided: a `Declaration`
value (which is ADR-0080 §3's declined `Code` value in a new costume), a compile-time iteration protocol, and
a re-entrancy story for a metaprogram running while the compiler is mid-check. Each is its own sub-wave.

**But a loop's message is only useful if something can inspect it**, and inspecting a declaration's notes is
the primitive that inspection consists of. So the primitive comes first, and the loop later delivers
declarations to it rather than inventing its own reading verbs.

## Decision

### 1. Two intrinsics, taking the **declaration itself** rather than its name as text

```jai
inlined :: () #expand { … } @inline @requires "s64"

main :: () {
    if has_note(inlined, "inline")   { … }   // -> bool
    which := note_value(inlined, "requires"); // -> string, "" when absent or bare
}
```

`has_note(decl, "name") -> bool` and `note_value(decl, "name") -> string`. They join `type_info`, `any_of`
and `any_as` in the same `Intrinsic` enum and the same "callee resolves to no declaration" gate, so a program
that declares its own `has_note` shadows the intrinsic and nothing is stolen from the namespace.

**The first argument is the declaration, not a string.** `has_note(add, "inline")` misspelt as
`has_note(addd, …)` is an ordinary unresolved-name error; had the declaration been named by text, it would
have been a silent `false`. A silent `false` is precisely the failure the formatter's dropped notes had
(ADR-0098's consequences), and rebuilding it in the reader would be careless.

**The note name is a string literal**, and a non-literal is refused (E0277) — the same narrowing an array
length took (ADR-0039 §3a) and an `#insert` operand took first (ADR-0072), with the same widening route
available later. A folding intrinsic needs the name at check time and const-eval runs after.

### 2. Both fold in **sema**, with no VM and no new query

`type_info` folds in `jr-db` because it needs `layout_of` and a mutable pool at a point where the described
type is known. A note needs neither: the answer is in `FileHir`'s `Proc::notes`, which sema is already
holding, and `Ctx::pool` is already `&mut`. So `check_has_note` interns the answer directly and records it in
`CheckOutput::folded_calls` — one map, `(ExprScope, ExprId) -> PoolId`, which `file_consts` copies into
`ConstValues` through the existing `set_run` channel.

**Reusing `set_run` rather than adding a channel**, for ADR-0075 §2's reason: `jr-mir` already replaces a
`run`-keyed call with its constant and never emits the callee, so a second mechanism would be a second thing
to keep in step. The call's *callee* is typed `void` and the fold makes it unreachable, exactly as
`type_info`'s is.

`folded_calls` is deliberately **separate from `type_info_calls`**, whose meaning is "build a `Type_Info` for
this type" rather than "here is the value". ADR-0076 §2 records what conflating those two cost: a 40-byte
`Type_Info` stored into a 16-byte `Any`, caught only because the sizes differed.

### 3. A missing note answers `false` and `""` — it is not an error

`has_note(f, "nonexistent")` is `false`, and `note_value` of an absent or payload-less note is `""`. Asking
whether a note is present is the *point*; refusing the question because the answer is no would make the
predicate unusable for the thing it exists for. This is the opposite call from `any_as`, which traps on a
mismatch (ADR-0076 §2) — and the difference is that `any_as` returns a value that would otherwise be garbage,
while `has_note` returns the truthful answer "no".

`note_value` returning `""` for both "no such note" and "a bare `@name`" is a deliberate conflation: a
build script asking for a payload wants the payload or nothing, and distinguishing the two would need an
optional return no caller in this wave has a use for.

### 4. `==` on an aggregate is refused (E0278) — a separable fix this sub-wave found by probing

Writing this wave's corpus file wanted `note_value(f, "since") == "0.3"`, and that **leaked an internal
compiler error**: `expected a scalar, found an aggregate`, from the VM's value decoder, for a program any
reader would expect to compile. A `string` is `{data: *u8, count: s64}` (ADR-0004), so the two available
meanings are exactly a *view*'s — same storage, or same contents — and ADR-0044 §5 already refused a view's
`==` on precisely that ground. This is that refusal one type wider, and it covers every aggregate.

The predicate is **structural rather than layout-based**: `Layout` records only size and alignment, so an
`s64` and a two-field struct of `s32`s are indistinguishable by it and only one of them is comparable. The
match over `Item` is exhaustive, so a new aggregate kind is a compile error here rather than a silent
fall-through to a comparison the VM cannot make.

Comparing *contents* needs a byte loop, which is `String`'s job in W7 rather than an operator this wave
invents — an `==` that looped would be the only implicitly-looping operator in the language. The corpus file
compares `.count` instead, which is what the refusal's help suggests.

This is the **fourth leaked internal error** this project has turned into a sentence a reader can act on, and
the third found by *probing a new feature's own corpus file* rather than by reading code.

## Consequences

- **A metaprogram can finally act on a note.** `valid/080` branches on `has_note` and exits with a value that
  differs depending on a note's payload, so the notes are load-bearing in a way the differential harness
  checks: delete the note and the program exits differently.
- **Two new diagnostic codes.** **E0277** covers both of the reader's refusals — a note name that is not a
  string literal, and a first argument that is not a procedure — because they are one intrinsic's two ways of
  being unaskable, and a reader who hits either needs the same page. **E0278** is `==` on an aggregate (§4).
  **E0279 is the first free code.**
- **A folded call has no *place***, so `note_value(f, "x").count` must bind to a local first. That is an
  ordinary consequence of folding, shared with `type_info(T).size` (which `valid/076` binds for the same
  reason), not a gap in the reader — but it took an honest gap report to notice, and the corpus file says so.
- **The refusal reports once.** Asking about a type marks the argument a type position first, so E0261's "a
  type is a compile-time value" does not pile on: two diagnostics for one mistake, the second about a rule
  this position does not have. The allowlist gains an entry rather than E0261 gaining an exception, which is
  ADR-0071 §3's asymmetry argument.
- **Teeth-checked.** Inverting the `has_note` answer moves `valid/080`'s exit from 127 to 56, so the fold is
  load-bearing and both engines read it. A note-reader that always answered `false` would otherwise be
  invisible — the corpus differential compares the two engines to each other, so a shared wrong answer
  passes.
- **Only a procedure can be asked**, because only `Proc` carries notes: the parser takes them in the
  *procedure* attribute loop. Asking about a struct is refused by the ordinary path (it resolves to a
  non-procedure) rather than answering `false`, so widening later is additive.
- **What this still is not** is the message loop: a script must *name* each declaration, so it cannot ask
  "every declaration tagged `@X`". That query is what the loop adds, and it now has a reading verb to hand
  its declarations to rather than needing to invent one.
