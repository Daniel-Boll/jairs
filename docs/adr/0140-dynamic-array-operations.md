# ADR-0140: `[..]T` operations — converting `modules/List` to the native type

- **Status:** Accepted
- **Date:** 2026-08-18
- **Deciders:** dboll
- **Follow-up to ADR-0136.** ADR-0136 lifted `[..]T` to native syntax with a compiler-known
  `{data, count, capacity}` layout, and deferred two things to "the wave that also converts
  `modules/List`": a library `push` on `[..]T`, and the `Type_Info_Kind.DYNAMIC_ARRAY` member.
  This is that wave — the first of PLAN §7's owed follow-ups, taken because it was named as the
  easiest.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are argued at their point of decision below, per the session directive.

## Context

### What ADR-0136 shipped, and what it left

ADR-0136 gave `[..]T` a native type: `xs: [..]s64` declares a zero-initialised dynamic array with
`.data`/`.count`/`.capacity` as readable, writable places. It deliberately shipped **the type, not
the operations** (§3): the growth policy belongs in a library so a caller can substitute it.

The library that had the operations was `modules/List`, which pre-dated the syntax. It maintained a
hand-rolled `List :: struct($T)` with fields `data`, `count`, `capacity` — the *identical* layout
the compiler later took as native — and wrote `push`/`pop`/`get`/`set`/`clear`/`free_data`/
`is_empty`/`elements`/`grow` on top of it, all concrete `*List(s64)`.

So the state before this wave was a native type with no operations and a library type with the same
layout and all the operations, side by side. ADR-0136 §4 named the resolution: convert `modules/List`
to operate on `[..]s64`, or write a new module.

## Decision

### 1. Convert `modules/List` in place — the struct is deleted, the routines take `*[..]s64`

`List :: struct($T)` is removed. The nine public routines and the private `grow` now take
`*[..]s64`; nothing else about them changes, because the native type's three place pseudo-fields
answer `.data`/`.count`/`.capacity` exactly as the struct's fields did. A caller declares
`xs: [..]s64` and calls `push(*xs, v)`; the type comes from the language, the growth policy (doubling
from `FIRST_CAPACITY = 4`) from this module.

**Rejected: keep both — the struct *and* a new module of native-type operations.** They had the
identical layout, so this is one behavioural set maintained in two places, and the native type exists
precisely so the hand-rolled one need not. The differential harness would have to cover both, and a
reader learning "what is a growable list in Jairs" would find two answers.

**Rejected: keep `List(s64)` as a compatibility name.** It is not expressible. A user-written struct
of the same shape is a *different* interned type that never coerces to `[..]s64` (ADR-0136 §1, the
same reason `string` is not `struct { data; count; }`), and Jairs has no type alias — so "keep the
name" would mean keeping the whole struct and its routines, which is the rejected duplication under
another word. The two consumers in the corpus (`valid/088`, `valid/089`) are updated to declare
`[..]s64` instead; there are no others (`modules/Map` mentions `List` only in prose).

### 2. The routines stay concrete `s64` — the native type does not lift the template bound

The native `[..]T` **does** cross a module boundary cleanly: it is structural, not a `struct($T)`, so
the E0269 that refused a parameterised struct in a module (ADR-0085 §5) does not apply to it. What
still binds is the *procedures* — an imported polymorphic procedure is refused (E0268, ADR-0104 §5) —
so `push :: (a: *[..]$T, v: $T)` would be uncallable by every importer. Concrete `s64` is therefore
what an importer can use, exactly as when the type was hand-rolled. The conversion buys the native
*syntax* and one shared *layout*; it does not buy generic library routines. When imported templates
arrive, these become `$T`.

### 3. `Type_Info_Kind.DYNAMIC_ARRAY` is added to `modules/Basic`, appended not inserted

The compiler already emitted the member *by name* for a `[..]T` (ADR-0136's `type_info_kind_name`),
and `jr-db`'s const-eval looks it up in the `Type_Info_Kind` enum declared in `Basic`. Before this
wave the member did not exist, so `type_info` of a dynamic array was a **hard const-eval error** —
`Type_Info_Kind has no member DYNAMIC_ARRAY, which the compiler expects`. Adding the member closes
that gap and makes a dynamic array discriminable from a `VIEW`, which it must be: a `[..]T` owns its
storage and carries a capacity a view has not got.

**Appended after `PROCEDURE`, not placed beside `VIEW`.** The lookup is by name, so *correctness* is
insensitive to position — but a member's *value* is its declaration order, and a snapshot or a caller
comparing raw ordinals would see every later shape (`STRUCT`…`PROCEDURE`) renumber if it were inserted
mid-list. Appending moves no existing value.

The path to the member from source is a `$T` procedure bound to a dynamic array — `type_info`'s
argument is an *expression* and `[..]s64` is a type, not one, so `type_info([..]s64)` does not parse.
`valid/113` reflects through `kind_of :: (x: $T) -> Type_Info_Kind`, which is the ordinary way a
program reaches the `type_info` of a view or an array too.

## Consequences

- **1010 workspace tests unchanged; 226 → 227 corpus files.** `valid/113` exercises the converted
  operations end to end (push past capacity, growth, `get`/`pop`/`set`, `elements` into `Sort`) and
  the `DYNAMIC_ARRAY` reflection distinguished from `VIEW`. `valid/088` and `valid/089` now declare
  the native `[..]s64` and pass with their prior exit codes (255 and 63) unchanged — the conversion
  is behaviour-preserving, which is the point of it being a conversion rather than a rewrite.
- **No compiler change.** Like ADR-0105's `Array`, this wave found the language already provided
  exactly what a library needed: the native type's pointer pseudo-fields, pointer arithmetic,
  `malloc`/`free`/`typed`/`untyped`/`size_of`/`view` all compose through a `*[..]s64` parameter. The
  only non-corpus edit is one enum member in `modules/Basic`.
- **`modules/List` is now an operations module over a language type**, not a type-plus-operations
  module. That is the shape ADR-0136 §3 described ("a library — initially `modules/List`, later
  converted — writes `push`, `pop`…").
- **Deferred, unchanged by this wave**: a `for xs { it }` loop shape for a `[..]T` (ADR-0136's other
  deferral — a dynamic array still iterates as `for xs.data[0 .. xs.count]`); generic library routines
  over `[..]$T`, which wait on imported templates (E0268); `type_info`'s reach into a dynamic array's
  *element* type (a `Type_Info_Kind.DYNAMIC_ARRAY` payload, still owed with the rest of the
  variable-length field list, ADR-0078).
