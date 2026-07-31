# ADR-0050: `using` promotes a struct's fields into scope, and a real local always wins

- **Status:** Accepted
- **Date:** 2026-07-29
- **Deciders:** dboll
- **Scope:** Field promotion and struct embedding only. Enum `using` and module `using` are
  **deliberately not here** — see §6.
- **Amends:** ADR-0045 §6, which said "no `using` on a union". That was a statement about `using`
  not existing yet, not a permanent refusal; §5 below decides the union case on its merits.

## Context

`PLAN.md` §2.1 calls `using` "the first genuinely hard resolution problem", and it is the last
large W2 feature. The difficulty is not the syntax — one keyword in three positions — but that
every other name in the language resolves to **one** thing, and a promoted name resolves to a
*path*.

Six facts were established by reading the code before this ADR was written, and four of them
shaped the decisions.

- **`Res` has five variants and every one names a single id**: `Local(LocalId)`, `Param(ParamId)`,
  `Item(ItemId)`, `Imported(ItemId, Symbol)`, `Error`. **This is the fact that decides §2**: `x`
  meaning `p.x` cannot be spelled in that enum, so either the enum grows or something else must
  carry the base.
- **`resolve_name` is a documented four-step list** (ADR-0014 §3, spec §03): block locals,
  parameters, this file's items, then imports. **This decides §3**: `using` inserts a step, and
  *where* it goes is the whole shadowing question. Putting it after parameters means a real
  parameter wins, which is the answer §3 argues for.
- **Resolution never mutates the HIR.** `ResolveCtx` holds `&'a FileHir` and writes only into its
  own `ResolveMap`. So the "rewrite `Expr::Name` into `Expr::Field`" mechanism would be the first
  pass to mutate the tree it reads, and that is why §2 rejects it.
- **`Field`, `Param` and `Local` are three separate structs**, each with `name`, `name_span` and
  `ty`. `using` is legal in all three positions, so each grows the same flag — which is repetitive
  and is still better than a shared "bindable thing" abstraction that exists only for this.
- **A struct's fields already live in a `jr-pool` side table keyed on `DeclId`** (ADR-0041 §4,
  ADR-0045 §4), and `field_offset` answers an offset per field. **This decides §4**: if an embedded
  base stays a real field, layout needs *nothing* and `using` is purely a resolution feature.
- **`check_field` already resolves a name against a struct type and suggests a near name on
  failure** (ADR-0031 §1). An embedded field's access is an ordinary two-step field access, so
  sema needs no new diagnostic for the common case.

## Decision

### 1. `using` in three positions, all meaning "promote this thing's fields"

```jr
Point :: struct { x: s64; y: s64; }

// A parameter.
len2 :: (using p: Point) -> s64 {
    return x * x + y * y;          // `x` is `p.x`
}

// A local.
main :: () {
    using q: Point;
    x = 3;                          // `x` is `q.x`
}

// A struct field — "embedding".
Entity :: struct {
    using base: Point;              // `x` and `y` become reachable through an Entity
    hp: s64;
}
```

**The promoted thing must be a struct** (or a pointer to one, auto-dereferenced exactly as
`a.b` already is). Promoting anything else has no fields to promote and is E0250.

**`using` is a prefix on the binding, not a statement.** `using p: Point;` declares *and* promotes;
there is no separate `using p;` that promotes an already-declared variable. Jai has the second
form, and it is deliberately absent because it makes the set of names in scope depend on a
statement's *position* within a block, which is a second order-sensitivity rule on top of the one
locals already have. One form, one rule.

**Rejected: `using` as a bare statement over an expression** (`using get_point();`). It would need
a rule for the lifetime of the promoted temporary and for what happens when the expression is
called twice. A named binding has an obvious answer to both.

### 2. A promoted name resolves to `Res::Promoted`, a new variant

```rust
pub enum Res {
    Local(LocalId),
    Param(ParamId),
    Item(ItemId),
    Imported(ItemId, Symbol),
    /// A field reached through a `using` binding.
    Promoted {
        /// What the fields were promoted *from*.
        base: Box<Res>,
        /// The field's name in the base's type.
        field: Symbol,
    },
    Error,
}
```

The `Box` is because `Res` is otherwise `Copy` and a self-referential enum cannot be; that costs an
allocation per promoted *resolution*, not per lookup, and a promoted name is rare compared to an
ordinary one.

**The reason this is right rather than merely workable: every exhaustive match over `Res` becomes a
compile error.** The house style forbids `_` arms precisely so that adding a variant is a compile
error at every site that must change — and this variant must be handled in `jr-hir`'s dump,
`jr-sema`'s checker, `jr-mir`'s lowering and four LSP handlers. A mechanism that changed none of
them would be a mechanism that had *silently* not taught them.

**Rejected: rewriting `Expr::Name` into `Expr::Field` during resolution.** Genuinely tempting —
nothing downstream changes at all, because sema, MIR and the LSP would see an ordinary field
access. Rejected for two reasons. It makes resolution mutate the HIR, which it has never done and
which would make `resolve`'s output no longer a pure function of its input; and the LSP would then
report `p.x` where the user wrote `x`, so hover and goto-definition would describe a construct
absent from the source. The second is the worse one: a tool that lies about the text is worse than
one that needs teaching.

**Rejected: a side map on `ResolveMap`.** A parallel `promoted: HashMap<(ExprScope, ExprId), …>`
beside `resolutions` keeps `Res` untouched — and that is the objection, not the benefit. A consumer
reading only `resolutions` would silently see nothing for a promoted name and treat it as
unresolved. The `Res` variant makes the same consumer fail to compile.

### 3. Resolution order: a real binding always wins, silently

`using` inserts **one step** into ADR-0014 §3's list, after parameters and before file items:

1. Block locals
2. Parameters
3. **Promoted fields, from the innermost enclosing `using` outward**
4. This file's items
5. Imports

**A real local or parameter shadows a promoted name, with no diagnostic.** That is exactly how a
file-scope item shadows an imported name today (ADR-0014 §3), and reusing the rule rather than
inventing one is the point.

```jr
f :: (using p: Point) {
    x := 99;        // an ordinary local
    y = x;          // `x` is the local; `y` is `p.y`
}
```

**Rejected: a promoted field shadowing a local.** It would mean adding a field to a struct silently
changes what an unrelated local name means in every procedure that `using`s that struct — action at
a distance, the same objection ADR-0014 §3 made about import order deciding behaviour and ADR-0048
§3's orphan rule made about `#import` redefining `+`. Named here so the ADR argues it down rather
than leaving it to look plausible later.

**Two `using`s promoting the same name is an error at the *use site*, not at the `using`.**

```jr
g :: (using a: Point, using b: Point) {
    // Both promote `x` and `y`. Legal so far.
    z := x;         // E0250: `x` is promoted by both `a` and `b`
    w := a.x;       // Fine — qualified, so unambiguous
}
```

This is ADR-0014 §3's ambiguity rule verbatim: overlapping providers are harmless as long as the
ambiguous name is never referenced, and the diagnostic names both providers. Refusing at the
declaration instead was considered and rejected for one concrete reason — it would make two
overlapping embeds illegal even in a procedure that only ever uses the qualified forms, which the
import rules deliberately permit.

**Innermost first among promotions.** A `using` local shadows a `using` parameter, matching how
locals shadow parameters. Two promotions at the *same* depth are the ambiguity above.

### 4. An embedded base stays a real field; layout is unchanged

`Entity` contains a `Point` at an offset, and `e.x` compiles to `e.base.x`.

```text
Entity :: struct { using base: Point; hp: s64; }

offset 0   base.x
offset 8   base.y
offset 16  hp
```

**`jr-pool` needs nothing at all**, which is the evidence this is the right split: `using` is a
*resolution* feature and touching layout would have made it two features. ADR-0018 §2's
one-layout-computation rule stays untouched, and `e.base` remains nameable — Jai allows it, and a
user who wants the whole embedded value should be able to say so.

**Rejected: flattening the fields into the outer struct.** `Entity`'s fields would become literally
`x, y, hp` with no `base` to name. Marginally shorter access paths, and it makes `using` a layout
feature: `jr-pool` would have to splice field lists, which means a struct's field list depends on
another struct's, which means a change to `Point` silently repositions `Entity`'s own `hp`. It also
deletes `e.base`, which is a capability rather than a detail.

**Embedding is transitive.** `using` on a field whose type itself embeds resolves through both
levels, because the promotion is computed from the type's fields and that type's fields already
include its own promotions. **A cycle is impossible** — a struct cannot contain itself by value, so
the existing recursive-type refusal already covers it, and this ADR adds no new check for it.

### 5. `using` on a union is refused, and this decides what ADR-0045 §6 deferred

ADR-0045 §6 said "no `using` on a union", on the grounds that `using` was W2's feature and did not
exist. It exists now, so the question is live rather than deferred, and the answer is **still no** —
but for a *reason* rather than for absence:

a union is untagged (ADR-0045 §1), so exactly one of its fields holds a valid value and nothing
records which. Promoting all of them into scope would put several names in scope of which all but
one read reinterpreted bits — and unlike an explicit `u.f`, a promoted `f` gives the reader no
syntactic clue that a union is involved. E0250 covers it, with a message that says so.

This is a refusal that a tagged variant type (ADR-0045 §1's deferred decision) would overturn,
because a tag makes "which field is valid" answerable. A future ADR may do that; this one records
why the answer is no *today*.

### 6. What is deliberately absent

- **No `using` on an enum.** `using Colour;` making `RED` unqualified is a real Jai feature, and
  ADR-0046 already solved the case that motivates it: a bare `.RED` takes its type from context, so
  the qualification is usually unnecessary. Adding a second mechanism for the same goal — one
  context-driven, one scope-driven — would create two ways to write one thing and a question about
  which wins.
- **No `using` on a module.** `using Basic;` making `print_int` unqualified is a *third* resolution
  rule layered onto ADR-0014 §3's flat merge, and it interacts with the ambiguity reporting the
  import rules already do. Its own decision.
- **No `using` on a procedure's return value**, and no `using` in a `for` header.

## Consequences

- **`Res` gains a variant, so every exhaustive match over it is a compile error until taught.**
  That is the mechanism working as intended, and the list is known in advance: `jr-hir`'s dump,
  `jr-sema`'s `check_name` and assignability check, `jr-mir`'s lowering and `scan`, and the LSP's
  hover, goto-definition, references and rename. **A `Box` in `Res` costs its `Copy` impl**, so
  every site that copied a `Res` now clones one — a mechanical change the compiler points at.
- **`Field`, `Param` and `Local` each gain a `using: bool`.** Three structs, one flag, no shared
  abstraction — deliberately, because a trait existing only to unify three bools is more machinery
  than it saves.
- **`USING_KW` leaves the tree-sitter reserved match** — the seventh keyword to make that trip
  after `cast`, `enum`, `union`, `xx`, `for` and `defer`. Six for six on that trap so far; this is
  checked in advance rather than discovered, and `editors/nvim/verify.lua` gets the assertion.
- **`jr-fmt` needs `using` in three emitters** — the field, the parameter and the local — and the
  formatter has *deleted* a construct in three consecutive waves when a kind or a token went
  unhandled. A test must assert `using` survives **and** that the output is canonicalised, because
  the round-trip gate is green for a formatter that emits raw text (ADR-0049's lesson).
- **One new diagnostic code, E0250**, covering four refusals with distinct notes: `using` on a
  non-struct, `using` on a union, an ambiguous promoted name, and a promoted name that does not
  exist. **E0251 is the first free code**; E0128 remains the first free *parser* code, and the
  parser needs one for a malformed `using`.
- **MIR lowering builds a field access from a `Res::Promoted`**, reusing the place-projection path
  an ordinary `p.x` takes — so there is no new MIR node and no back-end change, the fourth wave
  running where the lowering needed nothing new. `Res::Promoted`'s `base` is itself a `Res`, so a
  promoted name reached through an embedded field is a *chain* of projections, and lowering must
  walk it rather than assuming one level.
- **A corpus program must exercise a promoted name that is shadowed by a local**, because §3's
  "a real binding wins silently" is invisible in any program where the names differ — and getting
  it backwards is a silent wrong answer rather than an error.
- **A corpus program must read an embedded field through two levels** (`Entity` embedding `Point`,
  accessed as `e.x`), because §4's transitivity claim is otherwise untested and a one-level
  implementation would pass every single-level test.
