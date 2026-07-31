# ADR-0057: `context` is a real hidden parameter, with one field and no allocator yet

- **Status:** Accepted
- **Date:** 2026-07-30
- **Deciders:** dboll
- **Implements ADR-0001**, which decided the ABI — a hidden *trailing* parameter, `#c_call` to opt out
  — in the vertical slice, and put `ContextKind` in every procedure type so this wave would not have
  to re-type every signature. §1 is that decision becoming real; §4 explains why the parameter is
  **leading** rather than trailing and amends ADR-0001 on that one point.
- **First feature of W3.** Allocators and temporary storage both live in the context, so they wait on
  this.

## Context

`ContextKind::Jairs` sits in every `Item::ProcType` and **nothing passes a context**. ADR-0001 fixed
the calling convention in the slice specifically so this wave would be a matter of implementation
rather than of re-typing, and that has held: the type side needs no change at all.

Six facts were established by reading the code, and four shaped the decisions.

- **`ContextKind` is already part of a procedure type's identity**, and `jr-sema` already sets
  `CCall` for every `#foreign` declaration. **So §3 needs no sema change**, which is ADR-0001's
  reserved slot paying off exactly as intended.
- **`context` is not a keyword** and `#c_call` is not parsed. Both are new syntax.
- **ADR-0051 built a leading hidden parameter** — the `sret` result pointer — with
  `ArgumentPurpose::StructReturn`, and ADR-0053's lesson was that the *presence* of a hidden
  parameter shifts every other argument. **This is the fact that decides §4**: a second hidden
  parameter must be positioned so the two cannot be confused, and one shared predicate must decide
  both.
- **The VM's `Instr::Call` builds an argument vector positionally**, so a context is an extra entry
  in it and needs no new instruction.
- **A `Context` struct is an ordinary aggregate**, so ADR-0051's `sret` machinery already knows how to
  pass one by pointer — which is what a mutable context requires.
- **`main` has no caller**, so something must create the first context. §5 is about that.

## Decision

### 1. `context` is a value of a compiler-known struct type, reached by a keyword

```jr
f :: () -> s64 {
    return context.allocator;      // read
}

g :: () {
    context.allocator = 7;         // write, visible to callees
    h();
}
```

`context` is a new keyword denoting the current context. Its type is `Context`, a struct the compiler
declares rather than the source: `Context :: struct { allocator: s64; }`.

**One field, and it is deliberately not usable as an allocator.** `allocator: s64` is a placeholder
that a program can read and write, which is what makes the ABI observable end to end. What an
allocator *is* — a procedure pointer plus data — needs indirect calls, which both engines still
refuse, so it is a separate ADR and a separate wave.

**Rejected: an empty `Context`.** Zero-sized, so the ABI is exercised merely by the parameter
existing. Rejected for a concrete reason: a zero-sized parameter is exactly the thing a calling
convention may elide, so the test would pass while nothing was passed. A field a program can read
back is the only way to know the value arrived.

**Rejected: `context` as an ordinary name in scope.** It would need no keyword, and it would collide
with any user variable called `context` — and ADR-0014 §3's position is that behaviour must not depend
on an invisible name, which an implicitly-declared local is.

### 2. The context is passed **by pointer**, and a callee's writes are visible to its own callees

`g` above sets `context.allocator` and calls `h`; `h` sees 7. A copy would make that silently not
work, and "set the allocator, then call" is the entire point of a context.

The hidden parameter is therefore a `*Context`, not a `Context`. That also makes it one machine word
regardless of how many fields the struct grows, which matters because every Jairs call carries it.

**A caller's context is not affected by its callee's writes.** `g` sets the field, `h` sees it, and
`g`'s own caller does not — because each Jairs procedure that *modifies* the context does so on a
copy it owns. Jai spells this `push_context`; this wave has no such form, so a write is visible
downward only, and §6 records the absence.

### 3. `#c_call` opts out, and `#foreign` implies it

```jr
plain :: () { }                              // receives the context
raw :: () #c_call { }                        // does not
write :: (…) -> s64 #foreign libc "write";   // implicitly #c_call
```

This is ADR-0001's decision unchanged, and `ContextKind` already encodes it. The parser learns
`#c_call`; sema already sets `CCall` for `#foreign`.

**A `#c_call` procedure cannot mention `context`**, because it has none — E0254, with a message
saying so rather than reporting an unresolved name.

**Rejected: inferring the opt-out from the body.** A procedure that never mentions `context` could
skip the parameter, so most code would pay nothing. Rejected because it makes a procedure's *ABI*
depend on its body: adding a `context` read to a leaf function would silently change its calling
convention and every caller with it. ADR-0001 rejected this too, and the reason is worth restating
because the performance argument is genuinely attractive.

### 4. The context parameter is **leading**, after `sret` — amending ADR-0001

ADR-0001 said "hidden *trailing* parameter". This makes it leading instead, immediately after
ADR-0051's `sret` pointer when there is one:

```text
mk :: (a: s64) -> Vec2      →  (sret: *Vec2, ctx: *Context, a: s64)
add :: (a: s64) -> s64      →  (ctx: *Context, a: s64)
raw :: (a: s64) #c_call     →  (a: s64)
```

**Two reasons, and the second is the load-bearing one.**

Trailing means the context's *position* depends on the argument count, so every site computing an
index has to know it. Leading means the hidden parameters occupy positions 0 and 1 and the declared
ones start at a fixed offset — which is the shape ADR-0051 already established and `bind_entry_params`
already skips a prefix for.

And ADR-0053 §1 recorded that a hidden parameter shifts every other argument by one, which was a
silent miscompile when the cursor started at 0. There are now **two** hidden parameters, so the
offset is 0, 1 or 2 — and a single shared predicate must compute it, exactly as
`repr::returns_via_sret` does for the first one. Trailing would make that predicate need the argument
count as well.

**This amends ADR-0001 rather than reversing it.** The decision that mattered — a hidden parameter
rather than a global, so that a `#c_call` boundary is explicit and the type carries the kind — is
untouched. Only the position changes, and ADR-0001 chose trailing before `sret` existed.

### 5. `main`'s context is created by the entry stub

`main` has no Jairs caller, so `jr-codegen-clif`'s entry stub allocates a zeroed `Context` on its own
stack and passes its address. The VM does the same in `run_main`.

**Zeroed rather than uninitialised**, so `context.allocator` reads 0 in a program that never sets it —
a defined value rather than garbage, matching what ADR-0039 §4a decided for a default-initialised
aggregate.

### 6. What is deliberately absent

- **No allocator protocol** (§1), and so no `alloc`/`free`. That needs indirect calls.
- **No `push_context`**, so a write is visible to callees and not to callers (§2). Jai's form
  introduces a scope, which interacts with `defer` (ADR-0049 §3) and deserves its own decision.
- **No temporary storage**, which §2.1 lists in W3 and which wants an allocator first.
- **No context on an operator overload's implicit call.** An overload is an ordinary procedure
  (ADR-0048 §5) so it *receives* one; what is absent is any way to give it a different one.

## Consequences

- **Every Jairs call in both engines gains an argument**, so the MIR snapshot changes for every corpus
  file that calls anything. That is a large diff of a mechanical kind, and reviewing it means checking
  the *shape* rather than reading 130 files — the differential harness is what says the values are
  right.
- **One new diagnostic code, E0254**, for `context` in a `#c_call` procedure. **E0255 is the first
  free code.**
- **`Context` is the first compiler-declared type.** It has no `DeclId` from any file, so
  `Item::StructType`'s nominal identity (ADR-0015 §1) needs a synthetic one — the same problem
  ADR-0052 §1 solved for a results aggregate by going structural, and the same answer applies.
- **A `#c_call` procedure calling a Jairs one has no context to pass**, and that is a real hole: it
  must either be refused or manufacture one. Refusing is right for this wave — a `#c_call` procedure is
  a boundary, and a boundary that silently invented a context would hide where one came from.
- **`jr-fmt` needs `#c_call`**, and the formatter has lost a construct in six of the last seven waves.
  A test must assert survival *and* canonicalisation.
- **A corpus program must observe a callee reading what its caller wrote**, because §2's by-pointer
  decision is invisible in any program that only reads the context.
