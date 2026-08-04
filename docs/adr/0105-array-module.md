# ADR-0105: `Array` is a fixed-capacity array — and three refusals decided that, not effort

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W7 sub-wave 3.** The fourth module, and the first that is a *data structure* rather than operations on
  something the language already has. W7's plan names a **dynamic** array; this is not one, and the ADR's main
  job is to say exactly why in terms of decisions already made.

## Context

**Three probes, and each failure is a documented deferral rather than a surprise.** Probing before writing is
what turned "why isn't this a dynamic array" from a confession into a design input.

What works: a polymorphic struct with an **array field**, a **pointer to a struct instance** mutating it,
`malloc`/`free`, a **view of a struct's array field**.

What does not:

1. **A `malloc`'d region cannot be typed.** `cast(*s64, p)` is **E0232** (ADR-0045 §1): a general pointer cast
   makes a wrong pointee type a *silent wrong read*, so it is refused. `data: *T` is declarable, but nothing can
   produce a `*T` from an allocator that returns `*u8`. **Heap-backed storage is therefore unreachable**, and
   the fix is a **typed allocation** primitive rather than a weaker cast — a language decision, not a library's.
2. **Inference through a parameterised struct is deferred** (ADR-0085 §5): `push :: (a: *Array($T), v: T)` is
   **E0212**, since `T` is not in scope there.
3. **A parameterised struct cannot cross a module boundary at all** (**E0269**, ADR-0085 §5). This is the one
   that decided the shape, and it was found by *importing the module*: the first draft declared
   `Array :: struct($T)`, which compiled cleanly **inside** the module and failed at the importer's first
   `a: Array(s64)`. So a polymorphic struct in a module is **unusable by every importer**.

## Decision

### 1. A concrete `Int_Array` with `[16]s64` storage, and the name says so

`Int_Array :: struct { items: [16]s64; count: s64; }`, with `push`, `pop`, `get`, `set`, `clear`, `is_empty`,
`is_full`, and `CAPACITY`.

Concrete because of §3 above: a `struct($T)` here would be a module nobody could use. The name says `Int` so a
reader knows what they have rather than discovering it at the first call — and when cross-file parameterised
structs arrive it becomes `Array($T)` and the name loses its prefix.

**The capacity is in the struct**, because that is what makes two capacities two types with two layouts.
`Array(s64, 16)` is not available even in principle: a parameterised struct takes **type** arguments (ADR-0085
§3), and a `$N` on the *procedures* cannot help, because the capacity has to be part of the struct's type. So a
caller who wants another capacity declares their own struct of the same shape, and the corpus file says so.

### 2. A full array answers `false`; an out-of-range **element** answers a flag

`push` returns `false` when full rather than trapping. The difference from an out-of-range index (ADR-0003) is
*who made the mistake*: indexing past a bound the compiler knows is a **program error**, while filling a
fixed-capacity buffer is an ordinary thing a correct program does and then handles. A trap would push the
capacity check into every caller — the check `push` already performs.

`pop` and `get` return **two values** (ADR-0008) rather than a sentinel, and that is the opposite call from
`String.find`'s `-1` for a deliberate reason: **an index has values outside its domain, an element does not.**
Every `s64` is a legitimate element, so no value could mean "empty" without excluding it from the array.

**`get` and `set` bound on `count`, not `CAPACITY`.** Reading an unused slot would return the value the
declaration zeroed it to — a real number, indistinguishable from a genuine element. That is the
well-typed-placeholder failure AGENTS.md names, one level up from the compiler and in a library this time, and
bounding on `count` is what keeps `count` meaningful. Teeth-checked: changing `get`'s bound to `CAPACITY` clears
bit 8 (255 → 247).

`set` also refuses to **extend** — `set(a, a.count, v)` is `false`, not a push — because growth through
assignment would make the length depend on which indices a caller happened to touch.

### 3. Routing around the refusals was considered and rejected

A `*u8`-backed array with hand-computed byte offsets **is** expressible today, since pointer arithmetic works.
It was rejected: every read would need the element size as a literal and every write would reinterpret bytes,
which is precisely the silent wrong read E0232 exists to prevent. Routing around a deliberate refusal is bad
anywhere and worst in the **standard library**, which is where a reader looks to learn what the language means.

## Consequences

- **A useful bounded buffer exists**, and most of what a compiler's own data structures need is a bounded
  buffer. `valid/086` exits 255 across eight groups, including a fill-to-capacity loop that terminates *because*
  `push` refuses — one teeth-check made `push` always succeed and the test looped forever, which is a blunt
  demonstration that the refusal is load-bearing.
- **Three language limits are now demonstrated rather than predicted**, with a module and a corpus file as the
  evidence. That is the argument for writing the standard library in Jairs: ADR-0104 found two leaked ICEs this
  way, and this sub-wave found the *shape* of what the language cannot yet express.
- **No compiler change at all**, which is worth noting after two sub-waves that each fixed leaks: this one found
  only refusals that were already correct and already documented.
- **What unblocks the real dynamic array**, in order: **typed allocation** (§1's first refusal), then
  **cross-file parameterised structs** and **inference through them** (§1's second and third). Each is a
  language decision with its own argument, and none belongs to a library.
