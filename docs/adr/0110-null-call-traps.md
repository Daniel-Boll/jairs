# ADR-0110: Calling a null procedure pointer traps — and the VM's handle is biased so zero means null

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W7 sub-wave 8.** Found while probing the allocator convention for `String`'s allocating half: the *first*
  thing tried — `context.allocator(8)` without installing one — leaked an internal compiler error. That is a
  bigger finding than the sub-wave's intended subject, so it ships first and alone.

## Context

`context.allocator` is **null until something installs one** (ADR-0057 §5). So `context.allocator(8)` before an
installation is a configuration mistake a reader will actually make, and it is the ordinary way to reach a null
procedure-pointer call.

**Both engines were wrong, in different ways.** A proc pointer in the VM is a packed handle
`(file << 32) | proc` (ADR-0059 §4), so a null one decoded to **file 0, procedure 0** — an arbitrary *real*
procedure — and calling it gave `called a procedure taking 1 arguments with 2`: an internal compiler error naming
an arity nobody wrote. Native code uses a real code address and would have jumped to zero, taking a signal the
compiler has nothing to say about.

That is two different wrong behaviours where a **language trap** belongs.

## Decision

### 1. A null callee is a trap, in both engines, and the VM's handle is biased by one

`Trap::NullCall` / `TrapKind::NullCall`, message *"call through a null procedure pointer"*. Both engines exit 4
(ADR-0019 §2) with a source location and the live call chain, like every other trap.

A trap rather than a diagnostic, because a procedure pointer's value is a **run-time** fact: sema cannot know
whether the field was installed. And rather than a signal or a wrong call, because there is no procedure to call
and no answer to invent — the same argument ADR-0068 §4 makes for reading the wrong `variant` case.

**The VM's handle needed a bias**, and finding out why is the interesting part. `valid/048` calls `add`, which is
**file 0 procedure 0** — the *first* procedure in a file — and that packs to handle `0`, indistinguishable from
`null`. The first version of the check trapped on it, and the corpus differential said so immediately. So the pack
is now `((file << 32) | proc) + 1` and the unpack subtracts the bias.

Native needs no bias: a code address is never zero. So this is the **VM's encoding earning a property native
already had**, not a language change — nothing observes a proc pointer's bits (ADR-0059 §4), which is exactly why
the encoding is free to change.

### 2. Why this is a differential test rather than a corpus file

A trapping program cannot be a corpus file: those must exit 0. And the two engines were wrong *differently*, so
the property worth pinning is that they now agree — which is what `differential.rs` is for. The test asserts the
exit code, that the message names the null pointer rather than an arity, and that the two engines' entire
observable behaviour matches.

## Consequences

- **The seventh leaked internal error turned into a real diagnostic**, and the first found by probing a *library
  convention* rather than a language feature. `String`'s allocating half is the sub-wave that was intended; this
  is what asking "which allocator?" turned up on the first attempt.
- **`valid/048` earned its keep in an unexpected way.** It exists to check indirect calls, and what it caught was
  an encoding collision two waves later — a file whose *subject* is a mechanism will notice a change to that
  mechanism's representation, which is an argument for corpus files being specific.
- **No new diagnostic code**, because this is a trap rather than a diagnostic — traps are named, not numbered.
- **The allocator convention question is unchanged and still open**: `String`'s allocating half is next, and it
  now has one fewer trap waiting for it. What this sub-wave settles is that a caller who *forgets* to install an
  allocator gets a sentence instead of a puzzle.
