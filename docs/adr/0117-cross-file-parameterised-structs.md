# ADR-0117: A parameterised struct may cross a module boundary — the importer resolves its fields

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W7 sub-wave 15.** The biggest remaining language unblocker, named by **three** library sub-waves: `Array`
  (ADR-0105 §3), `List` (ADR-0107 §1) and `Map` (ADR-0116 §1) are each concrete `Int_*` types *only* because a
  `struct($T)` in a module was unusable by every importer. This lifts that.

## Context

`Box :: struct($T) { value: T; }` works **within** a file (ADR-0085). Across a module boundary it was E0269 — "not
a parameterised struct" — because `Ctx::parameterised_struct` looks a name up in `hir.scope`, this file's own
declarations, and finds nothing for an imported one.

**Why it is not a one-line lookup change.** A parameterised struct's fields must be resolved **per instance,
under the caller's type arguments** (ADR-0085 §2, ADR-0086 §3). Its own file cannot do that: it does not know
what arguments an importer will supply, and it records its body with the variables bound to `PoolId::ERROR`
precisely because nothing concrete exists there yet. So the *importer* must resolve the fields, which means the
field **`TypeRef` tree** has to cross the boundary — and a `TypeRef` is a `TypeRefId` index into the *declaring
file's* arena, which the check phase never had for an imported file.

That is the real shape of the problem, and it is why three library sub-waves worked around it rather than through
it.

## Decision

### 1. The check phase receives the imported **HIR**, not just its signatures

`check_file` already takes `imports: &[(&str, &FileSignatures)]`. It now takes the imported `FileHir` alongside,
which `jr-db` already holds — the same values `file_signatures` is given through `ImportedFile`. So the importer
can walk an imported struct's `poly_vars` and its field `TypeRef`s in *their own* arena, and resolve each under
the arguments it is supplying.

**Passing the HIR rather than flattening the TypeRefs onto the signatures.** Flattening would mean copying a
`TypeRef` sub-tree per parameterised struct into `FileSignatures`, re-indexing every `TypeRefId` into a private
arena — a second representation of the same tree, which is a second thing to keep correct and exactly the drift
ADR-0022 §2 refuses for arithmetic. The HIR is already loaded, already correct, and already what the *signature*
phase uses for the same job.

### 2. Resolution runs in the **declaring** file's arena, with the importer's bindings

`resolve_apply` gains an imported branch: find the struct in the imported file's scope, take its `poly_vars`,
intern the instance as `DeclId::new(imported_file, sid)` — so its identity is the declaring file's, which is what
ADR-0015 §1 requires of a nominal type and what makes `Box(s64)` the same type in two importers — then resolve
its fields **against the imported file's `type_refs`** while the caller's arguments are bound.

The instance-keyed field map (ADR-0086 §2) needs no change: it keys on the instance `PoolId`, which already
carries the declaring file in its `DeclId`. That is the ADR-0086 generalisation paying out — the map was built
for exactly this and did not know it.

### 3. Staged as ADR-0086 §1 was, and the split could not be two *commits*

The plumbing was built and proven first: `check_file` and `Ctx` take the imported HIR, **nothing reads it**, and
the workspace is byte-identical — 986 tests, no moved snapshot. Then the imported branch in `resolve_apply`, the
narrowed E0269, and the corpus.

**That proof is what mattered, and it is recorded here rather than in two commits** — because a commit whose only
change is an unread field does not pass `clippy -D warnings`: `field imported_hirs is never read` is a hard error
under this project's gates. So the *staging* happened (the plumbing was verified alone before any reader existed)
while the *commit* is one, and the reason is worth stating rather than quietly abandoning ADR-0086 §1's practice.

The proof still did its job: had the plumbing changed any observable output, it would have shown *before* an
imported `Box(s64)` existed to hide it — which is the whole point of separating them.

### 4. What stays refused, and why each is separate

- **Inference through an imported parameterised struct** (`(b: *Box($T))`) — still deferred (ADR-0085 §5). It is
  a *procedure* inference question, not a type-resolution one: the same refusal applies to a local `Box($T)`
  parameter, so lifting it here would be lifting an unrelated feature.
- **A `$T` procedure imported and instantiated** — still E0268 (ADR-0104 §2). Cross-file *instantiation* appends
  a procedure to the caller's expanded HIR, which is a different mechanism from resolving a type; this ADR does
  not touch it.
- **`using` on an imported parameterised struct** — ADR-0050 §5 already defers `using` on any imported struct.

Keeping these separate is what makes this sub-wave a *type-resolution* change rather than a general lift of every
cross-file deferral.

### 5. Three things building it found, each by running

- **A field naming the declaring module's own type resolved in the wrong file.** `Wrapper($T) { helper: Helper; }`
  imported and instantiated failed, because `self.hir` was swapped to the module's (so `hir.scope` was right) while
  the *type value* still came from the **importer's** `FileSignatures`. `Ctx::resolving_in_module` carries the
  declaring module's signatures for the duration. This is the sharper failure of the two possible: had the importer
  happened to declare its own `Helper`, the field would have resolved silently to a **different type**.
- **A type-argument reference did not mark its import used.** A file importing a module *solely* for `Box(s64)`
  reported E0231 "unused import", and the quick fix beside that warning would have broken the build — ADR-0031 §2's
  rule. `resolve_type_name` already records an ordinary imported annotation; `resolve_apply` never reaches it,
  because the constructor is looked up separately, so it records it too.
- **A module name is a global identifier across both trees.** Adding `modules/Generic` shadowed
  `tests/corpus/modules/Generic.jr` — the fixture ADR-0104 uses for cross-file *instantiation* — and
  `imports/valid/017` silently resolved the wrong one, failing at MIR. Renamed `Generic_Types`. A new module must be
  checked against the fixture tree as well as the library one.

Also: the sema corpus harness tolerates **E0269** now, beside the E0212 it already tolerated, and for the identical
reason — that harness checks a file with *no modules loaded*, so an imported parameterised struct is "not a
parameterised struct" there exactly as an imported type is "unknown". The with-modules CLI corpus test is what
proves resolution.

## Consequences

- **`Map($K, $V)`, `List($T)` and `Array($T)` become writable**, which is what three library sub-waves were
  waiting for. Converting them is deliberately **not** part of this sub-wave: the language change and the library
  rewrite are separate, so a regression in either is attributable. `imports/valid/018` proves an imported
  `Box(s64)` works; the module conversions follow.
- **No new diagnostic code.** E0269's meaning narrows — it now means "not a parameterised struct in this file *or
  any imported one*" — and its note drops the sentence about cross-file being unsupported, because it no longer
  is.
- **The pool needed nothing.** ADR-0086's instance-keyed field map already keys on an instance whose `DeclId`
  carries the declaring file, so a cross-file instance was representable from the day that map existed. A
  generalisation built for a stated reason turned out to cover a case its author had deferred.
