# ADR-0015: Type identity — nominal structs, distinct `string`, interned `void`

- **Status:** Accepted
- **Date:** 2026-07-26
- **Deciders:** dboll

## Context

The Jairs-0 slice builds the InternPool (`jr-pool`), and the pool's key design
*is* the answer to "when are two types the same type?". Nothing has answered it
yet. There is no type-system chapter — `docs/spec/README.md` says the type system
is written "as their waves land" — and no ADR settled it. It has to be settled
now, because retrofitting type identity means re-keying the pool, and re-keying
the pool is a rewrite rather than a change.

ADR-0005 looks like it already decided this, and it did not. That ADR says
identity is *structural*, but it is scoped to **polymorph instantiation keys** —
the tuple of resolved comptime-argument IDs — not to type equality. Nominal
struct identity is fully compatible with it: a nominal struct type still interns
to exactly one ID, so two mentions of `Point` still key equally. ADR-0005 does
not force structural struct types.

Three questions are answered together, because the pool key has to encode all
three at once: what makes two struct types equal, whether `string` is the struct
whose layout ADR-0004 fixes, and how "returns nothing" is represented.

## Decision

### 1. Struct type identity is nominal

Two separately-declared structs with identical field lists are **different
types**. The pool keys a struct type on its *declaration site* — a stable
declaration identity, the file plus the declaration's index within it — not on
its field list.

```jr
Point :: struct { x: s64; y: s64; }
Vec2  :: struct { x: s64; y: s64; }
```

`Point` and `Vec2` are distinct types and are not interchangeable, in either
direction. Both are ordinary constants (ADR-0012), and their type names are
ordinary identifiers resolved by name, not keywords
(`docs/spec/01-lexical.md:111`).

### 2. `string` is a distinct builtin type, not an alias for a struct

`string` is a builtin type with the layout `{data: *u8, count: s64}`. ADR-0004
fixes the *layout and ABI*; it does not make `string` a re-declarable struct. A
user-written `struct { data: *u8; count: s64; }` is therefore **not** `string`,
and never coerces to it. `.data` and `.count` remain directly accessible on a
`string` exactly as ADR-0004 requires (`021-string-literals.jr`).

ADR-0004's `{data: *u8, count: s64}` notation is not legal Jairs syntax at all —
struct fields are semicolon-terminated (`docs/spec/02-declarations.md`) — which
is the clearest evidence that the notation was always a layout sketch and never
source.

### 3. "Returns nothing" is an interned `void` type, not an absence

A procedure that returns nothing has return type `void`: a zero-size type with
its own pool key. A procedure type is therefore always `(params) -> ret` with a
**total** return field, even though the surface syntax omits the arrow
(`docs/spec/02-declarations.md:155`).

This coins a term the spec does not use: `void` appears nowhere in the docs
today, and nothing about this decision changes what users write. It is an
internal representation decision, not a new spelling.

### 4. The rest of the identity story, already fixed elsewhere

Recorded here because it belongs in one place:

- A **procedure type's** identity is its parameter types, its return type, its
  context flag (ADR-0001, which already makes the flag "part of their
  identity"), and its currently-inert effect-row slot (ADR-0008).
- **Pointer types are structural.** `*T` is identical to `*T` for the same `T`,
  and it nests: `**T` is the pointer-to type applied twice
  (`docs/spec/02-declarations.md`, `015-pointers.jr`).

## Consequences

### Positive

- The class of bug where a structurally-coincidental type is silently accepted —
  passing a `Vec2` where a `Point` is wanted — becomes an error rather than a
  coincidence that happens to work.
- `string` keeps the ability to acquire distinct behaviour later (iteration,
  UTF-8 operations, distinct formatting) without that behaviour leaking onto
  every user struct of the same shape.
- The model matches Jai, Zig, Odin, and C, so it is unsurprising to exactly the
  audience Jairs is for.
- A total return field keeps the proc-type key uniform, and keeps later MIR and
  codegen uniform: there is no "no return type" branch anywhere.
- Multiple returns (W2) extend the return slot rather than reworking it.

### Negative

- **The pool must carry a stable declaration identity for every nominal type,
  which means declaration identity has to survive incremental edits.** This is
  the real cost. Under salsa (ADR-0007) a file-plus-index identity moves when a
  declaration is inserted above it, and ADR-0013 deliberately deferred the
  `AstIdMap` that would make node identity stable under unrelated edits. An
  unstable declaration id does not fail loudly — it silently splits one type into
  two, or merges two into one. The slice tolerates this because it re-analyses
  whole files anyway, but the tension is real and is the most likely reason this
  ADR gets revisited.
- Anonymous and inline struct types (`p: struct { … }`) need a declaration
  identity even though they have no name, so declaration identity cannot be
  derived from the bound name.

### Follow-on work this forces

- **Into the slice:** `jr-pool` keys nominal types on declaration identity,
  interns `void`, and gives `string` its own key. A user struct matching the
  string layout is a different type, and the pool must make that fall out of the
  key rather than out of a check.
- **Into wave W1:** the numeric tower and `cast()`/`xx` land in a world where
  identity is already nominal, so conversions are explicit by construction
  instead of being retrofitted onto accidental structural compatibility.
- **Into wave W4:** first-class `Type` values intern as ordinary comptime values.
  Because a nominal type has exactly one ID, equal type-values still key equally,
  which satisfies ADR-0005 without weakening it.
- A future spec chapter on the type system must document all of this.
  Assignability and coercion rules remain unspecified and are explicitly out of
  scope here: this ADR fixes *equality*, not conversion.

## Alternatives considered

**Structural struct identity.** Rejected: it collapses distinct domain types, so
a `Vec2` is silently accepted wherever a `Point` is wanted, and every pair of
same-shaped types in a program becomes an unintended alias. Worse, it makes
`string` indistinguishable from any user struct of the same shape, which directly
undermines ADR-0004 by removing the compiler's ability to treat the string type
specially. Zig shipped anonymous/structural struct types and then removed them;
that is cited prior art that this direction is painful to walk back.

**`string` as an alias for the struct type.** Rejected: it is only coherent under
structural identity, which is rejected above, and it forecloses ever giving
`string` behaviour that a plain two-field struct does not have.

**"Returns nothing" as an absence.** The HIR already does this —
`Proc::ret` is an `Option<TypeRefId>` (`crates/jr-hir/src/hir.rs:451`) — and
mirroring it in the pool was the obvious move. Rejected: it forces every
consumer, from the type checker through MIR to codegen, to handle the absent case
at every use, and W2's multiple return values want a return *slot* regardless.
The absence buys nothing and is paid for everywhere.
