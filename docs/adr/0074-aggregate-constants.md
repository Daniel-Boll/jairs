# ADR-0074: An aggregate compile-time value, interned field-wise

- **Status:** Accepted
- **Date:** 2026-08-03
- **Deciders:** dboll
- **W4 sub-wave 6.** ADR-0073 §0 found that `type_info()` and `Any` are blocked not on the `#insert`
  cycle but on a representation: `jr-pool` has no aggregate *value*, so a `#run` returning a struct is
  refused. This sub-wave lifts that refusal on its own terms, because it is also a standing gap users meet
  without any RTTI in sight.

## Context

### 0. What running found

```
P :: struct { x: s64; y: s64; }
mk :: () -> P    { p: P; p.x = 7; return p; }
V :: #run mk();      // error[E0230]: a compile-time struct value arrives with a later wave
```

Four facts, each checked rather than assumed — the habit that has now caught a false schedule four waves
running (ADR-0067 §0, ADR-0070 §0, ADR-0072 §5, ADR-0073 §0):

- **The gap is in `jr-pool` alone.** `Item`'s value variants are `VoidValue`, `BoolValue`, `IntValue`,
  `FloatValue`, `StrValue`, `TypeValue`, `ProcValue` and `ForeignLibraryValue` — and that is the whole
  list. There is nothing to intern an aggregate as, which is exactly what `jr-db`'s `reduce` says when it
  refuses: "a struct computed at compile time would need the pool to intern an aggregate value, which
  ADR-0015's `Item` has no variant for".
- **Both engines are already able to *hold* one.** The VM has `Value::Aggregate(Vec<u8>)` and builds one
  for a string today; `jr-codegen-clif` already emits static data through `DataDescription` and
  `define_data`, which is how a string literal reaches the binary. So this is a representation decision,
  not a back-end one.
- **The refusal covers *arrays* too, and its message does not say so.** `#run` returning a `[2]s64` reports
  the same "a compile-time **struct** value" — true of the code path, misleading about the language. A
  reader with an array constant is told about structs.
- **`string` already works** (`V :: #run mk();` where `mk` returns a `string` checks cleanly), because
  `reduce` special-cases `Value::Aggregate` when the type is `PoolId::STRING` and interns the *text*. That
  is the shape §1 generalises: a string constant is already an aggregate interned by its **contents**
  rather than by its bytes.

### The pool is target-independent, and that is the whole design constraint

`layout_of(pool, target, ty)` takes a `TargetLayout`; the `Pool` itself holds **none**. Field offsets,
padding and pointer width are therefore *not* pool facts — they are answers the pool computes when asked
about a target. Every target in the slice is `LP64`, which is precisely why this has never bitten: it will
the day a second one exists.

So the question is not "how do we store the bytes". It is whether an aggregate constant should be bytes at
all.

## Decision

### 1. `Item::AggregateValue { ty, elements }` — the field values, in order, not a byte image

An aggregate constant interns as the list of its **element values**, each itself a `PoolId`, plus the
aggregate's own type. `V :: #run mk();` above interns
`AggregateValue { ty: P, elements: [IntValue(7), IntValue(0)] }`.

**The `ty` field is not decoration**, and writing this section without it was a mistake the compiler caught
within minutes: `Pool::type_of` is *total*, so it must answer for an aggregate — and two distinct struct
types with identically-typed fields produce the same element list, so an elements-only key would intern
them to **one id**. A constant of one type would then silently stand in for the other. This is the same
reason `IntValue` and `ProcValue` carry a `ty` ("one shape can have many"), reached by the same route:
an exhaustive match refusing to compile.

**Rejected: interning the byte image the VM produced.** It is the obvious answer — `reduce` already *has*
the bytes, so this would be a one-line variant — and it was rejected because those bytes are
**target-specific**. The VM writes them with `write_le` at offsets from `layout_of(pool, target, ty)`, so
the image encodes one target's padding and pointer width. Interning it would put a target fact inside the
target-independent pool, and the pool is *shared* — `ADR-0018 §2` put layout there precisely so both
engines ask one question and get one answer. A cross-compile would then either reuse an image built for the
host or need a second pool, and the failure would be silent: a struct whose padding differs by target reads
back as plausible wrong values rather than as an error. Field-wise interning has no target in it, and
`field_offset(pool, target, …)` already exists to turn it into bytes *per target*, at the point that knows
which target is meant.

**Rejected: a side table keyed by declaration**, the way `Pool::struct_fields` holds field *types*. That
works for a type, whose fields are a property of the declaration, and not for a value: two constants of the
same struct type have different contents, so the key would have to be the constant rather than the type —
which is what `intern` already does. A second keying mechanism for the same job is a second thing to keep
correct (the argument `ImportedValues` made for reusing `ImportedProcs`' key shape, ADR-0055 §1).

**Why `Vec<PoolId>` rather than a nested enum.** An element may itself be an aggregate — `[2]P` is an array
of structs — and a `PoolId` already expresses that, because interning is recursive by construction. A
bespoke tree would re-derive what the pool is.

### 2. It covers **struct and array**; `string` keeps its own variant

`AggregateValue` is produced for a struct, a union, a variant and a fixed array — every shape whose runtime
form is several fields at offsets. `string` continues to intern as `StrValue`, and that is not an
inconsistency: a string's *contents* are its identity, its runtime form is a `{data, count}` pair pointing
at bytes the back end emits separately, and it already round-trips. Folding it into `AggregateValue` would
mean interning a pointer, which has no compile-time value at all.

**A union's constant is its bytes-as-written, and is deliberately out of scope** (§4): a union is untagged
(ADR-0045 §1), so "which field is valid" is unanswerable, and an aggregate value would have to pick one.

### 3. The refusal that remains says which shape it means

`reduce`'s message becomes specific: an aggregate that *is* representable is interned, and one that is not
— a union, or an element whose own value cannot be interned — is refused naming the shape. The current
message calls every case "a struct value", which is wrong for an array and unhelpful for both.

### 4. What is deliberately absent

- **A union constant** (§2): untagged storage makes the "which field" question unanswerable, and picking one
  silently is the reinterpretation trap ADR-0045 §1 accepted only for *runtime* reads a programmer wrote.
- **A struct or array *literal*** — `P.{1, 2}`, `[1, 2, 3]` — which is a **syntax** question ADR-0039 §6
  already defers. This wave gives a `#run` result somewhere to live; it does not add a way to write one
  directly. Worth stating because the two are easy to conflate: after this, `V :: #run mk();` works and
  `V :: P.{1, 2};` still does not parse.
- **`type_info()` and `Any`** (ADR-0073 §0), which this unblocks rather than delivers. The describing
  struct, its schema in `modules/Basic`, and `Any`'s `{type, pointer}` pair are each their own decision.
- **A constant aggregate as a *pattern* or a `switch` case.** ADR-0067 §2 makes a case a value, and
  comparing two aggregates needs a structural equality this project has not decided on (ADR-0071 §5's
  question, one type down).

## Consequences

- **`Item` gains its first *recursive* value variant.** Every existing value is a leaf; an
  `AggregateValue` holds `PoolId`s that may themselves be aggregates. Anything walking `Item` exhaustively
  must decide what to do with a nested one — which is the point of an exhaustive match, and where ADR-0068
  found two wrong answers by adding a variant and letting the compiler find the sites.
- **`layout_of` needs no change**, because an aggregate *value*'s layout is its *type*'s layout, which
  already works. That is the payoff of not interning bytes.
- **Both engines need one new materialisation each**, and they are the same shape as the string ones they
  sit beside: the VM builds a `Value::Aggregate` by writing each element at `field_offset`, and
  `jr-codegen-clif` emits a `DataDescription` the same way it does for a string's bytes. Two
  materialisations from one interned value is exactly ADR-0019's arrangement — shared representation, two
  independent lowerings, and a differential test that says they agree.
- **E0230's message stops being wrong about arrays** (§3).
- **Reading a field of an aggregate constant spills the whole constant, once per read.** A field projection
  needs an address and a constant is an operand, so `Res::Item` gained a place by storing the constant into
  a fresh slot — which means `POINT.x + POINT.y` materialises `POINT` **twice**, as the corpus snapshot
  plainly shows (`store s0 <- {3_s64, 4_s64}` then `store s1 <- …`). Correct but wasteful, and named here
  rather than left for someone to discover: caching the slot per `(item, body)` is the obvious fix and it is
  deliberately not in this wave, because a cache keyed on the wrong thing is a wrong *address* rather than a
  slow program. Const-prop already folds the reads themselves, so the cost is a stack slot and a store, not
  a load.
- **This is the last thing between W4 and RTTI**, which is why it ships alone: after it, `type_info()` is a
  schema decision rather than a representation one.
