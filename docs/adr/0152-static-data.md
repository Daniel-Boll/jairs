# ADR-0152: Compiler-emitted static data, and the field list ADR-0078 deferred

- **Status:** Accepted
- **Date:** 2026-09-01
- **Deciders:** dboll
- **PLAN §8.6 step 3, and §8.2's wave-sized decision for W6.** The mechanism ADR-0078 §3 deferred, which
  PLAN §8.2 predicted would discharge two owed things at once — it does.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

### One mechanism, owed to two things

ADR-0078 §1 gave `Type_Info` the *fixed-size* per-kind facts — `count`, `element` — and §3 deferred the
variable-length field list, because handing out a `[]Type_Info_Field` needs the compiler to emit an
array somewhere and produce a view of it. PLAN §8.2 then found W6's message loop wants the same thing: a
declaration table a metaprogram indexes. Two owed items, one mechanism.

### Why it was hard, and it is the same defect twice

ADR-0074 found that a pointer or view *inside a compile-time aggregate* interned the **evaluator's own
address** as an integer — giving 48 in one engine and a segfault in the other, with no diagnostic — and
the fix was to refuse it. So an address can never be a pool value: the pool is target-independent, and an
address is the most target-dependent thing there is.

That is why `fields` could not simply be a member of the interned `Type_Info` aggregate.

### The precedent was already in the tree

`string` literals have always worked exactly the right way: the pool interns a `StrId` (contents), each
engine emits the bytes into its own read-only region once per program, and the `{data, count}` pair is
**built at materialisation**. No address is ever interned.

So this wave is that solution one type wider, not a new idea.

## Decision

### 1. `Item::StaticArray { view, values }`, materialised as a view

The pool interns the element *values* and the `[]T` type they materialise as. Each engine emits the bytes
once per program, keyed on the table's `PoolId` — the pool already deduplicated by contents, so two
identical tables are one id and one emission.

It stores the **view** rather than the element type because `Pool::type_of` must answer with it and that
method takes `&self`; it cannot intern one on demand. The element is recoverable from the view, so
nothing is duplicated.

**Rejected: materialising an address instead of a view.** Every consumer wants a `[]T`, and a bare `*T`
would make the count a second thing to keep in step. Producing the descriptor means the count comes from
`values.len()` at materialisation and cannot disagree with the bytes.

`static_array` also interns `*elem`, which is not decoration: reading `view[i]` needs that pointer type
and both back ends look it up rather than construct one. Every other way of making a view goes through a
type annotation, which interns the pointer as a side effect of resolving `[]T` — this is the first
constructor with no annotation behind it.

### 2. The byte image is computed **once**, in `jr-pool`

`jr_pool::static_image` builds the bytes. Three engines emit these tables, and a byte image is *offsets
plus widths* — precisely the computation ADR-0018 §2 centralised in the pool so that the VM and both
back ends cannot disagree about a layout. Three implementations of "write each element at its own
offset" would be three chances to produce a differently-shaped table, and **no verifier would catch it**:
every engine would be internally consistent and one would read the wrong field.

**The engine supplies only its own addresses**, through a callback. And the callback is told *where* —
the byte offset of the `data` word it is filling — because a native engine cannot answer with a number at
all: there is no address until the linker has run, so it records a **relocation** at that offset instead.

That was got wrong first, and the corpus file caught it: both native back ends recorded patches and never
applied them, leaving a zero in the string pointer. Reading a field's name through it gave **139 where
the VM gave 121**. A three-way differential over a construct with a pointer in it is exactly the test
that finds this, and nothing else would have.

The two native engines express the address differently, and both are honest to their toolchain:

- **Cranelift** has `write_data_addr`, a relocation at an offset — a direct fit.
- **LLVM** has no post-hoc relocation API on a byte initialiser, so the global is built as a **packed
  struct of chunks**: bytes before each pointer, then the pointer as a `ptrtoint` constant expression of
  the string's own global, then the bytes after. LLVM emits the relocation itself. The chunk boundaries
  come from the same pool image walk, so the *layout* is still one computation — this back end differs
  only in how it expresses an address it cannot yet know.

**Rejected: letting each engine walk the fields itself.** Three layout opinions, invisible to every gate.

### 3. `Type_Info.fields: []Type_Info_Field`

`Type_Info_Field { name: string; ty: s64; offset: s64 }`, in declaration order, empty for every kind
without fields — a real answer rather than a sentinel, since a scalar has no fields.

`ty` is a **type id** rather than a `*Type_Info`, for the reason `element` already is (ADR-0078 §1): a
pointer to the field's own info would need that info built and stored somewhere, and an id needs nothing
built. It is the same identity `any_as` compares, so a program can ask "is this field an `s64`?" without
a name.

The offsets come from `jr_pool::field_offset` — the same fold both back ends read to compile a field
access. A reflected offset disagreeing with a compiled one would be the worst available failure: two
internally consistent halves and one wrong answer.

Sema's `TYPE_INFO_FIELDS` contract grows a `ViewOfStruct` shape check, for the reason `PointerToStruct`
exists: `Type_Info_Field`'s `PoolId` depends on its declaration site, so the compiler cannot name it in
advance.

## Consequences

- **W6's blocking decision is made and implemented.** The message loop's declaration table now has a
  mechanism waiting for it, and `Type_Info`'s field list — owed since ADR-0078 — is delivered by the same
  work, exactly as PLAN §8.2 predicted.
- **Reflection can walk a struct.** A field's name, type and offset are all readable, which is what a
  struct *printer* needs and what ADR-0078 §3 named as the reason to want this.
- **A `#run` cannot return a table.** Its bytes live in the engine that emitted them, so interning the
  descriptor would intern that engine's address — the ADR-0074 defect. Refused explicitly.
- **`Item` grew a variant**, so every exhaustive match over it grew an arm. That is the cost the
  discipline charges, and it is what made the twelve sites that had to decide visible.
- **One corpus file, `valid/121`, exits 150 in all three engines**, and the term that carries the wave is
  the name read back through the view: it is the only one that fails if a pointer is wrong rather than a
  number.
