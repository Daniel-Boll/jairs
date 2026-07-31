# ADR-0063: `push_context` gives a block its own copy of the context — amending ADR-0057 §2

- **Status:** Accepted
- **Date:** 2026-07-31
- **Deciders:** dboll
- **Amends ADR-0057 §2**, whose isolation claim — "a caller's context is not affected by its callee's
  writes… each Jairs procedure that modifies the context does so on a copy it owns" — is **not true in
  the implementation**, and never was. §2 below records what the implementation actually does and why
  `push_context` is the construct that makes the claim real where a program asks for it.
- **Sixth feature of W3** (ADR-0057 §6 named it absent), and the one that makes a *scoped* allocator
  possible — the shape ADR-0062 §5 said temporary storage wants.

## Context

ADR-0057 §2 says two things. The first is true: the context is passed **by pointer**, so a callee's
write to `context.allocator` is visible to *its* callees. The second is false: it claims a callee's
write is *not* visible to its caller, "because each Jairs procedure that modifies the context does so
on a copy it owns." No copy is made anywhere. The hidden parameter is one `*Context`, threaded
unchanged through every call, so a write through it is visible in **every** direction — down to
callees and back up to callers alike.

This was verified by running, not by reading:

```jr
callee_writes :: () { context.allocator_data = 42; }
main :: () {
    context.allocator_data = 7;
    callee_writes();
    if context.allocator_data == 7 { exit(0); }   // isolation would exit 0
    exit(1);                                        // leak exits 1
}
```

Both engines exit **1**: the callee's write of 42 leaked back to `main`. Worse for the ADR, corpus
`050-allocator.jr` *relies* on the leak — `counting_alloc` writes `context.allocator_data` and `main`
reads the accumulated total back — so the leak is not merely present, it is load-bearing, and
"fixing" §2 by making every call copy would break a passing corpus program.

So the honest position is: **the context is one shared mutable object reached by pointer, and writes
propagate both ways.** That is a defensible default — it is exactly what makes "set the allocator,
then call" work, and what makes a stateful allocator's data word travel. What was missing is any way
to say "these writes are mine, restore the caller's context when I leave" — Jai's `push_context`,
which ADR-0057 §6 recorded as absent and this ADR adds.

## Decision

### 1. `push_context { … }` introduces a scope with a private copy of the context

```jr
push_context {
    context.allocator = scratch_alloc;    // visible to callees of this block
    do_work();                            // allocates from scratch_alloc
}
// here, context.allocator is whatever it was before the block
```

Inside the block, `context` names a **fresh copy** of the context as it was on entry. Writes to it
are visible to the block's callees (the pointer still threads down), and are **discarded on exit** —
the enclosing scope's context is unchanged, whichever way the block is left.

**The form takes no explicit context value**, unlike Jai's `push_context <expr> { … }`. The reason is
concrete: `Context` is not a spellable type — naming it is E0212 (`unknown type name`), because it is
the first compiler-declared type and has no `DeclId` (ADR-0057 Consequences). A program therefore has
no `Context` value to hand to `push_context`, so the only thing the construct can do is copy the
*current* one. When `Context` becomes nameable — it need not, for the slice — a value-taking form is a
compatible extension, recorded in §5.

**Rejected: `push_allocator(a)` as sugar for the common case.** Most `push_context` uses swap only the
allocator, so a narrower form would cover them with less machinery. Rejected because it bakes one
field into the language: the context grows fields (ADR-0057 §1 already lists temporary storage), and a
per-field push form would need one keyword per field. `push_context` scopes the *whole* context, and a
program writes the one field it means to change inside the block.

### 2. It is lowered as a copy plus a pointer swap — no new MIR node, no back-end change

`jr-mir` lowers `push_context { body }` by:

1. allocating a fresh slot of type `Context`;
2. `Store`-ing into it the aggregate **loaded** through the current context pointer — the identical
   `Load`/`Store` pair that lowers `b := a` for any aggregate, which both engines already memcpy;
3. pointing the lowering context (`Lower::context`, the `*Context` operand every `context` expression
   and every call reads) at the **address of the new slot** for the duration of the body;
4. restoring the previous operand after the body, on the fall-through path.

**No `Statement`, `Rvalue`, `Terminator`, VM opcode or Cranelift primitive is added.** The whole
feature is a lowering-time manipulation of which pointer `context` resolves to, built from aggregate
copy (ADR-0039 §4a's `Load`/`Store` of a struct) and slot allocation that already exist. This is the
same evidence ADR-0048 §5 took as a design fitting: a feature that needs no new IR node is a feature
the IR was already shaped for.

**Restoration is a compile-time pointer swap, not a runtime save/restore.** Because the swap is which
SSA operand later code reads — not a mutation of memory — leaving the block on *any* path (fall
through, `return`, `break`, `continue`) automatically uses the outer pointer again: those paths
terminate the block and lowering resumes with `Lower::context` already restored. There is nothing to
run on the exit path, so unlike `defer` (ADR-0049 §3) there is no per-exit-path emission. §3 records
the one subtlety this creates.

**Rejected: a runtime push/pop of a context stack.** A `*Context` global with a save/restore around
the block is how a language without the context-as-parameter design would do it. Rejected because the
context is already a parameter, not a global (ADR-0057 §2, unchanged) — there is no stack to push, and
introducing one would reintroduce exactly the global ADR-0001 refused.

### 3. A `defer` inside `push_context` runs against the pushed context

`defer` runs its statement at scope exit (ADR-0049 §3). A `defer` written *inside* a `push_context`
block runs while `Lower::context` still points at the copy — because the block's defers are emitted on
the fall-through path *before* the pointer is restored, exactly where they are for an ordinary block.
So a `defer context.allocator_free(p)` inside a `push_context` frees through the pushed allocator,
which is what a reader expects: the `defer` and the allocation it releases see the same context.

**This is the one ordering that needed a decision**, and it is why the restore happens after the
block's own defers are emitted rather than before. The corpus checks it.

### 4. `push_context` is refused in a `#c_call` procedure — E0254

A `#c_call` procedure receives no context (ADR-0057 §3), so there is nothing to copy. `push_context`
there is E0254, the same code and the same reasoning as `context` itself in a `#c_call` procedure —
a construct that needs a context where none exists. The message names `push_context` rather than
`context` so the diagnostic points at what was written.

**No new diagnostic code.** E0254 already means "this needs a context and there isn't one", and
`push_context` is another instance of exactly that. **E0258 is still the first free code.**

### 5. What is deliberately absent

- **`push_context <expr> { … }`** — the value-taking form. Absent because `Context` is unspellable
  (§1); a compatible extension when it is not.
- **`push_context` as an expression** — it is a statement only. There is no reason a block that scopes
  the context should produce a value, and making it an expression would raise "what is its type" for
  no gain.
- **Temporary storage** — still W3's last feature, and it now has the scoping form it wanted: a scratch
  allocator installed in a `push_context` block is restored on exit. What it still lacks is a *bump*
  allocator, which needs pointer arithmetic (E0223) — recorded in the handoff as possibly-blocking and
  unchanged by this wave.

## Consequences

- **ADR-0057 §2's isolation half is now documented as false**, and this ADR is where a reader learns
  the real rule: the context is one shared mutable object, writes propagate both ways, and
  `push_context` is the only isolation boundary. That is a correction to the record, made where the
  record is immutable — which is what a new ADR is for.
- **Every existing program is unchanged.** `push_context` is new syntax; nothing that did not write it
  behaves differently, and the shared-context behaviour corpus `050` relies on is untouched.
- **One new keyword, `push_context`**, and one new statement node, `PUSH_CONTEXT_STMT`. It is a
  reserved word from this wave; a program using `push_context` as an identifier now gets a parse error
  rather than a name, which is the cost of any new keyword and is why the grammar's keyword set is
  where the decision is visible.
- **The MIR snapshot of a body containing `push_context` shows the copy** — a `Load` of the context
  aggregate and a `Store` into a fresh slot — which is the shape a reader should see and the evidence
  the copy happens once, on entry, not per access.
- **A `#c_call` proc-pointer type is unaffected** — this wave adds no attribute-in-type syntax
  (ADR-0062 §5 still owes it).
