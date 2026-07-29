# ADR-0041: `enum` is a nominal type whose members are namespaced, and `.RED` is owed

- **Status:** Accepted
- **Date:** 2026-07-28
- **Deciders:** dboll

## Context

`PLAN.md` §2.1 puts `enum`, `enum_flags` and `union` in W1, and §7 named `enum` next
because it is **the first new nominal type since `struct`** — so it touches the pool, layout
and both back ends rather than mapping onto something that already exists.

Four facts were established by reading the code before this ADR was written.

- **`ENUM_KW` is already a token**, refused by the parser with E0121 "`enum` arrives in wave
  W1". Like ADR-0040's E0120, that message becomes a lie the moment this wave lands, so the
  refusal is removed rather than reworded.
- **ADR-0012 already decides the declaration form.** `name :: value` is *the* compile-time
  constant form, and `Point :: struct { … }` is an instance of it. `Colour :: enum { … }` is
  the same rule with a different value, so this ADR adds no declaration syntax — it adds a
  `ConstValue` variant beside `Struct`.
- **ADR-0015 already decides the identity.** A `struct` is nominal, keyed on its declaration
  site. An enum needs the same treatment for the same reason, and `DeclId` plus the
  `struct_fields` side-table is a shape that generalises.
- **`Colour.RED` needs no new syntax.** `FIELD_EXPR` exists, `check_field` dispatches on the
  receiver's type, and `field_place` turns one into a place. An enum member is a *fourth*
  `ReceiverKind` beside `Str`, `Array` and `Struct`.

One stale comment was also found and is fixed here rather than left: `kind.rs` still
described `CAST_KW` as "reserved, wave W1" after ADR-0037 made it real syntax. A keyword
comment claiming a feature has not arrived, three waves after it did, is the same
plan-contradicts-code failure this project has named twice.

## Decision

### 1. Members are namespaced. `Colour.RED` works; bare `.RED` does not, yet

```jr
Colour :: enum { RED; GREEN; BLUE; }

c := Colour.RED;
if c == Colour.GREEN { … }
```

Members live in the enum's own namespace and **never** enter the enclosing scope. Adding a
member to an enum therefore cannot shadow an existing name, and two enums may share a
member name without colliding.

**Rejected: members leak into the enclosing scope, C-style.** `RED` alone would resolve.
Rejected for the reason C regrets it — two enums with a shared member name collide, and
adding a member can break unrelated code — and the hazard is *worse* here than in C:
ADR-0014's flat import merge means an imported enum's members would enlarge the name space
every identifier in the file resolves against, which is exactly the correctness problem
ADR-0031 §3's unused-import warning exists to catch.

`Colour.RED` reuses field-access syntax deliberately. `check_field` and `field_place` gain a
receiver kind rather than a sibling implementation, so the one place that answers "what does
`a.b` mean" keeps answering it.

### 2. Bare `.RED` is deferred, and here is exactly what it needs

Jai allows `c: Colour = .RED;` — a member named without its type, where the target type is
known from context. **Jairs does not, yet.** This is the one place this ADR knowingly ships
less than Jai, so the plan is written down rather than left as "later".

**Why it is not free.** Every context-typing rule so far pushes a *type* inward to a literal
that has none: ADR-0016 §1 for integers, ADR-0040 §5 for floats. `.RED` is different in kind
— it asks the context to supply a **namespace to resolve a name in**, not a type to give an
untyped value. That is a new resolution rule, not a new literal.

**What it requires, concretely:**

1. **A syntax node.** `.RED` is not a `FIELD_EXPR` with a missing receiver: `parse_postfix_chain`
   only reaches a `.` after an expression. It needs its own prefix form, and the parser must
   not confuse it with a float — `.5` is not a member, and the lexer's rule that a `.` begins
   a fractional part only when a digit follows is what makes that unambiguous. A
   `MEMBER_EXPR` kind, with `is_expr_kind` and `EXPR_START` both updated (the trap that has
   swallowed two features already).
2. **An `Expr::Member { name, span }` in HIR** that resolves to nothing during lowering,
   because resolution needs a type.
3. **A sema rule** in `check_expr`'s `expected` path: with `Some(ty)` where `ty` is an enum,
   look the name up in that enum's members; with `None`, a diagnostic that says *why* — "the
   type of `.RED` cannot be inferred here" — rather than "unresolved name `RED`", which
   would send the reader looking for a declaration.
4. **Every `expected` site audited.** `check_operands` passes the *other operand's* type as
   context, so `if c == .RED` should work; a call argument passes the parameter type, so
   `f(.RED)` should too. Those are the two places a Jai programmer will try first, and each
   is a separate check that the context actually reaches the member.
5. **A decision about `.RED` in a `switch`**, which does not exist yet — W2 owns `for` and
   friends and there is no `switch` in any wave's list, so this is a question W2 or later
   will have to answer rather than one to pre-empt.

**Why it is safe to defer.** `Colour.RED` is the explicit form and stays valid forever, so
nothing written today breaks when `.RED` arrives. The reverse is not true: shipping `.RED`
with a wrong resolution rule would need a *reversal* ADR, and getting the "no context"
diagnostic wrong would teach users to write `Colour.RED` anyway.

This is recorded as an owed item in `PLAN.md` §7 with the five steps above, not as a vague
"add autocast for enums".

### 3. Representation: Jai's rules exactly

Jai's enum is an integer type with a nominal wrapper. The rules, and Jairs adopts each:

- **Auto-numbered from 0**, in declaration order: `enum { RED; GREEN; BLUE; }` gives 0, 1, 2.
- **An explicit value is allowed**, and **later members continue from it**:
  `enum { A; B :: 10; C; }` gives 0, 10, 11. This is C's rule and Jai's, and the
  continue-from-here behaviour is the part that is easy to get wrong by resetting to
  `index`.
- **Duplicate values are legal.** `enum { A :: 1; B :: 1; }` is two names for one value. C
  and Jai both allow it, and it is occasionally what someone means.
- **The backing type is `s64`**, matching the integer literal default (ADR-0016 §1). No
  layout logic is new: an enum's `Layout` *is* its backing type's.
- **The enum is nominal, not an alias.** `Colour` is not `s64`, a bare integer cannot be
  passed where a `Colour` belongs, and `cast(s64, c)` is how the number is obtained.

**Rejected: an explicit backing type now** (`Colour :: enum u8 { … }`). Jai has it and it
matters for FFI and packed structs. Rejected for this wave because nothing in the corpus or
`modules/Basic` needs it, so its shape would be chosen with no test to justify it — and
adding it later is a parse form plus a per-member range check, neither of which changes
anything decided here.

**Rejected: a transparent alias for the integer type.** No cast needed, arithmetic works
directly. This throws away the only thing an enum buys — that `Colour` and `s64` are
different types is what stops a raw integer being passed where a colour belongs — and
ADR-0015 makes `struct` nominal for exactly this reason.

**Rejected: `enum_flags` in this wave.** It is meaningless without `& | ^ ~`, which are
still refused with E0122 and are their own §7 item. It is **blocked**, not merely deferred,
and saying so is the difference between a plan and a wish.

### 4. `Item::EnumType { decl }`, with members in a side table

Structurally identical to `Item::StructType`: nominal, keyed on `DeclId`, with the member
list stored separately via `set_enum_members`/`enum_members` so that identity exists before
the members are resolved.

That separation is not copied for symmetry — it is load-bearing for the same reason it is
for structs (ADR-0015 §1): a member's *value* is a constant expression that resolution may
have to evaluate, and the type must have an ID before that starts.

An enum member is a `(Symbol, i64)` pair rather than a `Field`: a field has a type and a
member has a value, and reusing `Field` would mean a `PoolId` field that is always the same
`s64` and a name that lies about what it holds.

#### 4a. A struct and an enum can share a `DeclId`, and that is safe for one reason

`DeclId` is `(FileId, u32)`, and the `u32` comes from the *arena index* — `StructId` for a
struct, `EnumId` for an enum. Both arenas start at 0, so the first struct and the first enum
in a file both get `DeclId(file, 0)`.

This is safe, and the reason is worth stating because it is not obvious and would be easy to
break:

- **The types stay distinct** because `Item::StructType { decl }` and
  `Item::EnumType { decl }` are different *variants*. Interning keys on the whole `Item`, so
  two different variants with equal payloads are two different `PoolId`s.
- **The side tables stay distinct** because there are two of them: `struct_fields` and
  `enum_members` are separate maps, so `DeclId(file, 0)` in one has no relation to the same
  key in the other.

**What would break it:** one map keyed by `DeclId` holding both kinds of body, or any code
that treats a `DeclId` as identifying a declaration *without* knowing which kind it is. A
future `type_info()` (W4's RTTI) that maps `DeclId` to "the declaration" would hit this
immediately.

The alternative — a single arena index space for all nominal declarations — was rejected as a
larger change than the hazard warrants, and one that would renumber every existing struct's
`DeclId`. Recorded as a trap in `PLAN.md` §7 instead, which is the cheaper half of the same
protection.

### 5. `Colour.RED` is a *value*, not a place

Field access on a struct yields a location — `p.x = 1` assigns. An enum member does not:
`Colour.RED = 2` is meaningless, and `*Colour.RED` has no address to take because the member
is a compile-time constant with no storage.

So `check_field`'s enum arm returns the enum type, and `is_place` answers `false` for it —
the same answer it gives a `cast`, and for the same reason. `field_place` returns `None`, and
the *value* path folds the member to an interned constant, which is the shape `.count` on an
array already needed (ADR-0039 §5). Getting this wrong is how `Colour.RED` would have
refused the whole body, which is precisely the bug `.count` hit in ADR-0039's wave.

### 6. Comparison and equality, but no arithmetic

`==` and `!=` work on two values of the same enum type. `<` and the rest do **not**, and
neither does `+`.

Ordering is refused because an enum's members are named alternatives, not magnitudes: with
auto-numbering, `Colour.RED < Colour.GREEN` would be true by an accident of declaration
order, which is a fact about the source file rather than about colours. A program that wants
the number writes `cast(s64, c)` and gets ordering on an `s64`, where it means something.

Arithmetic is refused because `Colour.RED + 1` has no member to name and no meaning as a
`Colour`. This is Jai's position too.

## Consequences

- **No new layout logic.** An enum's layout is its backing type's, so `layout_of` gains one
  arm that delegates. Both back ends see an integer-shaped scalar and need no float-style
  representation change.
- **`Colour.RED` folds to a constant at MIR**, so an enum costs nothing at run time: the
  member is an interned `s64` value with the enum's type, and comparison is an integer
  compare. That is checked by the differential harness rather than assumed.
- **`describe` and `type_name` must render the enum's *name***, not `enum{DeclId}` — reading
  `FileSignatures::type_name` exactly as the struct case does (ADR-0028 §1's one renderer).
- **A member that is not a member is a new diagnostic**, and it should suggest a near name:
  the candidate set is the enum's own members, which `jr-sema` has and nothing else does
  (ADR-0031 §1). This reuses `no_such_field`'s machinery rather than adding a second guesser.
- **The parser's "arrives in wave W1" refusal shrinks again.** After this wave it covers
  `union`, `xx` and `null` — and `enum_flags` has no keyword of its own, so nothing claims it
  is available.
