# ADR-0111: `String`'s allocating half uses the context allocator, and the caller frees

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W7 sub-wave 9.** ADR-0103 §3 shipped `String`'s non-allocating half and **deferred the allocation
  convention**, because settling it in passing is how a library acquires an accidental one. This settles it, and
  builds `concat`, `substring`, `to_upper`, `to_lower`, `free_string` on top.

## Context

The routines that produce a *new* string need somewhere to put it. The mechanism was never missing —
`context.allocator` is a real protocol (ADR-0062: two procedure pointers and a state word) and `talloc` is a
real arena (ADR-0065). The **choice** between them was, and ADR-0103 §3 named the three candidates.

## Decision

### 1. Allocate through `context.allocator`; the caller frees with `free_string`

- **Not `talloc`-always.** A result that silently expires on an unrelated `reset_temporary_storage()` is a trap:
  a caller who kept a string across a reset reads freed memory with nothing to warn them. And it is strictly less
  capable — a caller who *wants* arena behaviour installs `talloc` as the context allocator and gets exactly it.
- **Not an explicit allocator parameter.** It doubles every signature for a choice a caller makes once, and the
  context exists to carry precisely this (ADR-0001): install an allocator, and every `String` routine uses it
  with no second API.
- **Not a caller-supplied buffer only (`concat_into`).** That is the right shape for a hot path and is additive
  later, but it cannot be the only form — a caller who does not know the result length in advance would have to
  compute it separately, which is `concat`'s job.

**A caller must install an allocator first, and forgetting is not silent.** `context.allocator` is null until
installed (ADR-0057 §5), and calling a null one **traps** (ADR-0110). That trap shipped the sub-wave before this
one, found by probing this very convention — so a program that concatenates without installing an allocator gets
a sentence naming the null pointer, not a wrong answer.

**A failed allocation returns `""`.** ADR-0058 §4's line is that a trap is for a *program* error, and running out
of memory is not one. `""` is distinguishable from every non-empty result, and a caller joining two non-empty
strings and getting `""` back has detected the failure.

### 2. `free_string`, symmetric and safe on empty

A facility that can allocate and not free leaks by construction (ADR-0106's argument for `untyped`). `free_string`
is safe on a `""` result — its data is null and `allocator_free` of null is a no-op — so a caller need not
special-case the empty case. It must be given a string from an allocating routine, not a literal: freeing a
literal's storage would release memory the compiler owns, which the module docs state because nothing enforces
it.

### 3. Small deliberate limits

`to_upper`/`to_lower` are **ASCII only**, said plainly: a `string` is bytes (ADR-0004), and case-folding beyond
ASCII needs a Unicode table the language does not have. `substring` **clamps** an out-of-range request rather
than trapping — "up to n bytes from here" is an ordinary thing to want at the end of a string, the reasoning
`byte_at`'s `-1` follows.

`make_string` and `copy_bytes` are module-private: a caller has no business building a `string` from raw parts,
and one shared copy loop means a bug in it is fixed once.

## Consequences

- **The two halves compose.** `valid/090` builds strings with the allocating routines and checks them with
  `equal` from the non-allocating half, freeing every result — so the differential harness catches a leak, a
  double-free, or a wrong copy as a divergence in one engine. Teeth-checked: dropping `concat`'s second copy
  clears bits 1 and 2 (255 → 252).
- **No new diagnostic code, no compiler change.** This is a library sub-wave built entirely on what the language
  already had — the first W7 sub-wave in several that touched no compiler crate, which is what a maturing
  language should let a library do.
- **`split` stays deferred**, and now with a sharper reason: it wants a *list of strings*, and a `List($T)`
  needs cross-file parameterised structs (ADR-0085 §5), still the gap between the standard library and full
  generics. `concat_into` is deferred as additive with no caller yet.
