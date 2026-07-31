# ADR-0043: `enum_flags` numbers by powers of two, and a combination is a value rather than a member

- **Status:** Accepted
- **Date:** 2026-07-28
- **Deciders:** dboll
- **Depends on:** ADR-0042, which supplied the operators this type exists to be used with.
  ADR-0041 §3 recorded `enum_flags` as **blocked** on them rather than deferred, and this is
  that unblocking.

## Context

`PLAN.md` §2.1 lists `enum_flags` beside `enum` in W1. ADR-0041 shipped `enum` and refused
`Colour.RED | Colour.GREEN` (§6), on the grounds that an enum's members are named
*alternatives* rather than magnitudes. A flags enum is exactly the case where they genuinely
*are* combinable, and it is a separate declaration form precisely so that the two cannot be
confused.

Three facts were established by reading the code before this ADR was written.

- **`enum_flags` has no token at all.** Unlike `enum`, `union` and `xx`, it was never
  reserved — it appears in the workspace only in two `jr-sema` comments predicting it. So
  this wave adds a keyword, which no wave since the slice has done.
- **The enum machinery generalises.** `Item::EnumType { decl }`, the `enum_members` side
  table, `ReceiverKind::Enum`, `no_such_member` and the `Colour.RED` constant fold are all
  independent of *how* members are numbered.
- **The refusals to lift are precisely locatable.** `reject_enum_operator` (ADR-0041 §6)
  refuses arithmetic and ordering on any enum, and `reject_bitwise` (ADR-0042 §5) refuses
  bitwise on any non-integer. A flags enum must pass the second and still fail the first.

## Decision

### 1. `enum_flags` is a distinct declaration form, not a modifier

```jr
Perm :: enum_flags { READ; WRITE; EXEC; }
```

A new keyword and a new `ConstValue` variant, beside `enum`. Not `enum #flags { … }` or an
attribute, because the *numbering rule differs* — and a numbering rule that changes based on
a decoration is a rule a reader has to look for. Jai does the same.

The two forms share everything else: nominal identity keyed on `DeclId`, members in the
`enum_members` side table, `Perm.READ` as a namespaced constant, `no_such_member` with its
suggestion. Only the numbering and the permitted operators differ.

### 2. Powers of two from 1, and no implicit zero member

- **Auto-numbered `1, 2, 4, 8, …`** in declaration order.
- **An explicit value is allowed**, and a later member continues from **the next power of two
  strictly above it**: `enum_flags { A; B :: 8; C; }` gives 1, 8, 16.
- **Zero is not auto-created.** A program that wants it writes `NONE :: 0;`.

```jr
Flags :: enum_flags {
    NONE :: 0;   // explicit, and 0 does not disturb the sequence
    A;           // 1
    B :: 8;
    C;           // 16
}
```

The continue-from-here rule is the part that is easy to get wrong twice over: it must be the
next power of two **above the previous value**, not the next power of two after the previous
*index*, and not the previous value doubled when the previous value was not a power of two.
An explicit `B :: 3` is legal — a mask of two flags given a name — and `C` after it is 4.

**Rejected: injecting a `NONE :: 0` automatically.** Convenient, since `f == Perm.NONE` would
always work. Rejected because it puts a name in the type's namespace the programmer did not
write: if they also write `NONE :: 0;` there are either two members with one name or a
collision error about a member they cannot see. An explicit zero is one line and says what it
is.

**Rejected: sequential numbering like a plain `enum`.** Simplest and consistent, and it
defeats the entire purpose: `READ|WRITE` would be `0|1` = 1, which *equals* `WRITE`. A
combination colliding with a member makes the type useless for its one job, and the collision
would be silent.

### 3. `& | ^ ~` yield the flags type; `<<` `>>` stay refused

```jr
f := Perm.READ | Perm.WRITE;      // Perm, value 3
if (f & Perm.READ) == Perm.READ { … }
g := ~Perm.READ;                  // Perm
h := f ^ Perm.EXEC;               // Perm
```

The result type is the **flags enum**, not its backing integer. That is what makes the type
worth having: a `Perm` stays a `Perm` through a combination, so it cannot be passed where an
`s64` belongs or vice versa.

**A combination names no member, and that is correct rather than a gap.** `Perm.READ |
Perm.WRITE` is 3, and no member has that value. `describe` therefore cannot render it as a
member name and does not try — a flags value is a *set*, and the type's job is to keep the
set distinguishable from an integer, not to name every subset.

**Shifts stay refused.** `Perm.READ << 1` would produce `WRITE` by an accident of the
numbering, which is the same accident-of-declaration-order objection ADR-0041 §6 used to
refuse ordering. Arithmetic (`+ - * / %`) and ordering (`< <= > >=`) stay refused too, for
ADR-0041 §6's reasons unchanged: they applied to "an enum" and a flags enum is one.

`==` and `!=` work, as they do for a plain enum. They are how a flag test is written.

**Rejected: a `has` operator or intrinsic.** `f has Perm.READ` reads better than
`(f & Perm.READ) == Perm.READ`. Rejected because Jai uses the `&` idiom and inventing an
operator is a language-surface decision wider than this wave — and because the idiom composes
(`f & (A|B)` tests two flags) where a binary `has` would not.

### 4. A plain `enum` keeps *all* of ADR-0041 §6's refusals

`Colour.RED | Colour.GREEN` remains an error. This is the whole reason `enum_flags` is a
separate form: if bitwise worked on both, the declaration would carry no information and the
numbering difference would be the only thing distinguishing a set from an alternative — which
is exactly the kind of implicit distinction that produces the collision §2 rejected.

The diagnostic for bitwise on a plain enum should say so, rather than only "not supported":
`enum_flags` is the answer, and a reader who does not know it exists cannot find it.

### 5. `cast(s64, f)` works; `cast(Perm, 1)` does not

Unchanged from ADR-0041 §3: a flags enum casts *to* a numeric type and not *from* one.
`cast(Perm, 3)` would manufacture a value from an integer with no check that it names
anything — and for flags the hole is wider than for a plain enum, because *most* integers are
valid flag sets, so a wrong one would look right.

The consequence, stated because it bites: there is no way to build a `Perm` from a computed
integer. A program needing one combines members with `|`, which is the operation §3 exists to
provide.

### 6. Members of a flags enum need not be distinct, and need not be powers of two

`enum_flags { A :: 3; B :: 1; }` is legal. `A` is a named mask, `B` is a flag, and they
overlap. C and Jai both allow this and it is the standard way to name a common combination
(`ALL :: 7`).

No check is added for "this member is not a power of two" or "these members overlap", because
both are things a programmer does deliberately. A lint could suggest otherwise later; a
*refusal* would reject working code.

## Consequences

- **`FLAGS_KW` is the first keyword added since the slice.** Every keyword list must learn
  it: the lexer table, `from_keyword`, `static_text`, `is_reserved_keyword`'s range (it goes
  *outside* that range, unlike `ENUM_KW`, because there is no reason to add a keyword straight
  into the reserved block), the tree-sitter grammar, and the highlight query. That last one is
  the trap `cast` and `enum` both hit from the other direction.
- **`Item::EnumType` gains a `flags: bool`**, which is part of the interning key. Two enums at
  the same `DeclId` cannot differ in it, so this is redundant *as identity* — but it means
  every consumer that has a `PoolId` can answer "is this a flags enum" without a side-table
  lookup, and the sema checks in §3 need that answer at every operator site.
- **The numbering lives in `jr-sema` beside `enum`'s**, in the same function, because the two
  rules differ by one expression and separating them would let them drift on everything else
  they share.
- **`enum_flags` completes the `enum` family**, so §7's W1 list loses an item that was
  *blocked* rather than merely pending — the distinction ADR-0041 §3 insisted on.
