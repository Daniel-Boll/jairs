# ADR-0109: `view(p, n)` builds a `[]T` from a pointer and a count — the library composes

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W7 sub-wave 7.** ADR-0107 closed by naming this gap: a growable array could not hand its contents to `Sort`
  or `String`. This closes it, and the result is the first time three modules cooperate on one buffer.

## Context

`Int_List` holds a `*s64` and a count. `Sort` takes a `[]s64`. Nothing could turn the first into the second,
because **a slice takes an array** (ADR-0044) — so a growable array and a sorting routine sat side by side, unable
to be combined. That is a poor advertisement for a standard library, and it is the kind of gap only writing the
library finds.

**A stale refusal, found by probing.** ADR-0044 §4 refused `view.data` because it "would hand out an unbounded
`*T` one wave after the bounds check was added, and there is no pointer arithmetic to use it with". **Both halves
have expired**: pointer arithmetic arrived in ADR-0064, and typed allocation (ADR-0106) makes a `*T` an ordinary
thing to hold. A refusal whose stated reason has expired is worth revisiting rather than inheriting — and this
project has now found three such: a scheduled dependency that did not exist, a comment claiming another path
handled a case, and now a refusal outliving its rationale.

The answer is **not** to expose `.data`, which would hand out the pointer without giving a caller what they
actually wanted. It is to add the missing *constructor*.

## Decision

### 1. `view(p, count)` — the element type comes from the pointer

`view` on a `*s64` is a `[]s64` and cannot be anything else, so **nothing is asserted**. That is the property
that made `typed` acceptable while `cast` stayed refused (ADR-0106 §1), and it is why this takes no type argument.

**The count is unchecked, and that is stated rather than hidden.** A pointer's allocation size is not tracked
anywhere — `malloc` returns a bare address and no shadow table records what was asked for — so a checked `view`
would need an allocation registry, which the native back end could not share with the VM. So `view` is in the same
honest category as `typed`: it does not make the operation *safe*, it makes it **visible and searchable**.

**Syntax (`p[0 .. n]`) is deferred deliberately.** It is prettier and a later wave should have it, but slicing is
currently defined over arrays with a *known* bound, so this would weaken that definition or add a second slicing
rule keyed on the base's type. Syntax is the expensive thing to get wrong and the cheap thing to add; an intrinsic
can be replaced by syntax without changing semantics.

**Neither engine needed a line.** Lowering emits the same three statements a slice does — zero the slot, store the
data word, store the count — so this is a second way to **produce** a view rather than a second thing to consume.
The only difference is where the parts come from.

### 2. A view with no place is spilled — another leaked gap report, fixed

`elements(*l).count` reads the count of a view a **call returned by value**, which has no address to project from.
That leaked *"this compiler has a gap — please report it"* for a program the language allows.

Fixed by giving the value a slot and projecting from that, which is exactly the move ADR-0077 made for
`type_info(s64).id`. A place is still reached for *first*, so the ordinary case pays for no slot.

That is the **sixth** leaked internal error this project has turned into working code, and the pattern across them
is now unmistakable: every one appeared the first time a *value-returning* form met a construct that had only ever
been used through a place.

### 3. `List.elements` bounds on `count`, and says what invalidates a view

`elements(a)` is `view(a.data, a.count)` — the **used prefix**. Bounding on `capacity` would hand out slots holding
whatever the allocator returned.

**A view is invalidated by anything that reallocates**, and the module says so: a `push` that grows moves the
storage, and `free_data` releases it. Nothing enforces that — there is no borrow checker, which is a design
position rather than an omission — so the documentation is the only place it can be said.

A view over an **empty** list is `view(null, 0)`, which is well-formed: a zero-count view is never indexed, and
every routine that takes one loops `while i < count`.

## Consequences

- **The library composes.** `sort_ints(elements(*l))` sorts a growable list in place — `List`, `Sort` and the
  language's own view type cooperating on one buffer, with no copy. `valid/089` runs it in both engines.
- **`type-errors/037`'s stated reason is superseded**, and the file now says so rather than repeating an argument
  that no longer holds. The refusal itself stands: `.data` is still not a field, because `view` gives a caller what
  they wanted without handing out an unbounded pointer.
- **No new diagnostic code.** `view`'s refusals reuse E0279 (the `typed`/`untyped` boundary code) and E0266 (an
  element type with no layout) — one boundary, one code, the argument ADR-0099 made for E0277.
- **Deferred with reasons:** `p[0 .. n]` syntax; a view of a *sub*range; a checked view (wants an allocation
  registry); and a `[]u8` over a `*u8`, which already works and is how a caller builds a byte view.
