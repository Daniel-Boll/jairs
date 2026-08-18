# ADR-0141: A `..Any` variadic — the coercion that already composed

- **Status:** Accepted
- **Date:** 2026-08-18
- **Deciders:** dboll
- **Follow-up to ADR-0139 and ADR-0076.** The second of PLAN §7's owed follow-ups: a `..Any`
  variadic, so `print(fmt, a, b, c)` takes arguments of arbitrary types. ADR-0139 delivered variadic
  packing for a *concrete* element type and named the `Any` coercion "a small extension".
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

### Probed before writing, and it already worked — with one gap

The habit `AGENTS.md` names — confirm a wave's premise by *writing* the thing before planning around
it — applied here and paid off, for the sixth time (ADR-0067 §0, ADR-0070 §0, ADR-0072 §5,
ADR-0073 §0, ADR-0075's closing claim, ADR-0140's dump).

A `..Any` procedure `f :: (args: ..Any)` **parsed and checked** on the callee side already: ADR-0138
wraps a `..T` parameter as a `[]T` view, so the callee sees `args: []Any` and iterates it like any
other view. And at the *call* site, ADR-0139 packs the trailing arguments by checking each against
the element type through `check_arg` — which is *also* ADR-0076 §1's `*U`→`Any` coercion point. So
`f(*a, *b, *c)` erased each pointer to an `Any` and packed the three into a `[]Any` **with no new
compiler code**. The two features composed.

The gap: ADR-0139's disambiguation of the **exactly-one-trailing-argument** case (is it a packed
element or a pass-through view?) typed that single argument with no target and then compared its
natural type against the element type *directly*, bypassing `check_arg`'s coercion. So `f(*a, *b)`
coerced and `f(*a)` reported a mismatch — an asymmetry no caller could predict.

## Decision

### 1. Share the coercion decision between the one-argument and many-argument paths

The `*U`→`Any` decision — "is the wanted type `Any` and the argument a pointer with a laid-out
pointee? then record an `AnyOp::Of` coercion" — is extracted from `check_arg` into
`record_any_coercion`, which takes the argument's already-computed type. The single-trailing-argument
path calls it before falling back to the mismatch diagnostic, reusing the type it computed to decide
pass-through-vs-pack, so the argument is neither re-checked nor double-diagnosed. `check_arg` calls
the same helper. The two paths now cannot drift — the same teach-the-shared-layer discipline
ADR-0048 §2 and ADR-0044 §1 record.

**This is the whole compiler change.** MIR needed none: an argument with a recorded `AnyOp::Of`
coercion already lowers to an `Any` before packing (that is why the multi-argument case worked), so
packing a coerced single argument into a `[1]Any` is the identical path.

### 2. The argument is a **pointer** — bare values stay deferred (ADR-0076 §4)

`f(*n)`, not `f(n)`. A `..Any` erases each argument the way `any_of(*x)` and every `Any`-taking
procedure already do: through a pointer, because the `Any`'s `data` word must point at the value and
a bare value (`f(42)`) has no address. `f(42)` remains a clean E0214 (`expected Any or []Any, found
s64`), which is ADR-0076 §4's deferred bare-value→`Any`.

**Rejected: auto-spill each bare value into a temporary at the variadic slot.** It is now more
tractable than when ADR-0076 §4 deferred it — packing *already* allocates stack storage per call, so
the address the bare-value coercion lacked is at hand. But it is a genuine decision with its own
fork: whether the language silently materialises a temporary for a value passed where an `Any` is
wanted is a question about implicit temporaries that reaches beyond the variadic slot (a plain
`a: Any = 3;` wants the same answer), and settling it in passing inside a variadic wave is how a
language acquires an accidental rule. It is left for the wave that decides bare-value→`Any` as a
whole, and this ADR sharpens *why* it is now cheap to implement so that wave starts informed.

**Rejected: a distinct `..Any` element rule separate from the `Any` parameter coercion.** The point
of §1 is that a variadic `Any` slot and a scalar `Any` parameter are *the same boundary*; giving them
two coercion rules would be two chances to disagree about what `*T` into `Any` means.

## Consequences

- **1010 workspace tests unchanged; 227 → 228 corpus files.** `valid/114` exercises the empty
  variadic, a single pointer argument (the case the fix repairs), several arguments of one type,
  genuinely mixed types in one call (an `s64`, a `Point` and a `bool`, each discriminated by
  `args[i].type.id` and read with the matching `any_as` — the `print` use case), and the explicit
  `[]Any` pass-through. Both engines agree, which the differential harness pins; a wrong `Any`
  aggregate would diverge exactly as ADR-0116's wrapping-overflow bug did.
- **The compiler change is one shared helper and one call site.** No new diagnostic code, no MIR
  change, no grammar change. The wave is almost entirely the corpus file that proves the composition,
  plus the repair of the single-argument asymmetry.
- **Deferred, unchanged**: bare-value→`Any` (§2), which `..Any` now shares a clear implementation
  path with but not a decision.
