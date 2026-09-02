# ADR-0153: The compiler message loop — a metaprogram that iterates rather than unrolls

- **Status:** Accepted
- **Date:** 2026-09-01
- **Deciders:** dboll
- **W6's headline claim, and PLAN §8.2's remaining item.** Built directly on ADR-0152's static-data
  mechanism, which §8.2 predicted would be what unblocked it.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

### The limit ADR-0100 stated honestly, and what it was waiting for

`noted_count("x")` and `noted_name("x", i)` let a metaprogram *unroll*: ask how many declarations carry
`@x`, then ask for name 0, name 1, and so on — up to a bound **written into the script**. ADR-0100 §2 did
not present that as a spelling limitation. It recorded it as the boundary of folding itself:

> a fold is answered while *checking*, and a `for` variable does not exist then. Genuine loop-driven
> iteration needs a compiler-emitted table.

And it named the mechanism: the one `Type_Info`'s variable-length field list had been deferred for since
ADR-0078. ADR-0152 built it. So this wave is the collection of a debt, not a new design — which is why it
is small.

## Decision

### 1. `noted_declarations("x")` folds to a `[]Declaration` table

A view over a table the compiler emitted, in **declaration order** — the one order a reader can predict
from the source, and the same order `noted_count`/`noted_name` already use.

```jai
health :: () -> s64 @route "/health" { … }
users  :: () -> s64 @route "/users"  { … }

routes := noted_declarations("route");
for i: 0..routes.count - 1  { … routes[i].name … routes[i].note_value … }
```

The bound is `routes.count`, not a literal. That is the whole difference: adding a fourth `@route`
declaration changes the program's behaviour without editing the loop.

`Declaration { name: string; note_value: string }`. The note's value is `""` for a bare note like
`@deprecated` — a real answer rather than an absent marker, so a script asks whether a payload was written
by comparing the count against 0 and needs no second field.

**Rejected: a genuine poll** — `compiler_wait_for_message()`, the shape Jai uses. It implies the
metaprogram runs *concurrently with* compilation, which needs an execution model this compiler does not
have and PLAN §8.3 has now split into its own wave (W11). Worse, a poll's observable behaviour would
depend on compilation *order*, which salsa's re-execution makes unstable by design: the same program
would answer differently between runs depending on what was memoised. A table is a value; a value is
reproducible.

**Rejected: putting a `Type_Info` or a procedure pointer in `Declaration`.** Both would make this the
*inspection* half of a loop that also wants to *change* what it inspects, and §2 keeps those apart
deliberately.

### 2. Inspection and generation stay separate, and that is the design

There are now two halves, and they do not overlap:

- **`noted_insert("x", template)`** (ADR-0101) *generates*: it emits a template once per noted
  declaration, at compile time, and the result is code.
- **`noted_declarations("x")`** *inspects*: it hands a running program a table of what was declared.

Keeping them apart is what makes each simple. A single mechanism that both iterated and declared would
need to answer when a declaration added by one iteration becomes visible to the next — which is
ADR-0120's expansion fixed point, in a place where it has no bound. Generation already has its answer
(a fold that loops internally); inspection needs no such rule, because a table is built once from a tree
that is no longer changing.

**This is also why the table carries no procedure pointer.** A script that could *call* what it found
would be doing at run time what `noted_insert` does at compile time, with none of the guarantees — and it
would make `Declaration` depend on the declaration's signature, which differs per entry.

### 3. The library contract mechanism gets its third client

`DECLARATION_FIELDS` sits beside `TYPE_INFO_FIELDS` and `ANY_FIELDS`, checked the same way, so editing
`Declaration` in `modules/Basic` produces a diagnostic (E0265) rather than a wrong read. Three clients is
the point at which that mechanism stops being a one-off: it was written for `Type_Info`, reused for `Any`,
and now costs two lines for a third type.

## Consequences

- **W6's headline claim in §2.1 is met.** A metaprogram can find declarations by note and iterate them in
  an ordinary loop. What remains of W6 is build scripts, plugin hooks and workspaces — all of which sit on
  top of this rather than beside it.
- **`noted_count` and `noted_name` stay**, and are not deprecated. They fold to *constants*, which a
  `#run` or an `#insert` can use where a run-time view cannot reach. The two answer different questions
  and ADR-0100 §2's honest limit is now a documented trade rather than a gap.
- **One corpus file, `valid/122`, exits 21 in all three engines.** Its load-bearing term is that
  `@deprecated` is *absent* from the `@route` table: a query returning every declaration regardless of
  note would still pass a count-only test if the numbers happened to line up.
- **A test-harness lesson worth recording**, because it cost real time here: `cmd | head -1; echo $?`
  reports *`head`'s* status, not the command's. Two apparent VM divergences in this wave were that bug in
  the shell, and both looked exactly like a real engine disagreement.
