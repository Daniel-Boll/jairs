# ADR-0100: `noted_count` and `noted_name` query a file's noted declarations — and the honest limit of folding

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W6 sub-wave 3.** ADR-0099 gave notes a *reader* — a script could ask a **named** declaration about its
  notes. This gives them a *query*: a script can ask the **file** which declarations carry a note, without
  knowing their names. It also states precisely what folding cannot do, and names the wave that fixes it.

## Context

ADR-0099's closing consequence was exact about what was missing: *a script must name each declaration, so it
cannot ask "every declaration tagged `@X`"*. That is the query, and it is the half a build script actually
needs — a script written against a note it will meet in code it has not seen.

**The blocker has to be stated before the fork, because it decides it.** A folding intrinsic is answered at
*check* time, so every argument must be readable then. A `for` variable is not: it exists only at run time. So

```jai
for i: 0..noted_count("serialise") {  name := noted_name("serialise", i);  … }
```

**cannot** be made to work by folding, whatever it is called. Genuine loop-driven iteration needs the query to
lower to *real code* reading a **compiler-emitted table** — static data the back end emits and the VM can also
read. That mechanism does not exist in Jairs, and it is not a small addition: it is the same one `Type_Info`'s
variable-length field list has been deferred for since **ADR-0078**, and it is owed its own wave.

## Decision

### 1. `noted_count(note)` and `noted_name(note, i)`, both folded, both taking literals

```jai
serialise_a :: (x: s64) -> s64 @serialise { … }
plain       :: (x: s64) -> s64             { … }
serialise_b :: (x: s64) -> s64 @serialise { … }

n     := noted_count("serialise");     // 2
first := noted_name("serialise", 0);   // "serialise_a"
past  := noted_name("serialise", 9);   // ""
```

They join `has_note`/`note_value` in the same `Intrinsic` enum, the same resolver exemption, and the same
`folded_calls` channel — the value is interned during checking and reaches `jr-mir` through `set_run`, so
nothing new was plumbed (ADR-0099 §2).

**Declaration order, not any other.** It is the one order a reader can predict from the source. Sorting by
name would make *inserting* a declaration renumber every index a script had already unrolled; a hash order
would make one program answer differently between runs, which is the property a compiler must never have.

**The index must be an integer literal** (E0277, the reader's code — this is the same "unaskable at check
time" family). And **an out-of-range index answers `""` rather than being refused**, because unrolling to a
fixed bound is the intended use and its tail has to be quiet: a script written for "up to four serialisable
types" must compile in a file with two.

### 2. What this deliberately does not do, and the wave that will

After this, notes can be **counted** and **named**. They cannot be **looped over**. That is the boundary of
folding, not a gap in the spelling, and it is worth stating in those words rather than implying that the
message loop is nearly done.

Four options were weighed, and the two rejected ones are rejected *for their cost*, not their value:

- **The static-data table now** — the honest full answer and the right eventual one. It needs a declared
  static-data mechanism, both back ends emitting it, the VM reading it, and a decision about who owns the
  memory. Doing that inside a notes sub-wave would bury an architectural decision inside a feature, which is
  the mistake ADR-0086 was written to avoid for the pool.
- **Returning the names as one space-separated string, spliced with `#insert`** — needs no table and is
  genuinely useful for code generation, but splitting text needs `String`, which is W7. It would ship a query
  whose only consumer does not exist yet: ADR-0080 §3's rule.
- **A `#for_each_note` directive expanding at lowering** — the most powerful and the least honest. It would be
  a second, hidden iteration construct with its own scoping rules, in a language that already has `for`. A
  metaprogram facility should not need a parallel `for`.

### 3. The query sees **this file only**

`noted_declarations` walks `FileHir::items`, so a note in an imported module is invisible. That is the same
boundary a cross-file `#expand` splice has (E0272, ADR-0091 §3) and a cross-file instantiation has (ADR-0082
§5), and for the same reason: reaching across files during checking is what makes sema and the module loader
mutually recursive, PLAN §5's named top risk. A build script is a *file*, so the boundary is where a build
script would want it — but this is a real limit and the corpus file says so rather than leaving it to be
discovered.

## Consequences

- **A metaprogram can now act on declarations it was not written knowing about**, within one file and up to a
  bound it unrolls by hand. `valid/081` counts two `@serialise` procedures, names both, and reads past the end
  — and its exit code depends on all three answers, so a wrong one is visible to the differential.
- **No new diagnostic code.** Both refusals are E0277's family — a name or an index that is not readable at
  check time — and reusing it is right for the reason ADR-0099 gave for covering two refusals with one code:
  they are one mechanism's ways of being unaskable, and a reader who hits either needs the same page.
- **The message loop's remaining scope is now purely the iteration mechanics**, which makes it a wave about
  *static data* rather than a wave about notes. That is a better-shaped wave, and getting here is what the
  data-then-reader-then-query ordering bought.
- **Teeth-checked**: making `noted_declarations` return an empty list moves `valid/081`'s exit from 211 to 255,
  so every answer is load-bearing in both engines.
