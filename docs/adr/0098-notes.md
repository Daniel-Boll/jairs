# ADR-0098: `@note` attaches metadata to a declaration for a metaprogram to read — W6 opens

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W6 sub-wave 1.** W5 is complete (ADR-0081–0097). This opens W6 — Metaprogram, the wave PLAN §2.1 calls
  "the Jai superpower", where build scripts become the build system.

## Context

W6's scope is workspaces, the compiler message loop, `#run build()` build scripts, plugin hooks, and `@note`
attributes. Notes are the right first piece, and the reason is an ordering argument rather than a size one.

**Notes are the data the rest of W6 operates on.** The message loop's purpose is to hand declarations to a
build script — and a declaration with nothing extra to say is not worth handing over. A build script's first
real job is "collect every declaration tagged `@X`". So building the loop first would mean designing its
message shape against no consumer, which is precisely the failure ADR-0080 §3 named when it declined a `Code`
value: *worth representing only once something can inspect it.*

Notes are also the only self-contained piece: parse, HIR, formatter, grammar, and nothing else.

Premise verified by running: `@deprecated` on a procedure was E0106.

## Decision

### 1. A note is `@name` or `@name "payload"` on a declaration, and it is its own node kind

`NOTE` is a `SyntaxKind` of its own, taken in the same attribute loop as `#c_call`, `#no_abc`, `#expand` and
`#modify`, so a declaration may carry notes and directives **in any order** — the rule ADR-0058 settled for
the directives themselves.

**Its own kind rather than a generic attribute**, because a note is *data for a metaprogram* while the
directives are *instructions to the compiler*. A consumer collecting notes must not have to filter directives
out of the same list, and a query colouring one must not have to exclude the other. `Proc::notes:
Vec<(Symbol, Option<String>)>` carries them, with the payload's quotes stripped at lowering so every consumer
sees text.

The name is required and a missing one is an error: a note with no name is nothing a metaprogram could look
up. The payload is optional, which is what makes `@deprecated` and `@requires "x"` one form rather than two.

`looks_like_proc_signature` needed `AT` — the token-set trap for the **seventh** time, since a procedure whose
signature is followed by a note, with no `->`, reaches neither `ARROW` nor `L_BRACE`.

### 2. A note attaches to a **declaration**, carrying a name and an optional string

Not to an arbitrary expression, which would raise "what does a note on `a + b` mean" — a question nothing
needs answered. Not arbitrary key-value pairs, which is a superset nothing in W6 consumes yet; ADR-0080 §3's
rule again.

A note affects **no code**. A clone of a noted procedure keeps its notes (a `$T` instantiation, a baked
specialisation) because the clone *is* that procedure; the synthetic `#modify` predicate carries none, since
it is not a declaration the author wrote.

## Consequences

- **The formatter dropped every note on its first run**, turning each declaration into an unnoted one — the
  lossy-CST trap, caught by gate 5 on this wave's own corpus file. This is the *metaprogram-input* direction
  of that failure: a build script collecting `@X` would have silently found nothing.
- **No new diagnostic code.** A malformed note reuses the parser's E0131; nothing new is refused.
- `valid/079` exercises a bare note, a payload, several on one declaration, notes on both sides of a
  directive, and notes on a `#expand` macro and a `$T` procedure — so notes compose with everything W5 built.
  Its MIR is exactly what it would be without them, which is the point.
- **What notes still lack is a reader**, and that is deliberate: the next sub-wave is the mechanism that lets
  a metaprogram *ask* for the declarations carrying a note. Shipping the data first is what gives that
  mechanism something to be designed against.
