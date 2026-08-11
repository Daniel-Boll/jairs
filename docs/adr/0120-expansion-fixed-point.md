# ADR-0120 — Expansion reaches a fixed point, and the two expansions compose

**Status:** Accepted
**Date:** 2026-08-07
**Supersedes:** nothing. **Amends** ADR-0082 §2 (one expansion round) and ADR-0101 §3
(which fixed one instance of the stale-key family this generalises).

## Context

`file_mir` builds MIR from a tree that two passes may have rewritten: a computed `#insert`
splices statements (ADR-0073), and instantiation appends one procedure per distinct
polymorphic key (ADR-0082). Both rewrites carry side tables keyed on
`(ExprScope, ExprId)` — a folded `#run` value, a `typed`/`untyped` target type, an
`any_of` lowering, a `$N` argument mask, a call→instantiation redirect.

An audit of the tree at `354d900` found that **four legal programs reached an internal
compiler error** while `jr check` reported no errors:

```jairs
inner :: (x: $T) -> T { return x; }
outer :: (flag: $U) -> s64 { n := inner(40); return n + 2; }   // ICE
```

```jairs
CODE :: "bonus := 2;";
id :: (x: $T) -> T { return x; }
main :: () { #insert CODE; exit(id(40) + bonus); }              // ICE
```

plus a `#run` and a `typed(…)` inside a template body. Each reported
`no routine for file N proc M` — the sixth appearance of that message, and this project's
own **#1 named failure mode**: a construct the grammar allows, with no representation on
the lowering path, where the gap surfaces as a well-typed value in the wrong place rather
than as a diagnostic. `scan` did refuse the affected bodies, but the refusal is **E0245, a
warning**, and only `main`'s refusal is gated — so the program linked and the call went to
a template that has no MIR.

Three distinct causes, one shape: **a key computed against one tree, read against
another.**

## Decision

### 1. Redirects are built from the *final* check, not the base one

`instantiated` collected its call sites from the base check and mapped each to an appended
procedure. But an instantiation's body is a **clone**, with its own `BodyId` and therefore
its own `ExprScope` — so the clone's copy of `inner(40)` is a call site no base-tree
redirect can name. The redirect map is now built from the check of the **final** expanded
tree, which is the only one that has seen every clone body.

Rejected: *rewriting the clone's call node to point at the instantiation directly.* That
would make the HIR describe a call the user did not write, which ADR-0050 §2 already
argued against for a different construct — and the LSP reads that HIR.

### 2. Expansion iterates to a bounded fixed point

A clone's body can instantiate a template the base tree never did, so one round is not
enough in general. `instantiated_from` now loops, accumulating distinct keys and
re-expanding from the starting tree with the whole accumulated list, until a round adds
none. `MAX_INSTANTIATION_ROUNDS = 8`.

Rebuilding from the starting tree each round rather than appending incrementally keeps
`new_ids[i]` paired with `keys[i]`, so an appended `ProcId` stays a function of the key
list alone — which the MIR snapshots depend on.

A bound rather than "until stable" for the reason `file_consts`' round limit is one: a bug
in the progress check should be a diagnosable stop rather than a hang.

### 3. The two expansions compose

`file_mir` ran instantiation **only when no `#insert` had expanded**, justified in a
comment as excluding "a computed `#insert` that *introduces* a polymorphic call". The code
implemented something far broader: it skipped instantiation whenever *any* insert expanded,
even when the polymorphic call owed the insert nothing. Instantiation now runs on whichever
tree is current, which is the narrow exclusion the comment always described.

This also fixed a latent bug beside it: `expanded_diagnostics` was assembled with
`expanded.map(…).or_else(|| instantiated.map(…))`, which reports one set of diagnostics
**or** the other. With the two expansions composing, both exist, and the `or_else` would
have silently dropped the instantiation's.

### 4. Non-convergence is refused — E0280

If eight rounds still produce new keys, the file yields an unbounded family of
instantiations. That is refused with **E0280**, owned by `jr-db` beside E0230, E0271 and
E0275, because convergence is a property of the expansion loop and the loop lives here.

Refusing is the point rather than a formality: the alternative is lowering a call whose
target was never appended, which is the ICE this ADR removes.

### 5. A clone inherits its template body's values — `ConstValues::copy_body_scope`

A `#run`, a `typed`/`untyped` or an `any_of` **inside a template body** was folded by
`file_consts` against the template's scope and had no entry under the clone's. Because
`append_one` clones a body with `hir.body(b).clone()` — arena and all — the clone's
expression at index *i* **is** the template's expression at index *i*, and only the scope
differs. So carrying the values across is a scope substitution, not a remap.

It deliberately does not copy the instantiation redirects or the comptime masks: a clone's
own polymorphic calls are redirected from the final check (§1), and copying the template's
would point a clone's call wherever the template's happened to resolve. Existing entries
under the destination are left alone, so the per-instantiation `type_info(T)` fold — the one
value that legitimately differs per binding (ADR-0092) — still overrides.

### 6. A `$N` call in a file with a computed `#insert` is refused — E0281

A `$N` argument's value is folded against the **unexpanded** tree and keyed by `ExprId`,
and a splice renumbers every id after it. This is not merely a missing value: a `$N` call
sitting *before* the splice keeps its key while one after it shifts, so the pairing can
silently deliver **another expression's value** — the well-typed-placeholder failure again,
in its worst form.

So it is refused with **E0281**, and the message names both ways out: make the `#insert`
operand a string literal, or move the comptime-value call to another file.

Rejected: *evaluating comptime arguments over the expanded tree.* That needs `file_consts`
to run on a tree that `insert_operands` produced from `file_consts`, which is the salsa
cycle ADR-0073 §4 restructured around. A refusal costs one narrow combination; breaking
that cycle is its own wave.

## Consequences

Four programs that reported an internal compiler error now run and agree in both engines;
one reports a diagnostic naming its workaround. Two new corpus files —
`valid/099-template-calls-template.jr` and `valid/100-insert-with-instantiation.jr` — pin
§1 and §3 with **asserted exit codes** rather than mere agreement between the engines,
because every failure mode here gives both engines the *same* wrong answer.

The MIR corpus snapshot grew by exactly the two new files and **nothing else changed**,
which is the evidence that the restructuring is behaviour-preserving for every program that
already worked.

Test count 986 → 988.

**What this does not fix.** A `$N` template whose body instantiates another `$N` template
is untested; §5's copy handles the `$T` case because a `$T` clone shares its template's
ids, and a `$N` clone rewrites parameter references to literals but does not renumber, so
it should follow — but "should" is not "does", and no corpus file makes it answer.
E0245 remains a **warning**, so a refused body still links; gating it on reachability is
its own change and would have masked the defects this ADR fixes rather than exposing them.
