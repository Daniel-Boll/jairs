# ADR-0171: Type and struct-layout DIEs — and the reference that makes them exist

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **W12's second item**, for the LLVM back end. Cranelift's hand-written `.debug_info` is the remaining half
  and is named as owed rather than claimed.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Decision

### 1. A `DIType` per `PoolId`, and the pool's own deduplication is inherited

`debug_type` maps a pool item to DWARF: `bool` and the integers and floats to `DW_TAG_base_type` with the right
encoding, a pointer to `DW_TAG_pointer_type`, a struct to `DW_TAG_structure_type` with a `DW_TAG_member` per
field at the offset `jr_pool::field_offset` computes.

**That last point is what makes the DIE trustworthy**: the offsets come from the same function both engines use
to *compile* a field access, so a debugger cannot disagree with the code about where `p.y` is. A DIE with
hand-computed offsets would be a second layout implementation, which is the thing ADR-0009 exists to prevent.

The cache is keyed by `PoolId`, which the pool already deduplicated **by structure** — so two identical struct
declarations are one DIE, and a debugger never shows the same type twice.

**A `None` propagates**: a struct with one undescribable field gets no DIE. A struct DIE listing *some* of its
members would show a type whose fields do not add up to its size, which is worse than showing nothing.

**The recursion terminates because a pointer stops the walk.** `Node :: struct { next: *Node; }` would
otherwise recurse forever. The pointee's DIE is used only if already cached; otherwise the pointer is described
as opaque, which costs `next.next.value` in a debugger and keeps the compiler from hanging. Stated rather than
assumed, because "why does this terminate" is the first question a reader has.

**Deliberately undescribed, each for its own reason** rather than as one bucket: `void` has no DIE *by
definition* (DWARF spells a void return as an absent type); `string`, views, arrays, unions and variants all
have real DWARF spellings but each needs a decision about *naming* — a `[]s64` has no user-written name — and a
wave that guessed at four at once would be four guesses; a procedure type wants a `DW_TAG_subroutine_type`,
which is work the subprogram already does and is worth sharing rather than duplicating.

### 2. `TrapLocations` became `SourceInfo`, because it now resolves names too

A struct's members need field *names*, and a back end has no interner — the same wall `FileInput::names`
already hit, where the driver resolves procedure names because "turning a `Symbol` into text needs the
interner".

So the trait gained `symbol(Symbol) -> Option<String>` and **was renamed**. A trait called `TrapLocations` with
a `symbol()` method teaches a reader the wrong thing about where the next such lookup belongs.

**Rejected: extending it under the old name.** Cheaper, and it leaves a name that lies. **Rejected: a second
trait.** Two driver-supplied lookups threaded through the same call sites, for no gain — the back end asks one
thing for one reason: *the driver can see the front end and I cannot.*

Clean cutover: every reference renamed, no alias.

### 3. Parameters are declared, and that is what makes a type DIE exist at all

**The wave's real finding.** The struct mapping was written, it was correct, and `dwarfdump` showed **base types
and no struct**. That looks exactly like the mapping being broken.

It was not. **LLVM prunes a type nothing declares.** A `DISubroutineType` listing a struct is not a
declaration — it is a signature, and signatures are metadata LLVM will drop. What retains a type is a variable
*of* that type, so each parameter now gets a `DILocalVariable` via `create_parameter_variable`.

So W12's second and third items are **coupled**, and the plan had them as separate lines. A type DIE with
nothing declaring it is not emitted, and a parameter declared without a type DIE has nothing to point at.

The parameter variables earn their place independently: `lldb` can print `p.x` at a breakpoint.

**`ProcDecl` gained `param_names: Vec<Symbol>` — interned, not resolved**, unlike `name` immediately above it,
which looks inconsistent and is the newer, better shape. `name` predates `SourceInfo::symbol` and had to be
resolved by the caller because nothing could resolve it later; these can be resolved on demand, so only a back
end that emits debug info pays. And they come straight from the HIR that `FileInput` already carries, where
`name` needed a whole parallel slice from the driver.

**Holes are kept in the parameter DIE list.** A `filter_map` would silently shift every later parameter's name
onto the wrong type — which would produce debug info that is confidently wrong rather than absent, this
project's least favourite failure mode.

### 4. The struct DIE is anonymous, and that is a recorded gap

The pool does not record a struct's *declared* name: `Item::StructType` carries a `DeclId`, and the name lives
on the HIR item that bound it, which a back end cannot see (ADR-0009).

DWARF permits an unnamed struct type and `lldb` shows it with its members, which is where the value is — a
reader wants `p.x` and its offset far more than the type's spelling.

**Rejected: faking a name from the `DeclId`.** It would print a number no reader recognises, and a plausible-looking
wrong name is worse than an honest blank.

### 5. The borrow checker enforced the right order

`self.debug` is behind a shared borrow while a DIE is built, and `debug_type` needs `&mut self` to cache — so
the builder cannot be held across the recursion. That is not the borrow checker getting in the way: **a
member's DIE must exist before the struct that lists it**, and the code now says so structurally.

### 6. The test asserts offsets, not tags

`Point { x: s64, y: s64, flag: bool }` must appear as 24 bytes with `x` at 0, `y` at 8 and `flag` at 16, with
those names.

**A test asserting only that a `DW_TAG_structure_type` exists would pass on a struct whose every member sat at
offset 0**, and on one whose members were `field0`/`field1`. Both parse perfectly and are useless — the same
argument ADR-0169 §8 made for "not every row is the same line".

## Consequences

- **A struct's layout reaches DWARF** with source field names and the offsets the compiler itself uses.
- **`TrapLocations` is now `SourceInfo`** everywhere; two implementors and both back ends changed.
- **1064 tests**, 1067 under gate 7 — the two new ones are `llvm`-gated.
- **W12's items 2 and 3 are coupled**, discovered by building rather than by planning. Parameters are declared;
  **local** variables are not, and they need a MIR-slot-to-HIR-local mapping the back end does not have.
- **Cranelift has no `.debug_info` at all** — only the line table. Its types must be written by hand with
  `gimli`, exactly as ADR-0170 predicted the split would go. Named as owed.
- **A struct's declared name is owed**, and so are views, arrays, unions and variants.
