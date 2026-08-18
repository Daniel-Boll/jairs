# ADR-0139: Variadic packing — the call-site sugar

- **Status:** Accepted
- **Date:** 2026-08-18
- **Deciders:** dboll
- **Follow-up to ADR-0138.** Wave 8 delivered the declaration surface for `args: ..T`; this ADR
  delivers the call-site half — `print(fmt, a, b, c)` packing its trailing arguments into a
  stack view. Together the two ADRs make the eight-wave programme's Wave 8 complete.
- No design fork was put to the decider. ADR-0138 §1 already recorded the shape:
  *stack-allocated array of the exact trailing-arg count + a view over it, one call site at a
  time.*

## Context

### What ADR-0138 left owed

ADR-0138 shipped `..T` as a declaration marker: the parser accepted `args: ..T`; the callee
saw the parameter as a `[]T` view. A caller with an explicit view worked; a caller with the
sugar `print(fmt, a, b, c)` was refused with a specific E0216 pointing at "the follow-up wave
that turns the trailing args into a stack view". This is that wave.

## Decision

### 1. Pack per call site — one stack array, one view slot

At each variadic call, MIR:

1. Allocates a stack slot of type `[N]T`, where `N` is the trailing-argument count and `T` is
   the element type recorded in `variadic_calls`. Zeroed for definedness (Statement::Zero),
   because a partial write would leave the tail uninitialised and the callee's `for x: args`
   would read poison.
2. Stores each trailing argument's operand into `array[i]`.
3. Takes `&array[0]` — through `Rvalue::Address(Place::Index(0))`, so the pointer type is
   `*T` and the view's stride is the element's, matching how `xs[0..n]` slices work today
   (ADR-0044 §4's shared path).
4. Allocates a `[]T` view slot, zeroes it, and stores `data` and `count = N` through the
   view projections.
5. Loads the view and appends it as the call's last operand, in place of the trailing
   operands.

**Rejected: heap-allocate the array via `context.allocator`.** A stack array has the right
lifetime — until the call returns — and needs no cleanup. The callee cannot outlive the call
frame; a heap array would need an owner-free rule and would be a real cost for a construct
whose whole appeal is convenience.

**Rejected: one shared array reused across calls.** Two consecutive variadic calls at different
sites could write the same storage and the second would clobber the first's live view.
Per-call storage is the price of not caring about lifetimes.

### 2. `variadic_calls` records "pack or not" — the recording is what MIR keys on

Sema records `(fixed_arg_count, element_ty)` in `variadic_calls` when a call needs packing.
`ConstValues::set_variadic_call` threads it to MIR, and `call_rvalue` reads it. Two shapes
share the surface — sema disambiguates:

- **Exactly-one trailing arg**: type the arg with **no target** (so no mismatch diagnostic
  fires), then decide by the natural type. If the natural type is the view type `[]T`, the
  arg is a pass-through — no packing, no record. Otherwise pack, and enforce that the
  natural type is the element type (E0214 otherwise).
- **Zero or many trailing args**: always pack. A `sum()` with no args packs the empty view;
  a `sum(1, 2, 3)` packs three. Each trailing arg is checked against the element type.

**Rejected: type the arg twice — once against the view type, once against the element.** The
first check would fire E0214 for every literal argument (`sum(42)` types `42` against `[]s64`
and reports a mismatch), and re-checking after the first failure would duplicate messages.

**Rejected: refuse the exactly-one-trailing case entirely and require callers to
disambiguate.** Users would then write `sum([]s64{42})` or similar, which turns a convenience
into a boilerplate. The natural-type disambiguation gets there without a syntactic tax.

### 3. `sum()` — the empty variadic — is recorded too

A call with zero trailing arguments still records a variadic-pack entry (with `N=0`), so
`call_rvalue` builds the empty view rather than passing no operand for the variadic slot.
Without this, `sum()` would arity-mismatch inside MIR — the callee's parameter list expects
one view — while sema had already accepted the call.

The empty case allocates a zero-length array. `Statement::Zero` on `[0]T` is a no-op, and
`Rvalue::Address(Place::Index(0))` on it produces the array's own address, which is a valid
`*T` for a view of length 0 to point at.

## Consequences

- **Wave 8 is now fully delivered.** The eight-wave programme (ADR-0128–0138 plus the two
  follow-ups ADR-0135 and this ADR) is *effectively complete*: every promise in ADR-0127 §3
  is kept, and the two deferrals recorded in the programme (`for x, i: a..b` in ADR-0133 §2
  and packing here in ADR-0138 §1) are closed.
- **1010 workspace tests unchanged; 225 → 226 corpus files.** `valid/112` exercises zero,
  one, several, mixed fixed+variadic, and pass-through view.
- **The MIR snapshot moved** for the two variadic corpus files — the packing emits four new
  statements per call site (Zero array, N Store, Address, Zero view, two Store view, Load
  view), which is the whole shape of this ADR's runtime cost.
- **Deferred**: the compiler-known `Any` variadic — `print(fmt, ..Any)` — needs each argument
  to be *coerced* to an `Any` before packing. ADR-0076 §1 already coerces a `*T` to `Any` at
  a boundary; extending that coercion to a variadic slot is a small follow-up when a caller
  actually needs it. This ADR ships variadic packing for a *concrete* element type, which is
  the shape `sum(..s64)` and `printf_i64(..)` need today.
