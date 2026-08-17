# ADR-0134: nested procedures and local constants — Jai-style, no capture

- **Status:** Accepted
- **Date:** 2026-08-17
- **Deciders:** dboll
- **Wave 5 of eight.** ADR-0128 was wave 1 (instantiation backtraces), ADR-0129 wave 2 (enum member
  from a constant), ADR-0130–0132 wave 3 (`Math`'s vec/mat/quat), ADR-0133 wave 4
  (`it`/`it_index`). This wave lifts E0207 — the fifth of ADR-0127 §3's six unkept promises to be
  kept, and the last one that stood behind an expired-deferral note that read "no wave currently
  owns them" while the plan's §7 table had decided the shape years ago.
- No design fork was put to the decider. PLAN §7's table already recorded the shape — *no capture,
  Jai-style — a file-scope proc with a scoped name* — and this wave implements it. §2 records two
  decisions the implementation forced that were not in the table.

## Context

### The refusal, and what stood behind it

`X :: 5;` and `foo :: () { }` inside a procedure body both produced E0207 with the note "a nested
procedure and a local constant are both unimplemented, and no wave currently owns them". The
comment above the arm said BodyLowerCtx *has no access to the file-level item arena to lower them
into*, and that was the actual gap: the two arenas are separated, and body lowering ran to
completion without ever calling anything that allocates an item.

### The shape PLAN §7 had already decided

Two properties, one for each half:

- **A nested procedure is a *file-scope proc with a scoped name*.** No closure over the enclosing
  locals, no captured environment. The body of a nested `foo :: () { … }` behaves exactly as a
  file-scope `foo`'s body — including having none of its enclosing procedure's locals in scope.
  This is what the PLAN §7 row said, and what this wave delivers.
- **A local constant is a *value bound to a name*.** `X :: 5;` inside a body reads and behaves as
  a compile-time constant: participates in arithmetic, cannot be assigned to (because
  `ConstDecl` is separate from `VarDecl` in the AST), and lives inside its declaring body's scope.

Both share the same syntactic form (`X :: <value>;`) and the same AST node (`AstItem::Const`), so
the wave that lifts one lifts the other by construction.

## Decision

### 1. Hoist to `items`, hide the name from `hir.scope`, register in the body's scope

Every nested `X :: <value>;` is *hoisted* into the file's `items` arena — allocated, checked,
lowered, linked exactly like a top-level item. The difference is that its name **is not** added to
`hir.scope`, so a lookup from anywhere in the file except the enclosing body's scope stack does
not find it. A new `Item::nested: bool` flag records the hoist so `check_duplicates` can skip
these when scanning for user-visible collisions.

Two nested items with the same name in different enclosing procedures are therefore legal:
`first` may declare `helper :: () -> s64 { return 1; }` and `second` may declare
`helper :: () -> s64 { return 2; }`, and both `helper()` calls resolve to the right body.
`valid/107` pins this — a pre-wave version of the file would fail to compile with the wrong
`helper` picked.

**Rejected: put nested items in `hir.scope`.** Simpler to implement (one boolean fewer), but it
loses the "scoped name" half of the plan: two nested items with the same name across the file
collide because `hir.scope` is a single map. The plan chose the scoped shape; this decision
implements it.

**Rejected: name-mangle nested items** (e.g. `outer::inner` at file scope). Two nested `helper`s
would no longer collide because their mangled names differ. Same visible behaviour, but the
mangled name leaks into diagnostics, hover, and `jr dump` output — a caller reads a message about
`outer::inner` when they wrote `inner`, and every downstream tool has to know to unmangle. The
hidden-in-scope-not-name-mangled shape keeps the user-visible name honest at the cost of one flag
on `Item`.

### 2. Sibling scope is *injected* into every nested proc's own body

A nested `factorial :: (n) -> s64 { … factorial(n - 1) … }` needs to see itself for recursion.
Two nested procs in the same block — `add` and `twice :: (n) { return add(n, n); }` — need to see
each other. Neither works if the nested body's scope is only its own parameters.

`lower_body_inner` now accepts an `inherited_items: &[(Symbol, ItemId)]` parameter that is
injected into the body's outermost scope before parameters are pushed. When the drain loop in
`lower_body_inner` calls `lower_hoisted_const` on each pending hoist, it passes **every** sibling
name (including the one being lowered) so a nested proc sees itself and every other nested proc
declared beside it. Parameters shadow siblings by ordinary Vec-order push, so a nested proc's
`(n: s64)` shadows a hypothetical sibling named `n`.

**The reservation trick that makes this work.** Nested items are allocated *before* their own
bodies are lowered — `lower_const_decl_with_inherited` reserves the item slot with a placeholder
`ItemKind::Var { ty: None, init: None, uninit: false }`, lowers the value, then patches the
`kind`. This is what keeps `items.len()` in sync with the ItemId predictions the body makes for
its own nested items: if the outer nested item's slot were only allocated *after* its body was
lowered, its inner nested items would predict the outer's position and the recursion would
assert. The placeholder is deliberately `ItemKind::Var { … }` and not `ItemKind::Const {
ConstValue::Expr(err) }`, because the second form allocates a spurious top-level expression that
would shift every top-expr index by one — which the
`resolve_map_does_not_collide_top_level_and_body_expression_ids` regression test probes.

**Rejected: two-pass lowering** (pre-scan the block for nested items, allocate their slots
upfront, then lower). Cleaner separation, at the cost of walking the AST twice. The single-pass
reservation + patch works and touches only `lower_const_decl_with_inherited`, so the interior of
the wave is smaller than the two-pass version would be.

### 3. No capture — enforced by name resolution, not by a new refusal code

`inner :: () -> s64 { return outer_local; }` inside `outer :: () { outer_local := 42; … }` is
E0201 (`unresolved name outer_local`). The resolver finds `outer_local` neither in `inner`'s body
scope (which has only `inner`'s params and inherited siblings) nor in file scope (which does not
have body-local names). Nothing changes here — the "no capture" refusal falls out of the fact
that the nested body's scope is *not* inherited from the enclosing body.

**Rejected: a dedicated capture-refused code.** It would read better in the diagnostic
("nested procedures cannot capture; move `outer_local` to a parameter or a file-scope constant"),
but shipping a new code with no separately-testable failure mode is exactly the "arrives in wave
Wn" trap ADR-0127 caught eleven times. The refusal falls out of ordinary scope resolution, and
the message a caller sees ("unresolved name `outer_local`") is honest — the name is genuinely not
in scope from the nested body.

## Consequences

- **The eight-wave programme is 6 of 8 done.** Waves 6–8 remain: `[..]T` dynamic arrays, `$$T`,
  and `print(fmt, ..Any)`. Wave 5 was the last of ADR-0127 §3's six unkept promises to be
  surfaced.
- **E0207 is retired for `AstItem::Const`.** It stays for `AstItem::Run` inside a body — that
  refusal is a separate decision about statement-position `#run` (ADR-0069's territory).
- **1010 tests unchanged, +1 corpus file = 221.** The pattern from the previous four waves — a
  wave's coverage rides on the corpus differential rather than on Rust unit tests — recurs here.
  `valid/107` exercises every property this ADR pins.
- **Deferred, not declined**: capture over enclosing locals is not this wave's fork — it is
  declined outright per PLAN §7. A `#run`-as-statement is still owed its own decision. The
  no-capture refusal reads as an unresolved-name error today; a dedicated capture-refused code
  (with its help text) is a real improvement and belongs to whatever wave introduces the first
  form that closure would make sensible.
- **One regression test's premise changed.** `declarations_inside_a_body_are_reported_not_silently_dropped`
  used to require an E0207 and a `Stmt::Error` placeholder — the shape ADR-0127 renamed it
  to "arrives in wave W2" behind. The test now asserts the *positive* form: nested declarations
  are *hoisted* into the item arena and the body sees `Stmt::Item(item_id, span)`, with a proc
  item for `inner` beside its enclosing `outer` in `hir.items`. The rename makes the check into
  a regression guard for the wave's own delivery.
