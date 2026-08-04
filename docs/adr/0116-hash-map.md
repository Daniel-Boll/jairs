# ADR-0116: `Int_Map` is an open-addressed hash table — and the wrapping operators overflowed `i128`

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W7 sub-wave 14.** The module that most exercises the earlier sub-waves, and it found a comptime miscompile in
  the wrapping arithmetic operators — the second engine divergence the corpus differential has caught.

## Context

A hash table needs a heap array of structs, grown, with keyed lookup. Probed first: `typed(Slot, malloc(n *
size_of(Slot)))` and field access through pointer arithmetic (`(slots + i).key`) work in both engines. So the
storage is exactly what typed allocation (ADR-0106) and `List`'s growth (ADR-0107) already built.

## Decision

### 1. Concrete `Int_Map`, open-addressed, linear-probing

`s64 -> s64`, concrete for the reason `Int_Array` and `Int_List` are (E0269, ADR-0085 §5): a `Map($K, $V)` in a
module is unusable by every importer until cross-file parameterised structs arrive.

**Open addressing** because one heap allocation holds every slot and the probe sequence is plain arithmetic — so
both engines walk it identically, which the differential harness needs. Chaining would need per-node allocation
for no benefit. **Grows at 3/4 probe load**, doubling and rehashing, because linear probing degrades sharply
past that — correctness-adjacent, not just speed. **Deletion by tombstone** (a slot a probe skips but an insert
may reuse), reclaimed by the next rehash, because clearing a slot would break a probe sequence that ran through
it.

**The hash is `Basic`-free `u64` arithmetic** — a Fibonacci multiply and a xor-shift — so both engines compute
the same bucket with no FFI. A negative key is cast to `u64` first, so its sign bit mixes rather than breaking
the modulo.

### 2. The wrapping operators computed in `i128` and overflowed — a comptime miscompile

`bucket`'s hash is `h *% CONST`, a **wrapping** multiply (`*%`), because a hash deliberately discards the bits
that overflow — plain `*` traps (ADR-0002). But `int_binary` decodes its operands to `i128` and computed
`WrapMul` as `out.wrap(a * b)` — and for two `u64` operands near the top of the range, `a * b` is ~2^128, which
**overflows `i128` itself**, panicking in the debug build *before* `wrap` could take the low bits.

Native code has no `i128` intermediary — it multiplies in the machine's 64-bit register and wraps — so it was
correct while the VM panicked. That is an **engine divergence**, and the corpus differential caught it: the map
test exited 101 (a VM panic) against 255 (native).

The fix is that the wrapping forms do their arithmetic on the truncated `u64` values with Rust's `wrapping_add`
/ `wrapping_sub` / `wrapping_mul`, which is exactly "keep the low bits, discard overflow" — the semantics the
`*%` family promises. The checked forms (`+`, `*`) still use `i128` and `check`, because their whole job is to
*detect* the overflow the wrapping forms discard.

This is separable from the module and committed on its own, with a `#run` test that multiplies two large `u64`s
at compile time — the smallest reproduction, independent of the hash table that surfaced it.

## Consequences

- **A working hash table exists**, and `valid/095` exercises it through several rehashes (fifty inserts), a
  tombstoned deletion that leaves a later key findable, a negative key, and absence — every bit depending on the
  two engines computing the same buckets and probe paths.
- **A comptime miscompile in `*%`/`+%`/`-%` is fixed**, which would have hit *any* wrapping arithmetic near the
  type's range evaluated at compile time — a `#run` hash, a checksum, a PRNG folded into a constant. It was
  reachable since wrapping operators existed (ADR-0002) and nothing had multiplied two large operands at
  comptime.
- **The second differential catch**, after ADR-0107 §2's `malloc`-in-a-callee. Both were one engine right and
  the other wrong, which is the failure two independent implementations exist to expose — and both were in
  arithmetic or memory the native path handled in hardware while the VM modelled it in Rust, where the model was
  subtly off.
- **No new diagnostic code.** The fix is to arithmetic, not a refusal.
- **Deferred**: a generic `Map($K, $V)` (cross-file parameterised structs); a string-keyed map (a byte-walking
  hash, additive); iteration.
