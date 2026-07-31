# ADR-0055: An imported constant's value crosses the boundary the way a callee does

- **Status:** Accepted
- **Date:** 2026-07-30
- **Deciders:** dboll
- **Follows ADR-0018 §5**, which made a cross-file *callee* representable by having `jr-db` resolve it
  from the other file's signatures. This does the same for a *value*, and §1 argues that the shape
  ADR-0018 §5 chose is the one to copy rather than a new one to invent.
- **Retires the E0245 warning on six corpus files.**

## Context

Reading an imported constant does not work. `jr-mir` refuses the body and `jr-db` warns E0245 — "the
compiler could not lower the body of `main`" — and six `imports/valid/` files carry that warning as an
honest record. `PLAN.md` §7 has listed it for eleven waves.

It is also the same gap as two others §7 records separately, which is why this wave is worth doing
before W3 rather than after: a named argument on a cross-file call (ADR-0053) and `using` on an
imported struct (ADR-0050 §5) both fail because **the importing file has the other file's
`ItemScope` and nothing else** — a name-to-id map with no values, no signatures and no field lists.

Six facts were established by reading the code, and three shaped the decisions.

- **`ImportedProcs` is `FxHashMap<(ItemId, Symbol), ProcRef>`**, filled by `jr-db` from the other
  file's signatures and handed to `lower_file`. **This is the fact that decides §1**: the mechanism
  for "resolve a cross-file thing in `jr-db`, hand `jr-mir` the answer" already exists, is already
  differentially tested, and needs copying rather than designing.
- **`ConstValues` is `items: FxHashMap<ItemId, PoolId>`** — an `ItemId` keyed to an interned value.
  An imported constant's `ItemId` belongs to the *other* file, so it cannot go in that map without
  making `ItemId` ambiguous across files. §2 is about that.
- **`file_consts` already runs per file** and is a salsa query, so the other file's constants are
  computable by calling it. The question is only whether doing so cycles, which §3 answers.
- **`scan` refuses an imported name that is not a callee** with "an imported name has no value until
  jr-vm", and that string is the whole of the current behaviour.
- **`Res::Imported(ItemId, Symbol)`** carries the *importing* file's `#import` item and the name in
  the other scope — exactly the key `ImportedProcs` uses. So no resolution change is needed.
- **A constant's value is a `PoolId`**, and the pool is shared across files (ADR-0018 §2). So a value
  computed while checking one file is meaningful in another with no translation at all.

## Decision

### 1. `ImportedValues`, keyed exactly as `ImportedProcs` is

```rust
pub struct ImportedValues {
    by_name: FxHashMap<(ItemId, Symbol), PoolId>,
}
```

`jr-db` fills it by calling `file_consts` on each imported module and copying out the values of the
names this file actually refers to. `jr-mir` reads it where it currently refuses.

**The same shape as `ImportedProcs`, deliberately.** ADR-0018 §5 established the pattern — resolve
across files in `jr-db`, hand `jr-mir` a flat map keyed on the import site — and a second mechanism
for the same job would be a second thing to keep correct. The two maps are built side by side in the
same function, from the same `(ItemId, Symbol)` pairs the resolve map yields.

**Rejected: giving `ConstValues` a file-qualified key.** Making it
`FxHashMap<(FileId, ItemId), PoolId>` would let one map hold local and imported constants together.
Rejected because `ConstValues` is *produced* by const-eval for one file and consumed by that file's
lowering; widening its key would make every producer supply a `FileId` it does not need, to serve a
consumer that is better served by a second map. `ImportedProcs` made the same call.

**Rejected: resolving it in `jr-mir`.** `jr-mir` would need to load the other file, which means a
module search path and a salsa database — neither of which it has, and ADR-0017 §3's "no cross-body
reads" is the rule that keeps it that way.

### 2. Only a **value**, not a body

An imported constant whose value is a literal, a `#run` result, or any expression const-eval can
fold, crosses the boundary. An imported *procedure* still crosses as a `ProcRef` (ADR-0018 §5), and
an imported struct's field list still does not cross at all (ADR-0050 §5, unchanged here).

That is a narrower fix than "make cross-file information available", and the narrowness is the point:
a `PoolId` is already meaningful in any file because the pool is shared (ADR-0018 §2), so a value
needs no translation. A field list would need `jr-sema`'s view of the other file, which is a larger
dependency and a separate decision.

**A constant const-eval could not fold does not become readable.** `MOD_CONST :: some_unfoldable()`
is E0230 in its own file already, and this wave does not change that — the importing file simply finds
no value and refuses as before, which is the existing behaviour and the right one.

### 3. Why this does not cycle

`file_consts(B)` depends on `file_signatures(B)` and `file_hir(B)`. It does **not** depend on
`checked(B)` — ADR-0018 §3 put const-eval downstream of signatures precisely so it would not — and it
does not depend on anything in the importing file `A`.

So `optimized_file_mir(A)` calling `file_consts(B)` adds an edge from A's lowering to B's const-eval,
and there is no path back: B's const-eval never asks about A. Two modules importing each other are
fine for the same reason `file_exports` is (ADR-0054 §3), and for the reason ADR-0014 §4 makes cycles
legal at all.

**The one thing to be careful of** is that `file_consts` takes `search_paths`, so the call must pass
the *same* paths the importing file was resolved with. Passing different ones would make a module's
constants depend on who imported it, which is the action-at-a-distance ADR-0014 §3 objects to
throughout.

### 4. What this unblocks, and what it does not

**Unblocks:** reading an imported constant, which retires E0245 on six corpus files.

**Does not unblock**, and each for its own recorded reason:

- **A named argument on a cross-file call** (ADR-0053). That needs the callee's `ProcSig` — parameter
  *names* and defaults — not a value. The same `jr-db`-resolves-it shape applies and it is a separate
  map; recorded as owed rather than bundled, because a signature is a different thing from a value and
  ADR-0053 §1's rule about where names live would have to be restated.
- **`using` on an imported struct** (ADR-0050 §5). That needs the other file's *field list*, which
  lives in `jr-pool` keyed on a `DeclId` — so it is closer than it looks, and still a different map.
- **An operator overload in a `#run`** (ADR-0048). That is the opposite direction: const-eval running
  before `checked`, not information failing to cross a file boundary.

§7 has grouped all four as "one fix serves all three". **That was wrong**, and this ADR is where the
grouping is corrected: they share a *shape* — `jr-db` resolves, `jr-mir` reads a flat map — but each
needs a different map filled from a different query, and two of them are not about imports at all.

## Consequences

- **`lower_file` and `lower_body` take one more parameter**, which is a compile error at every call
  site including two test harnesses. ADR-0053's lesson applies: a harness passing an empty map would
  make the tests pass while the feature did nothing, so each must pass the real one.
- **Six corpus files stop warning.** Their E0245 comments become wrong and must be rewritten — the
  warning was load-bearing documentation, and leaving the prose after removing the warning is exactly
  the rot `AGENTS.md` names.
- **No new diagnostic code.** A constant that still cannot be read is the existing E0245, and one that
  never had a value is the existing E0230. **E0254 remains the first free code.**
- **`jr-sema`, `jr-hir`, both parsers and `jr-fmt` are unchanged.** The seventh consecutive wave in
  which the formatter had to be touched ends here, because this wave adds no syntax at all.
- **A corpus program must read an imported constant *and use the value***, not merely resolve the
  name: a fix that produced the wrong number would satisfy any test that only checked the warning
  was gone.
