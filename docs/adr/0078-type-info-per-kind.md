# ADR-0078: `Type_Info` gains the fixed-size per-kind facts (`count`, `element`), amending ADR-0075 §3

- **Status:** Accepted
- **Date:** 2026-08-03
- **Deciders:** dboll
- **W4 sub-wave 9.** ADR-0075 §3 shipped `Type_Info` with `kind`, `name`, `size`, `alignment` and
  deferred "per-kind detail — a struct's field list, an array's element type, a procedure's signature"
  as "a variable-length member … a memory-ownership decision". This adds the part of that which is
  **not** variable-length, and leaves the part that is still deferred.

## Context

### 0. What the deferral actually covered

ADR-0075 §3 deferred per-kind detail as one thing. It is two:

- **Variable-length members** — a struct's *field list*, a procedure's *parameter list*. These need a
  view or a pointer-and-count, and the count is not the problem: the *elements* are, because a
  `Type_Info_Field[]` has to live somewhere with the program's lifetime. That is the memory-ownership
  decision (static data the back end emits, versus a comptime-built table), and it stays deferred.
- **Fixed-size facts** — a struct's field **count**, an array's **length**, an array's or pointer's
  **element/pointee type**. Each is a single number: a count is an `s64`, and an element type is a pool
  id, which is an `s64` (ADR-0077 made `Type_Info.id` exactly this). None needs anywhere to live; each is
  a field the builder fills from the pool it already reads.

The deferral bundled the two because "per-kind detail" sounded like one feature. Separating them is the
whole of this ADR: the fixed-size facts ship now, and the deferral shrinks to only the list.

### The facts are all in the pool already

`Pool::struct_fields(decl)` gives a struct's field slice (its `.len()` is the count); `Item::ArrayType`
carries `{elem, len}`; `Item::PointerType` carries its pointee. `type_info`'s builder already reads all
three to compute `kind` and `size`. So this adds no query, no new pool entry, and no engine change — the
same "needs no representation" property ADR-0077's `id` had.

## Decision

### 1. `Type_Info` gains `count: s64` and `element: s64`

```
Type_Info :: struct {
    id: s64;
    kind: Type_Info_Kind;
    name: string;
    size: s64;
    alignment: s64;
    count: s64;      // NEW — a struct's field count, or an array's length; 0 otherwise
    element: s64;    // NEW — an array's element or a pointer's pointee, as a type id; 0 otherwise
}
```

- **`count`** is a struct's number of fields, or an array's length. Both are "how many", so one field
  serves both — a reader switches on `kind` to know which it means, exactly as it already must to read
  `element`. 0 for every other kind, which is a real answer (a scalar has no count) rather than a sentinel
  standing in for "unknown".
- **`element`** is the type id of an array's element or a pointer's pointee — the same kind of id
  `Type_Info.id` is (ADR-0077), so a program compares it against another type's `id` to ask "is this a
  `[]s64`?". 0 for a kind with no element.

**Why an id, not a `*Type_Info`.** A `*Type_Info` for the element would need that element's `Type_Info`
to exist and live somewhere — the static-data decision §0 keeps deferred. An id is a fixed `s64`, needs
nothing built, and is the identity a program actually compares. The cost is that a program cannot yet
*follow* `element` to the element's own `Type_Info` — there is no `type_info_of_id(n)` — and that is
deliberately part of what stays owed (§4): the id says *which* type without materialising it.

**Why flat fields rather than a `union` or a per-kind struct.** A `union` reintroduces the "which field
is valid" question `Any` and ADR-0045 exist around — a reader would need `kind` to know which arm is live,
which is exactly what flat fields plus `kind` already give, without the untagged hazard. A per-kind struct
(`Struct_Info`, `Array_Info`) multiplies the compiler's validated `Basic` dependency by the number of
kinds, each its own E0265 surface. Flat optional fields extend the schema by *adding fields*, which
ADR-0075 §3 already declared safe for a reader that names only the earlier ones, and this is the second
exercise of that after ADR-0077.

### 2. This amends ADR-0075 §3, and the validation grows with it

`TYPE_INFO_FIELDS` gains `("count", s64)` and `("element", s64)` at the end, so a `Basic` whose
`Type_Info` lacks them — or misorders them — is E0265, as any other shape mismatch is. Appending rather
than inserting keeps the existing fields' offsets, so ADR-0077's `id`-first layout and the four original
fields are undisturbed; only the two new reads are added.

### 3. Filled for exactly the kinds that have the fact

The builder sets `count` for a **struct** (field count) and an **array** (length), and `element` for an
**array** (its element) and a **pointer** (its pointee). Every other kind leaves both 0. A **union** and a
**variant** have a field count too, and it is set for them as well, because it is the same
`struct_fields(decl).len()` and no more speculative than a struct's. A **procedure**'s parameter count is
*not* set, because a proc type's parameters are a `Vec<PoolId>` that is conceptually the variable-length
list §0 defers — setting only its length would imply the elements are reachable when they are not.

### 4. What is still deferred

- **The variable-length lists** (§0): a struct's field list, a procedure's signature. Still the
  memory-ownership decision, unchanged.
- **Following `element` to a `Type_Info`.** `element` is an id; there is no `type_info_by_id` that turns
  it back into a `Type_Info`, because that needs every reachable type's `Type_Info` to be materialised —
  the static-data decision again. A program compares ids; it cannot yet walk them.
- **`type_info` of a structural type argument** — `type_info([4]s64)` still does not parse, because the
  argument grammar is expression-only and a structural type alias (`Arr :: [4]s64;`) is ADR-0071 §5's
  deferred fixpoint. `type_info(Arr)` will work the day that alias does; this ADR does not change the
  grammar.

## Consequences

- **`Type_Info` is seven fields**, and `type_info`'s builder gains two elements — both `s64`, interned
  like `id` and `size`. No query, no pool entry, no engine change: the same payoff ADR-0077 had.
- **A third exercise of "the schema extends by adding fields"** (after ADR-0077's `id`). The validation
  appending rather than inserting is what keeps the earlier fields' offsets, so `valid/063` and `valid/064`
  read the same fields at the same places; only their MIR snapshots grow two elements.
- **RTTI can now answer "how big is this array" and "what does this point at"** without the list. That is
  most of what a `print_any`-style routine needs for a scalar, a pointer or an array — the field list is
  what a struct printer needs, and that is the honest remaining gap.
- **The deferral is now precise.** ADR-0075 §3 deferred "per-kind detail"; after this, what is deferred is
  exactly "the variable-length list and following an id back to a `Type_Info`", which is a memory-ownership
  decision rather than a grab-bag.
