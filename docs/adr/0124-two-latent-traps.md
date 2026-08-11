# ADR-0124 — Two latent traps closed structurally

**Status:** Accepted
**Date:** 2026-08-07
**Amends:** nothing behavioural. Both changes are preventive; neither alters what any program does today.

## Context

The audit at `354d900` ([`docs/assessment-2026-08-07.md`](../assessment-2026-08-07.md)) found two
hazards that were *not* live defects and were both one small change away from becoming ones. They are
grouped here because they are the same kind of thing — an invariant this project relies on that
nothing enforces — and because neither is worth a wave alone.

### The attribute token-set trap (finding F9)

`Parser::looks_like_proc_signature` decides whether `f :: (…)` begins a **procedure** or a
parenthesised-expression **constant**. For a procedure with no `->`, the only thing that can tell it
apart is the attribute between the parameter list and the brace. So the lookahead carried a list of
attribute directives, and the loop that *consumes* them carried a second, unlinked list.

This project has recorded **seven** separate bugs from that one duplication, and the comments at the
site count them. Each was the same: an attribute added to the loop and not to the lookahead, after
which every procedure carrying it was read as a parenthesised expression. `#expand` produced
**fourteen cascading errors**, none of which pointed at the attribute.

A shared `&str` list would fix the symptom. It would not fix the mechanism, because a string match
cannot be made exhaustive — which is precisely why seven of these got through per-crate review.

### `type_bindings` leaking into an imported struct's fields (finding F4, §4)

`resolve_instance_fields_in` resolves an imported parameterised struct's fields under the caller's
type arguments (ADR-0117). Its own doc comment states the invariant: "a struct's fields cannot depend
on who imported it."

That was true only by accident. `resolve_type_name` consults `type_bindings` **before** the declaring
module's signatures — ADR-0081 §1's "a bound variable wins over everything" — and the caller
(`instantiate_parameterised`) saves and restores only the struct's **own** `poly_vars`. Any other
binding in scope leaked through. So a field whose type names something the declaring module declares,
where that name collides with a type variable the *importer* has bound, would resolve to the
importer's type — and `set_instance_fields` caches the result for every later user of that instance.
Silent wrong type, wrong layout, no diagnostic.

**It is not reachable today**, and the reason is worth recording because it is not the reason one
would guess: making an instance resolve while a foreign binding is in scope requires giving it a type
argument that depends on one, and `Box(T)` for a bound `T` is **E0212** — inference through a
parameterised struct is deferred (ADR-0085 §5). The audit probed both shapes and got a clean check and
an E0212 respectively. So the invariant is held by an *unrelated* refusal, and would break the day
that refusal lifts.

## Decision

### 1. Procedure attributes are an enum, not two lists of strings

`ProcAttr` has one variant per attribute, with `text()`, `from_text()` and `ALL`. The consuming loop
matches on it **exhaustively**, and the lookahead calls `from_text` rather than restating anything.

Adding an attribute is now a **compile error** at every site that must change. Teeth-checked by adding
a fifth variant: two errors, at the loop's match and at `text()`.

`#foreign` is deliberately **not** a `ProcAttr`. It stands where the body goes, so it belongs to the
body arm rather than the loop — and it is why the lookahead's list was never quite the attribute list.
It is a separate constant, and a test pins the distinction, because a future tidy-up that folded it in
would make the loop consume it and leave the procedure bodyless.

Two tests: every attribute ends a signature and is consumed, using a **void** procedure so the
lookahead reaches neither `ARROW` nor `L_BRACE` — the exact shape all seven bugs took, and one a test
written with `-> s64` would pass with the lookahead completely broken.

Not adopted: the lexer's directive list (`lexer.rs`) was reported as a third copy. It is a *lexer
test* asserting that directives lex uniformly, and its comment says adding a directive must never
require a lexer change. It is not a copy of the attribute set, and is left alone.

### 2. `resolve_instance_fields_in` narrows the bindings to the instance's own

The whole `type_bindings` map is taken with `mem::take` and repopulated with only the struct's own
`poly_vars`, then restored. Exact rather than a filter, because a filter would have to know which
names matter — which is the judgement that was wrong in the first place.

The `poly_vars` are read from the **declaring** file's HIR, not `self.hir`. `sid` indexes the
declaring file's arena, so asking the importer's indexes a different one: it panics outright when the
importer has fewer structs, and would silently read a *different declaration* when it has more. The
first draft of this change made exactly that mistake and the corpus caught it immediately — which is
worth recording, because the wrong version is the one that reads more naturally.

Rejected: *reordering `resolve_type_name` to consult the declaring module's signatures before the
bindings.* It would fix this case and break ADR-0081 §1, under which a bound `T` must win over a
same-named type inside a signature. The ordering is right; the leak was in what was in scope.

Rejected: *waiting until it is reachable.* It costs three lines now and a debugging session later, and
the invariant is already written in the function's own documentation — where it was, until now, false.

## Consequences

No behavioural change: 1005 → 1007 tests, all of them new checks rather than changed expectations, and
the whole corpus unmoved.

`ProcAttr` makes the eighth token-set bug impossible rather than unlikely. The field-binding narrowing
makes ADR-0117's stated invariant structural, so lifting ADR-0085 §5's deferral — which is on the list
of three remaining polymorphism unblockers — will not quietly reintroduce a wrong-layout cache.

**What this does not do.** It does not consolidate `jr-hir`'s and `jr-db`'s codes into `code.rs` files,
which `AGENTS.md` still asks for and which ADR-0123 downgraded to tidiness. And the audit's F4 sibling
— `check_polymorphic_call` *removing* inferred bindings rather than restoring what they shadowed
(`check.rs:4413-4429`, unlike the correct idiom at `ctx.rs:678-692`) — is **not** fixed here. It is
masked by the same E0212 deferral and wants the same treatment; it is recorded in `PLAN.md` §7 rather
than bundled in, because it sits in a different function with a different caller contract.
