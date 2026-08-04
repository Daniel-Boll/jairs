# ADR-0090: `#expand` marks a macro whose body a call splices into the caller's scope — the surface

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W5 sub-wave 7a.** This delivers the *surface* of `#expand`: a macro parses, lowers, formats, and its
  declaration checks like any other procedure. A **call** is refused by design (E0272) pending the splice,
  which is the next sub-wave — the staging ADR-0081 used for `$T` and ADR-0087 for `$N`.

## Context

`#expand` is the core of Jai's macro family. A macro is a procedure whose body a call **splices into the
caller's scope** rather than calling: the statements land where the call was written, so they can read and
modify the caller's locals. That is what makes a macro able to express a custom `for`, an early `return` on
the caller's behalf, or anything else a call cannot.

It is the right one of the three to build first. `#modify` (run code at compile time to inspect or reject an
instantiation) and `#bake_arguments` (produce a specialised procedure from a partial application) are
refinements *of a macro*; neither is meaningful before one exists. `#expand` also composes with the splice
ADR-0072 and ADR-0080 already built.

Premise verified by running before this ADR was written, per AGENTS.md: `double :: (x: s64) -> s64 #expand
{ … }` was **E0106** ("expected a procedure body or `#foreign`"), so it is a real feature.

## Decision

### 1. `#expand` is a procedure attribute, in the existing attribute loop

The parser's attribute loop between the return type and the body — which already takes `#c_call` and
`#no_abc` — takes `#expand` too, so the three may be written in any order. `EXPAND_ATTR` is its own
`SyntaxKind` and `expand_attr` its own grammar rule, beside the other two: a consumer that handles one and
forgets this one is then a **missing arm** rather than a string comparison that falls through, which is the
route by which `jr-fmt` has lost a construct in most of the last dozen waves. `Proc::expand: bool` carries
it into the HIR.

That the trap is real was confirmed *this wave*: the formatter dropped `#expand` on its first run, turning
each macro into an ordinary procedure, and gate 5 caught it on this wave's own corpus file.

### 2. The splice will reuse `#insert`'s mechanism, and will be deliberately unhygienic

A call to a macro lowers by **splicing the macro's body into the call site's scope** — the mechanism
`Stmt::Insert` already provides (ADR-0072 §1, ADR-0080). This is recorded now because it settles the
question the splice sub-wave would otherwise reopen:

- **Not the MIR inliner** (ADR-0021). That inlines a *call*, so the callee keeps its own scope — the
  opposite of what a macro needs.
- **Not a HIR body clone** (what `$T` instantiation does, ADR-0082). Same reason: the clone is a separate
  procedure with its own scope.
- **Unhygienic, matching Jai.** The statements land in the *enclosing* scope, so the body sees the caller's
  locals. That is what `#insert` already does and what makes a macro useful. PLAN §2.1 lists "hygiene" in
  W5's scope; the recommendation recorded here is to ship the unhygienic splice first and treat any hygiene
  mechanism as **its own later decision**, because a scheme designed against no use case would be designed
  blind.

### 3. A call is refused by design (E0272), and the refusal ships *with* the surface

Until the splice exists, a call to a macro is refused with a new code that names what arrives later.

**This is not merely staging — it fixes a live defect the surface would otherwise introduce.** With
`#expand` parsed and nothing consuming it, a macro *was accepted and silently ignored*: `double(21)`
returned 42 by ordinary call, with nothing to say it had not spliced. "A directive that is silently ignored
is worse than one that is rejected" is ADR-0058 §3's rule, stated there for `#no_abc` on a `#foreign`
declaration; this is its second application, and it is why the refusal is part of this sub-wave rather than
the next.

## Consequences

- **A new diagnostic code, E0272** — a call to a `#expand` macro. Lifted or reworded when the splice lands,
  exactly as E0268 was for `$T` and E0271's first meaning was for `$N`.
- A macro's declaration is checked, formatted and lowered like any procedure, so everything except the call
  is real and verified before the splice is built on it. Its body does still emit MIR (it is a well-formed
  procedure body that nothing calls); the splice sub-wave will decide whether to keep that as a fallback or
  skip it the way `is_template` skips a template.
- The corpus pins both halves: `valid/074` declares four macros — including `#expand` beside `#no_abc` in
  **both orders**, since the loop takes either — and `type-errors/068` pins the refusal.
- `#modify` and `#bake_arguments` remain unbuilt and are each owed their own decision. Neither is blocked by
  this; both are refinements of the macro this establishes.
