# ADR-0118: The containers become generic structs with concrete procedures — half a conversion, on purpose

- **Status:** Accepted
- **Date:** 2026-08-06
- **Deciders:** dboll
- **W7 sub-wave 16.** ADR-0117 let a parameterised struct cross a module boundary. This collects on it in the
  three modules that asked for it — as far as the language allows, which is not all the way.

## Context

ADR-0117 lifted E0269, so a `struct($T)` in a module works for an importer. But **inference through a
parameterised struct is still deferred** (ADR-0085 §5): `push :: (a: *Array($T), v: T)` is E0212, because `T` is
not in scope in a parameter list. So the *struct* can be generic while the *procedures* cannot.

Probed before converting: a module declaring `Holder($T)` and exporting `push_int :: (a: *Holder(s64), v: s64)`
works from an importer — generic struct, concrete procedure.

## Decision

### 1. `Array($T)` and `List($T)`, with procedures still taking a concrete instance

`Int_Array` becomes `Array($T)` with `items: [16]T`; `Int_List` becomes `List($T)` with `data: *T`. Every
procedure takes `*Array(s64)` / `*List(s64)`.

**The storage declaration is now written once** instead of per element type: a second element type needs a set of
procedures but not a new struct. That is real progress, and it leaves the modules in the shape the eventual
inference lift *completes* rather than one it has to undo.

**The honest reading is "half converted"**, and the module docs say which half and why — a reader who sees
`Array($T)` and then `push(a: *Array(s64))` should find the reason in the file rather than infer an oversight.

### 2. `Map` stays concrete, because `size_of` cannot take an instance

`Map($K, $V)` needs `size_of(Slot(K, V))` and `typed(Slot(K, V), raw)` to allocate its slot array — and **an
intrinsic's type argument is not parsed in type position**, so `Slot(s64, s64)` inside one gives `unresolved name
s64`. `Map`'s conversion is therefore blocked by something neither ADR-0117 nor ADR-0085 §5 is about: the
intrinsics' argument grammar.

Reverted rather than worked around. The alternative was hand-computing the slot size as an integer literal, which
is exactly the "silent wrong read" ADR-0105 §3 refused for the same reason — and in the standard library, where a
reader learns what the language means.

**That is a fourth named unblocker**, and a small one: `size_of`, `typed` and `view` should accept a
`TypeRef::Apply` argument, which is a parser change in the intrinsics' argument position. Recorded so the next
sub-wave that wants it knows it is small.

### 3. Scoped to one module first, deliberately

`Array` was converted alone before the others, because its storage is *inline* — a mistake shows immediately —
while `List` and `Map` own heap memory and their `grow` paths are where a conversion could go subtly wrong. Only
after `Array` was green did `List` follow, and `Map`'s blocker surfaced in the attempt rather than in a plan.

### 4. Two more unused-import traps closed

**A `#foreign` library name imported from a module did not mark the import used.** `Math` imports `Basic` for
`libc`, and a library is named in a *declaration attribute* rather than an expression — so `ResolveMap`, which
covers `Expr::Name`, never saw it, and E0231 called the import unused. The quick fix beside that warning would
have broken every libm wrap in the module (ADR-0031 §2). `lookup_value_name` records it now.

That is the **third** place this trap has had to be closed — after an ordinary imported type annotation and a
type-argument reference (ADR-0117 §5) — and the shape is always the same: a name reached through a path that is
not an expression. Any future non-expression name lookup should assume it needs the same line.

**And `String`'s import was genuinely unused**, which the warning was right about: its allocator comes from
`context`, a *language* facility (ADR-0057) rather than a library name, so the module imports nothing at all.
Removed, with a note saying why a module that allocates needs no import.

## Consequences

- **The language's new capability is used where it was asked for.** Two of the three modules that named E0269 now
  declare their storage once. `valid/086`, `088` and `089` all still pass unchanged in behaviour — and **the MIR
  snapshot did not move**, which is the right outcome: `Array(s64)` lays out exactly as `Int_Array` did, so a
  moved snapshot would have meant the conversion changed something it should not.
- **No new diagnostic code, and no compiler change** beyond the two unused-import records.
- **What remains, now four named unblockers**: inference through a parameterised struct (`*Array($T)`), which
  would make the procedures generic too; an intrinsic accepting a parameterised type argument, which unblocks
  `Map`; cross-file `$T` procedure instantiation (E0268); and `using` on an imported struct. Each is small and
  named, which is a better position than one large deferral.
