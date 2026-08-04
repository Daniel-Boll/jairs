# ADR-0093: `#modify { … }` is a compile-time predicate over an instantiation — the surface

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W5 sub-wave 7d.** The surface: the block parses, formats, and its text is carried on `Proc::modify`. A
  call to a `#modify` procedure is refused (E0274) pending evaluation — the staging `$T` (ADR-0081), `$N`
  (ADR-0087) and `#expand` (ADR-0090) each used.

## Context

`#modify` is a predicate over an *instantiation*: it runs at compile time when a call binds the template's
type variables, and returning `false` rejects that call. So a template can say "only for an `s64`", or "only
for a struct with at least two fields", in code rather than in a comment.

Premise verified by running: `#modify { … }` after a signature was **E0106** ("expected a procedure body"),
so it is a real feature.

**Designing it found a bigger gap.** A predicate must be able to *ask something* about the bound type — and
`type_info(T)` inside a `$T` body was E0261, so a `$T` procedure could not reflect on its own parameter at
all. That gap is more valuable than `#modify` and on the same path, so it was fixed first: **ADR-0092** landed
before this ADR despite being nobody's plan. A predicate can now write
`type_info(T).id == type_info(s64).id`.

## Decision

### 1. `#modify` is a procedure attribute that carries a block

The parser's attribute loop takes `#modify` beside `#c_call`, `#no_abc` and `#expand`, so the four may be
written in any order — but this one **parses a block**, the predicate's code. `MODIFY_ATTR` is its own
`SyntaxKind` and `modify_attr` its own grammar rule (with a `predicate` field), for the reason the other
three are separate: a consumer that forgets it is a *missing arm* rather than a silent fall-through.

`looks_like_proc_signature` needed `#modify` too — the token-set trap for the **sixth** time, since a
procedure whose signature is followed by `#modify` reaches neither `ARROW` nor `L_BRACE` when the return type
is omitted.

`Proc::modify: Option<String>` carries the block's **source text**, for the reason a macro's body is text
(ADR-0091 §1): it is evaluated *per instantiation*, against that instantiation's bindings, and lowering it
once against the template would resolve `T` where nothing binds it.

### 2. Evaluation, designed here and deferred

The predicate becomes **its own appended procedure per instantiation**: body = the block's text, no
parameters, returns `bool`, and the same `proc_bindings` entries the instantiation gets — so `type_info(T)`
inside it describes the bound type (ADR-0092 §1). `jr-db` then evaluates it as a `#run`-shaped target, which
is why **no new query is needed**: `file_consts` already has that machinery, and ADR-0088's pre-pass already
demonstrated evaluating something per instantiation.

Attempting it in this sub-wave showed why it is its own: it needs `FileHir::modify_predicates` pairing each
predicate with the instantiation it guards, and a way to lower a body *from text* outside `LowerCtx` — which
owns the arenas — so body lowering has to be exposed. That is an API change, and a half-built version would
leave a predicate parsed and unevaluated, which §3 is precisely about refusing.

### 3. A call is refused by design (E0274), and the refusal ships *with* the surface

Until the predicate is evaluated, a call to a `#modify` procedure is refused — **before** the instantiation
is recorded, because instantiating would mean the predicate was parsed and then silently ignored: a `#modify`
that should reject a call would accept it.

"A directive that is silently ignored is worse than one that is rejected" is ADR-0058 §3's rule, and **this
is its third application** — after `#no_abc` on a `#foreign` declaration and `#expand` (ADR-0090 §3). That
the same rule keeps applying is why it is worth having stated once.

## Consequences

- **A new diagnostic code, E0274.** Lifted when the predicate is evaluated, exactly as E0268, E0271's first
  meaning and E0272's first meaning were.
- The formatter emits `#modify` **with its block** — dropping it would delete a compile-time guard, so the
  program would accept instantiations the author rejected. That is the *unsound* direction, like `#c_call`
  and `#expand`.
- `valid/077` declares three guarded templates — an identity predicate, a reflected-field-count predicate,
  and `#modify` beside `#no_abc` — and `type-errors/068` pins the refusal.
- `#bake_arguments` remains the last of W5's macro family, and it is unaffected by this.
