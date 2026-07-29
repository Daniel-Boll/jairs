# ADR-0045: `union` is untagged, and says so in its diagnostics rather than in its representation

- **Status:** Accepted
- **Date:** 2026-07-28
- **Deciders:** dboll
- **Depends on:** ADR-0015, whose nominal-identity rule a union follows exactly, and ADR-0018
  §2, which owns the layout this changes by one rule.

## Context

`PLAN.md` §2.1 lists `union` in W1 beside `struct`, `enum` and `enum_flags`. It is the last
of W1's *types*, and §7 flagged it as the one place where "the safe option and Jai's differ",
which is the fork this ADR settles.

Five facts were established by reading and running the code before this ADR was written, and
two of them decided the fork.

- **`union` has been a reserved keyword since the slice.** `UNION_KW` exists, it lexes, it is
  inside `is_reserved_keyword`'s range, and it appears in `docs/spec/01-lexical.md`'s table
  marked W1. So this wave *removes* a refusal, which is the same shape the `enum` and `cast`
  waves had — and the same trap: a keyword that becomes real must come out of the reserved
  block in the parser **and** out of the tree-sitter highlight query.
- **A union is a struct with one layout rule changed.** Nominal identity keyed on `DeclId`
  (ADR-0015 §1), a field list in a pool side table, `Projection::Field` by index, field access
  through `.`, auto-deref through pointers. Every one of those is shared. What differs is
  `field_offset` — every field at 0 — and `layout_of` — size is the *maximum* field size
  rather than the running sum.
- **There is no pattern matching in the language, and none is planned for W1.** No `switch`,
  no `match`, no `case`; `SyntaxKind` has no such token and `PLAN.md` §2.1 does not list one
  before W2's control flow. **This is the fact that decides the fork**, and it is argued in §1.
- **`differential.rs` compares observable output**, so a union's behaviour is only tested if a
  corpus program observes it. The `Statement::Zero` miscompile (ADR-0039 §4a) went undetected
  for waves precisely because nothing observed a default-initialised aggregate, and a union is
  another aggregate whose initialisation has a rule.
- **`field_offset` returns `LayoutError::NotAType` for a non-struct.** So a union reaching it
  without an arm would produce a *compiler* error rather than a wrong offset, which is the
  right failure direction and worth confirming rather than assuming.

## Decision

### 1. The union is **untagged**, because a tag with nothing to read it is a cost with no benefit

```jr
Value :: union {
    i: s64;
    f: float64;
}

v: Value;
v.i = 1;
n := v.f;   // legal, and reinterprets the bits
```

Writing one field and reading another is **allowed**, and it reinterprets the bits. This is
Jai's semantics and C's.

**Rejected: a tagged union**, with a hidden discriminant set on write and checked on read, so
that reading the wrong field traps. This is the *safe* option and it is what §7 flagged as the
real fork. It is rejected on the fact established above: **Jairs has no pattern matching**.

That matters more than it sounds. A tagged union's value comes almost entirely from
exhaustive destructuring — `switch v { case i: …; case f: … }` — where the compiler proves
every case is handled and the tag is checked once. Without it, the tag can only be spent on a
*runtime trap per field read*, which:

- costs a load, a compare and a branch on **every** field access, in a systems language whose
  ADR-0003 made even a bounds check a strippable build setting rather than an unconditional one;
- gives a program no way to *ask* which field is live, since there is no `switch` and no
  reflection until W4's RTTI — so the only way to avoid the trap would be to remember, which is
  exactly what an untagged union asks of you anyway;
- and would make the tag's width part of the layout, so a `union` would not be the size of its
  largest field, which is the one property a systems programmer reaches for a union to get
  (`#foreign` interop, and a variant that fits a register).

So a tag would impose a per-access cost and a size cost to deliver a diagnostic the language
cannot yet phrase a way to avoid. When W2 brings pattern matching and W4 brings RTTI, a *tagged*
variant type becomes worth having — and it should be a **different declaration form** then,
the way `enum_flags` is different from `enum` (ADR-0043 §1), rather than a silent change to
what `union` means. Recorded here so that the later wave adds a form rather than reinterpreting
this one.

**Rejected: refuse cross-field reads statically.** A tempting middle: keep the untagged layout,
but have sema reject reading a field other than the last one written. Rejected because it is
not decidable — the last write may be in another procedure, behind a pointer, or in a loop —
so the check would be either unsound (miss cases) or maddening (refuse working code). A partial
static check on a fundamentally dynamic property is worse than an honest absence of one.

### 2. Untagged is a *documented* hazard, not a silent one

The safety this wave declines to provide in the representation is provided in the
documentation and the diagnostics instead:

- The **module and ADR docs say plainly** that reading a field other than the one last written
  reinterprets bits, with the word "reinterprets" rather than a euphemism.
- **`README.md`'s language table lists `union` with the caveat in the same row**, so the
  outward-facing inventory cannot advertise it as safe.
- The corpus file that exercises it is **named for what it is** and its comment states the
  hazard, so the first thing anyone reading `union` in this project sees is the trade.

This is a deliberate application of ADR-0043's lesson that a diagnostic can be accurate and
useless: nothing here can produce a diagnostic at all, so the honesty has to live where a
reader will actually meet it.

### 3. Layout: every field at offset 0, size is the largest field, alignment the strictest

```text
union { a: u8; b: s64; }   →  size 8, align 8, both fields at offset 0
union { a: u8; b: u16; }   →  size 2, align 2
union { }                  →  size 0, align 1
```

Size is rounded up to the alignment, exactly as a struct's is, so an array of unions stays
aligned at every element. This is C's rule and Jai's.

Computed in `jr-pool`'s `layout_of` and `field_offset` — the one place layout may be computed
(ADR-0018 §2) — so the VM and Cranelift cannot disagree. That is not a formality: a union is
the construct where a layout disagreement is *invisible* rather than a crash, because both
engines would read plausible bits from the wrong place.

**An empty union is legal and zero-sized**, matching an empty struct. Refusing it would be a
special case with no argument behind it.

### 4. `Item::UnionType { decl }` — nominal, and a *separate variant* from `StructType`

Not `StructType` with a `union: bool` flag. The two differ in *layout*, which is the one thing
a `PoolId` holder cannot look up without a side table — and every consumer that computes an
offset must branch on it. A separate variant makes each of those sites a compile error until it
is handled, which is what the house style's ban on `_` arms is for; a boolean field would let a
site that forgot to check compute a struct's offsets for a union and produce wrong addresses.

The field list is **shared storage**: `Pool::set_struct_fields` and `Pool::struct_fields`
already key on `DeclId` and know nothing about which kind of declaration it was, so a union's
fields live there too. One side table, because the *fields* are the same data; two `Item`
variants, because the *layout* is not.

### 5. Everything a struct's field access does, a union's does identically

`v.i`, `v.i = 1`, `p.i` through a pointer with auto-deref, `Projection::Field(index)`, the
`no_such_field` diagnostic with its near-name suggestion, LSP completion and hover. All of it
falls out of sharing the field list, and none of it needs a rule.

A union is an **aggregate**: never register-promoted, always slotted, zeroed by
`Statement::Zero` on a default initialisation (ADR-0039 §4a), passed by copy, and refused as a
return value by the same pre-existing Cranelift gap that refuses a struct (ADR-0044 §5).

**Zeroing a union zeroes the whole slot**, which is its largest field's worth of bytes — so
every field reads as zero afterwards, and no field is left holding stack garbage. Worth stating
because it is the one place the untagged design and the zeroing rule interact, and the answer
happens to be the safe one.

### 6. No `using` on a union, and no anonymous unions inside structs

Jai allows `using` to hoist a union's members into the enclosing scope, and C11 allows an
anonymous union as a struct member. Both are genuinely useful and both are out of scope: `using`
is W2's (it is a reserved keyword and `PLAN.md` §2.1 lists it there), and an anonymous member
needs a name-less field in the field list plus a resolution rule for hoisted names — a separate
decision with its own failure modes.

Stated so it is not discovered: a union inside a struct must be a **named** field of a named
union type, which is one more line than C needs.

## Consequences

- **`UNION_KW` leaves the reserved block**, and the tree-sitter highlight query's reserved
  match must lose it too. This is the third time that pairing has come up (`cast`, `enum`, now
  `union`), and §7's trap list already names it — so it is checked rather than discovered.
- **`Item::UnionType` is the fourth nominal-or-structural type variant added since the slice**,
  so every exhaustive `Item` match gains an arm: `is_type`, `type_of`, `layout_of`,
  `field_offset`, `describe`, `Shape`, `Repr`, `escape.rs`, MIR's `dump` and `verify`, the LSP's
  `render` and `completion`. The compiler lists them.
- **`field_offset` gains a union arm returning 0 for every field**, and this is the single most
  consequential line in the wave: it is what makes a union a union, it is shared by both
  engines, and getting it wrong would be a silent wrong-address bug rather than an error.
- **`ConstValue::Union(UnionId)` beside `Struct(StructId)`**, and `jr-fmt` needs the new
  const-declaration value kind in **two** places — the kind predicate and the const-decl
  dispatch — which is §7's standing trap and cost the `enum` wave a `Colour :: ;`.
- **A `DeclId` still does not say which kind of declaration it is** (ADR-0041 §4a), so a union
  and a struct at the same `DeclId` index would collide in `struct_fields`. They cannot: the
  index is the declaration's position in its own arena, and a declaration is in exactly one
  arena. Confirmed rather than assumed, because the hazard is real for `enum` already.
- **The corpus gains a program that writes one field and reads another**, in both engines, with
  the *same* observable answer — which is the only way an untagged union's defining behaviour
  can be pinned. A differential test that merely ran a union without reinterpreting would prove
  nothing about the decision this ADR makes.
