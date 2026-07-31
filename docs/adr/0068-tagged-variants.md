# ADR-0068: `variant` is a tagged union with a checked read, and `switch` destructures it

- **Status:** Accepted
- **Date:** 2026-07-31
- **Deciders:** dboll
- **Completes W4.5 — Pattern matching.** ADR-0067 delivered `switch` and exhaustiveness; this is the
  third deliverable §2.1 lists, and the other half of ADR-0045 §1.
- **Follows ADR-0045 §1's instruction rather than reversing it.** That ADR rejected a tagged `union` and
  said what to do instead, in as many words: "a *tagged* variant type becomes worth having — and it
  should be a **different declaration form** then, the way `enum_flags` is different from `enum`
  (ADR-0043 §1), rather than a silent change to what `union` means." This adds the form. `union` is
  untouched and still untagged.

## Context

ADR-0045 rejected a tag on three specific grounds, and it is worth checking each against what the
language now has, because two of the three have changed:

1. **"A tagged union's value comes almost entirely from exhaustive destructuring — `switch v { … }` —
   and Jairs has no pattern matching."** It does now: ADR-0067 shipped `switch` with exhaustiveness
   checking a wave ago. This ground is gone, and it is the one ADR-0045 called decisive.
2. **"A program has no way to *ask* which field is live, since there is no `switch` and no reflection."**
   Also gone: a `switch` over the tag is exactly that question.
3. **"The tag's width would become part of the layout, so a `union` would not be the size of its largest
   field — the one property a systems programmer reaches for a union to get."** This ground **stands**,
   and it is why the tagged form is a *separate* declaration rather than a flag on `union`: a program
   that wants the register-sized, `#foreign`-compatible thing keeps `union`, and one that wants safety
   asks for `variant`. Both remain available, which is what makes the size cost a choice.

One further constraint was found by reading rather than assumed, and it shapes §2:

**`Struct::is_union` is a `bool` on a shared arena, deliberately.** Its own doc calls the sharing
"load-bearing": a `DeclId` is `(file, index-within-its-arena)` and says nothing about *which* arena, so
a separate `unions: Vec<Union>` would give a struct at index 0 and a union at index 0 the same `DeclId`
while they share `Pool::struct_fields` — the two field lists would silently overwrite each other. A
third form therefore cannot be a third arena either, and a `bool` cannot express three kinds. It becomes
an enum (§2), which is a mechanical change across nine readers and makes each of them state which form
it means.

## Decision

### 1. `variant { … }` is a new declaration form

```jr
Value :: variant {
    i: s64;
    f: float64;
}

v: Value;
v.i = 7;          // writing sets the tag
n := v.i;         // reading the live field is fine
b := v.f;         // traps: the tag says `i`
```

A third aggregate form beside `struct` and `union`, spelled with its own keyword for the reason
`enum_flags` has one (ADR-0043 §1): the two differ in *semantics*, not in a detail, and a reader should
see which they are looking at without checking a flag elsewhere.

**Rejected: `#tagged union { … }`** — an attribute on the existing form. It reads as a modifier on a
`union`, and the whole point of ADR-0045 §1's instruction is that the tagged thing is a *different
type* with a different size and a different access cost, not a `union` with a switch flipped.

**Rejected: reusing `union` and making the tag optional.** That is precisely the "silent change to what
`union` means" ADR-0045 forbade.

### 2. `Struct::is_union: bool` becomes `Struct::kind: AggregateKind`

```rust
pub enum AggregateKind { Struct, Union, Variant }
```

The same shared arena, for the reason its doc gives — one arena makes a colliding `DeclId`
unrepresentable — but a *kind* rather than a bool, because three forms do not fit in one. Every one of
the nine sites reading `is_union` becomes a match, which is the point: an exhaustive match makes adding
a fourth form a compile error at each place that must decide, rather than a `false` that silently means
"struct" (the project's first named failure mode, applied to a flag).

`Item::VariantType { decl }` joins `StructType` and `UnionType` in the pool, nominal for the same
reason both are: two `variant`s with identical fields in two files are two types.

### 3. The tag is a **leading field**, so layout stays the pool's ordinary question

A `variant`'s layout is the tag followed by the union of its cases — computed by the *existing*
`sequential_layout` over `[tag_type, union_of_cases]`, so `jr-pool` gains a case in `layout_of` and no
new layout algorithm. The tag is a `u8` when a variant has at most 256 cases, which every variant the
language can express does.

**Leading rather than trailing**, matching how ADR-0057 §4 chose to place the hidden context parameter
and for the same reason: a leading field's offset is 0 regardless of what follows, so nothing has to
compute a position from the case count. A trailing tag would sit at an offset that depends on the
largest case, which every site reading the tag would have to re-derive.

**A `u8` rather than the case's own index type.** One byte, and alignment padding usually absorbs it —
so the size cost ADR-0045 §1 warned about is real but small, and it is the cost a program *opts into* by
writing `variant` instead of `union`.

### 4. A write sets the tag; a read of a non-live field **traps**

Assigning `v.i = 7` stores the value and sets the tag to `i`'s index. Reading `v.i` when the tag says
`i` loads it; reading `v.f` when the tag says `i` **traps**, with a new `TrapKind` and the message
`"read the wrong variant field"`.

**A trap rather than a diagnostic**, because which field is live is not statically decidable — ADR-0045
§1 established that when it rejected a static cross-field check as "either unsound or maddening", and
nothing about a tag changes it. The tag makes the *runtime* answer available, which is what turns an
undetectable bit reinterpretation into a located trap.

**The check is a load, a compare and a branch per field access**, exactly the cost ADR-0045 §1 named.
That cost is now a choice a program makes per type, which is the difference: `union` still costs nothing
and still reinterprets.

**Not strippable by `--no-bounds-check`.** ADR-0003's setting is about *array indices*, where the check
is redundant with a proof the programmer often has; a variant's tag is the only thing that knows which
field is live, so removing the check does not remove a redundancy, it removes the type's meaning. A
program that wants no check writes `union`.

### 5. `switch` destructures a variant by field name, and is exhaustive over its cases

```jr
switch v {
    case i; print("an integer");
    case f; print("a float");
}
```

An arm names a **field**, and the `switch` compares the tag. Exhaustiveness is the same check ADR-0067
§3 applies to an enum, over the variant's fields instead of an enum's members — so E0258 lists the
missing cases and E0260 still refuses an `else` on a complete match. That reuses the wave that just
shipped rather than adding a second matching mechanism, which is the whole reason this wave follows that
one.

**A case names the field, not a binding.** `case i;` does not introduce `i` as a local holding the
value — the body reads `v.i`, whose check now provably succeeds. Binding forms (`case i => n;`) are a
pattern-matching surface ADR-0067 §2 deliberately declined, and adding one here would be that decision
taken sideways.

**A `switch` over a variant makes its reads safe**, and that is the payoff: inside `case i;` the tag is
known, so `v.i` cannot trap. Whether the compiler *elides* the check there is an optimisation, recorded
in §6 as absent — the semantics are the same either way.

### 6. What is deliberately absent

- **Eliding the tag check inside a matching arm.** Sound and worth doing (the arm proves the tag), but
  it is an optimisation over identical semantics, and doing it in the same wave as the feature would mean
  the corpus could not distinguish "the check works" from "the check was removed".
- **Binding a case's value to a name** (§5), and any other pattern surface — ADR-0067 §2's line.
- **A variant in a `#foreign` signature.** Its layout is ours, not C's; a C `union` plus a separate tag
  is what interop wants, and that is what `union` is still for.
- **Nesting a variant in itself** (a recursive `variant`), which needs a pointer indirection and a
  size-computation cycle check. Its own decision.
- **Asking the tag directly** (`v.tag`), which would make the field name part of the surface. `switch` is
  the way to ask, so there is one way.

## Consequences

- **W4.5 closes with this.** All three of §2.1's deliverables — `switch`, exhaustiveness, a tagged
  variant — are then in, and W4 (comptime) is next.
- **One new keyword, `variant`**, and one new pool item, `Item::VariantType`. A program using `variant`
  as an identifier now gets a parse error, the cost of any keyword.
- **`Struct::is_union` becomes `Struct::kind`**, a mechanical change at nine sites that turns each into
  an exhaustive match. **This is the wave's largest diff and its least interesting**, which is worth
  saying so a reviewer reads the shape rather than each line.
- **One new `TrapKind`**, so the trap-kind array's length assertion and `reason()` both change — and the
  new message must be identical in both engines, which `jr_base::trap_message` and the differential
  harness already enforce (ADR-0020 §2).
- **No new diagnostic code.** Exhaustiveness over a variant's cases reuses E0258/E0260 (§5), which is
  the evidence that reusing ADR-0067's machinery was the right shape. **E0261 is still the first free
  code.**
- **A variant is bigger than a union of the same fields**, by the tag plus its padding. That is the cost
  ADR-0045 §1 predicted, and it is now a per-type choice rather than a language-wide one.
