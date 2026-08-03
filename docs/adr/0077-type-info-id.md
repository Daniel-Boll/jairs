# ADR-0077: `Type_Info` gains a stable `id`, so a type has a runtime identity (amends ADR-0075 §3)

- **Status:** Accepted
- **Date:** 2026-08-03
- **Deciders:** dboll
- **W4 sub-wave 8, part of the `Any` work.** ADR-0076 §2 said `any_as(a, T)` reads an `Any` back "checked
  against its type" and "compares what the `Type_Info` *says* rather than where it lives". Building it found
  that a `Type_Info` says nothing a comparison can use — §0. This adds the one field that fixes it, and so
  **amends ADR-0075 §3's four-field schema** the way ADR-0018 §5 amends ADR-0017: a new ADR, not an edit.

## Context

### 0. What running found

`any_as` must answer "does this `Any` hold a `T`?" at run time. ADR-0076 §2 said to compare by what the
`Type_Info` describes, not by pointer — and it was right to, because probing confirms two calls do not share
an address:

```
a := type_info(Point);
b := type_info(Point);
pa := *a;  pb := *b;
if pa == pb { exit(1); }   // does not happen
exit(2);                   // this does — the two Type_Info values live in different slots
```

That is ADR-0075 §4's own consequence: it declined to promise one `Type_Info` per type, and by-value return
(ADR-0075 §2) means each `type_info(T)` spills its result into a fresh slot, so `*type_info(T)` is a
different address every time. Pointer comparison is out, exactly as ADR-0076 §2 anticipated.

But **comparing what the `Type_Info` says does not work either, as the schema stands.** Its four fields are
`kind`, `name`, `size`, `alignment` (ADR-0075 §3), and none is an identity:

- `kind` is far too coarse — every struct is `STRUCT`.
- `size` and `alignment` collide constantly — `Point{x,y: s64}` and `Pair{a,b: s64}` are both 16/8, and so
  is `[2]s64`.
- `name` is the tempting one and it is **unsound**: nominal identity is a declaration site, not a spelling
  (ADR-0015 §1), so a local `Point` and an imported `Point` are *different types with the same name*.
  Matching on it would let `any_as` hand back a value of the wrong type without a trap — the precise silent
  bad read the checked read exists to prevent.

So `Any` needs a field that *is* an identity, and the schema has none.

## Decision

### 1. `Type_Info` gains `id: s64`, the type's pool id

```
Type_Info :: struct {
    id: s64;                // NEW — the type's canonical identity
    kind: Type_Info_Kind;
    name: string;
    size: s64;
    alignment: s64;
}
```

`id` is the type's `PoolId`, widened to `s64`. It is a total, cheap, **already-canonical** identity: the
pool interns each distinct type to one id and hands the same id back for the same type (ADR-0015), so two
`type_info(Point)` calls carry the same `id` while `Point` and an identically-shaped `Pair` carry different
ones. `any_as(a, T)` compares `a.type.id` to the id of `T`, and traps on mismatch.

**Why the pool id is the right identity, and safe to expose.** It is *the* identity the whole compiler
already uses — every type comparison in `jr-sema`, `jr-mir` and both engines is a `PoolId` equality
(ADR-0015's entire point). Exposing it to a program does not invent an identity or promise a new one; it
surfaces the one that exists. And it is **the same in both engines** without any coordination, because both
share one pool (ADR-0018 §2) — which is why `any_as` can be a plain integer compare that the differential
checks like any other.

**Why `s64` rather than a `u32` or an opaque handle.** A `PoolId` is a 32-bit index, but the language has
no `u32`-typed literal path a program would compare against comfortably, and `s64` is the type every other
`Type_Info` integer field already uses — a reader writing `if info.id == other.id` should not meet a width
surprise. It is not arithmetic; it is an opaque token that happens to be an integer, and `s64` is the
widest, least-surprising carrier. A dedicated opaque `Type_Id` type was rejected: it would need its own
comparison, its own `Type_Info` field type, and a reason to exist that "an integer nobody does arithmetic
on" does not supply.

**Placed first, deliberately.** `id` leads the struct, so the identity is the first thing a reader sees and
the field the compiler's validation pins at offset 0 — the same instinct ADR-0057 §4 had for a variant's
tag. It also means the older four fields keep their relative order, so the ADR-0075 narrative still reads
top-to-bottom after `id`.

### 2. This amends ADR-0075 §3, and the validation moves with it

`TYPE_INFO_FIELDS` gains `("id", Exact(s64))` at the front, so a `Basic` whose `Type_Info` lacks `id` — or
has it in the wrong place — is E0265, exactly as any other shape mismatch is. The amendment is therefore
enforced, not merely documented: the compiler and `Basic` cannot drift on it silently.

ADR-0075 §3 said "the shape extends by adding fields, which does not break a reader that names only the
four". This is the first exercise of that, and it *does* shift the four's offsets by eight bytes — which is
exactly why the validation checks order, and why nothing reads a `Type_Info` field by a hard-coded offset:
`field_offset` computes them, so adding `id` moves `size` and both engines follow.

### 3. What is deliberately still absent

- **A guarantee of one `Type_Info` *object* per type.** `id` gives identity *comparison* without it; two
  `type_info(Point)` calls still produce two objects, they just now agree on `id`. Deduplicating the
  objects is the static-data decision ADR-0075 §2 and ADR-0076 §2 both deferred, and `id` is precisely what
  makes deferring it harmless.
- **Arithmetic or ordering on `id`.** It is an identity token. That it is an `s64` is a carrier choice, not
  a licence to add them; nothing in the language stops a program doing so, and nothing gives it meaning.

## Consequences

- **`any_as` is a plain integer compare and a conditional trap**, which both engines already have — no new
  MIR, and the differential checks the trap fires on the same mismatch in both. This is the payoff of
  choosing the identity the compiler already agrees on.
- **`Type_Info` is now five fields**, and `type_info`'s constant builder gains one element. The `id` is the
  described type's `PoolId` as an `s64`, interned like any other integer element (ADR-0074 §1).
- **ADR-0075's `valid/063` numbers shift.** It asserted `Point`'s size at offset-derived reads; adding `id`
  moves nothing it reads *by name*, but the MIR snapshot changes because the aggregate now has five
  elements. That is a snapshot update, not a regression.
- **This is a schema amendment mid-wave, and it is honest about being one.** ADR-0075 shipped four fields
  believing they were enough; `any_as` proved a fifth is needed for a *sound* checked read. Recording it as
  an amending ADR rather than editing ADR-0075 keeps the reason visible: the four-field schema was not
  wrong for `type_info`, it was insufficient for `Any`, and the two shipped one wave apart.
