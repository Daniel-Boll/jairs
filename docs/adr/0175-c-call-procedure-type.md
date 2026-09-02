# ADR-0175: A `#c_call` procedure type — the blocker W11 turned out to have

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **W11's first prerequisite**, discovered by probing rather than from the plan. §8.3 said W11 needs "a
  per-thread stack, atomics as language operations, and a rule for comptime"; it did not say a thread body
  could not be *named*.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

### The probe, and what it found in four minutes

W11 needs `pthread_create`, which takes a function pointer. The probe was five lines, and it failed three
times in a row, each failure narrowing the last:

1. `*worker` — **E0221**, "cannot take the address of this expression". A procedure has no address; it has a
   *value*.
2. `worker` into a `*u8` parameter — **E0214**, `expected u8, found (*u8) -> *u8`. So the value has a
   procedure type and the parameter must be spelled as one.
3. `(*u8) -> *u8` as the parameter type — **E0214 again**, and this time it read
   **`expected (s64) -> s64, found (s64) -> s64`.** Two identical types.

The third is the answer. `jr-pool` has *always* modelled the distinction — `Item::ProcType` carries a
`ContextKind`, and `jr-pool`'s own crate docs say "a `#c_call` procedure of the same shape is a *different*
type (ADR-0001)". What was missing is that **no type expression could say which**, so `ctx.rs` interned every
written procedure type as `ContextKind::Jairs` with a comment explaining that this was fine because "the type
syntax carries no `#c_call`".

It was fine, and it made every `#c_call` procedure **unpassable**. A `#c_call` procedure could be declared and
called directly and could not be handed to anything — which is exactly what a thread body must be.

**This is the item PLAN's open list called "the `#c_call` proc-pointer refusal"**, and its real shape was one
missing piece of *syntax*, not a missing mechanism.

## Decision

### 1. `#c_call` after a procedure type's return type

`(*u8) -> *u8 #c_call` parses, lowers to `TypeRef::Proc { c_call: true }`, and interns as
`ContextKind::CCall`. Four crates changed and none of them gained a concept: the parser, the AST accessor, the
HIR field, and one `if` in `ctx.rs`.

**After the arrow, matching a declaration.** `f :: (n: s64) -> s64 #c_call { }` puts it there, and a type
spelling it elsewhere would give one convention two readings.

**Rejected: a distinct type constructor** such as `c_proc(...)`. It would avoid the ambiguity §5 records and
introduce a second way to say "procedure type", which every consumer would then have to handle twice.

**Rejected: inferring the convention from the argument at the call site.** It reads well —
`pthread_create(…, worker, …)` "obviously" wants C's convention — and it makes the *type* of a parameter
depend on what is passed to it, which is not a thing this type system does anywhere else.

### 2. An indirect call reads its convention from the callee's type

Three engines hard-coded the Jairs convention at an indirect call, each with a comment saying it was safe
because no `#c_call` pointer type existed. §1 invalidated all three at once, and each said so differently:

- **MIR** prepended the context unconditionally → `internal compiler error: called a procedure taking 1
  arguments with 2`.
- **Cranelift** built a two-parameter signature → the verifier's `mismatched argument count ... got 1,
  expected 2`.
- **LLVM** built a two-parameter function type → no error at all until the call ran.

All three now read `ProcType.context`. **The LLVM one is the reason this matters**: two of the three failed
loudly and the third would have passed the context where C expects the first argument, silently.

`pointer_takes_context` answers `true` for anything it cannot determine, which is the safe direction: a
Jairs-convention call missing its context is an arity error the verifier catches, while a `#c_call` call *given*
one is a wrong argument.

### 3. `describe` renders the convention

Without it the mismatch in §2 of the Context above reads `expected (s64) -> s64, found (s64) -> s64`, which
tells a reader nothing and looks like a compiler bug. ADR-0001 made the two different *types*; this makes them
different *words*.

### 4. A procedure cannot cross a `#foreign` boundary at compile time — and that settles the comptime fork

PLAN §7 named the open decision: *what does a `#run` that spawns a thread mean?* Three options were on the
table — refuse it, serialise it, or grow a scheduler in the VM.

**The probe settled it at a lower level than any of them.** The VM's marshalling refused a procedure argument
with `passing ProcType { … } to a foreign procedure arrives with a later wave` — a message that dumped a Rust
`Debug` struct *and promised something unreachable*. C needs a machine address to call; the interpreter
executes bytecode and there **is no machine code** for a Jairs procedure.

So:

- **Refuse** — chosen, and *forced*. The refusal now says why: "C needs a machine address to call and the
  compile-time interpreter has no machine code to point at".
- **Serialise** — rejected. Running the body inline at spawn changes the program's meaning silently, and the
  project's central claim is that comptime and runtime execute the same MIR and agree (§3.1).
- **A scheduler in the VM** — rejected, and **not merely expensive: unreachable.** A scheduler still needs a
  thread body to *run*, and every spawn API takes a function pointer. Without a JIT there is nothing to hand
  over. This option was on the table because nobody had checked what it would have to produce.

**A decision that was going to be made on taste is now made on a fact**, which is the whole argument for
probing first.

### 5. `f :: () -> (s64) #c_call` binds the directive to the *type*

A real ambiguity, and tree-sitter reported it as an unresolved conflict rather than letting it pass. The
hand-written parser resolves it greedily — `parse_proc_type` consumes the directive before returning — so the
innermost construct wins.

**Verified, not assumed**: `pick :: () -> (s64) -> s64 #c_call { return id; }` compiles, returns a `#c_call`
procedure, and calling the result works.

The grammar **declares the conflict** rather than taking a `prec`, for the reason the `result_list`/
`proc_type_params` conflict does: a `prec` silently picks one reading, and if it ever picked the other the
program would still compile and pass the context where C expects an argument.

## Consequences

- **A `#c_call` procedure can be passed**, so `pthread_create` is reachable and W11 can exist.
- **Three engines read the convention at an indirect call** instead of assuming one.
- **The comptime-spawn fork is closed on evidence**, and the VM's refusal states a reason instead of promising
  a later wave.
- **The formatter dropped `#c_call` from a type** — caught by gate 5 on this wave's own module, the twelfth
  consecutive wave that loop has had to learn a construct. Dropping it is the *unsound* direction: the
  reformatted file no longer type-checks.
- **The tree-sitter grammar needed the rule too**, and without it reported an `ERROR` node — which stops
  highlighting for the rest of the file, the silent failure gate 6 exists to catch (ADR-0025 §4).
- **`jr-vm`'s marshalling fallback no longer dumps a `Debug` rendering** into a diagnostic.
