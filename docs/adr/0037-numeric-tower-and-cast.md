# ADR-0037: the numeric tower is a naming change; `cast(T, x)` is Jai's, checked at comptime

- **Status:** Accepted
- **Date:** 2026-07-28
- **Deciders:** dboll

## Context

The vertical slice is done. W1 opens, and `PLAN.md` §2.1 scopes it as the full numeric tower,
`float32/64`, wrapping ops, `enum`, `enum_flags`, `union`, `[N]T`, `[]T`, `[..]T`, `cast()`,
`xx` and operator overloading — eight to ten weeks. This ADR takes the first cut of that:
**the integer tower, `cast(T, x)`, and `print_int` written in Jairs.**

That cut is chosen because integer printing has been impossible since the slice began.
`PLAN.md` §1.2 carries a `[!CAUTION]` block explaining why: turning a digit into a byte for
`write` needs an `s64` → `u8` conversion, and `cast` was reserved for W1. So the slice's exit
criterion prints strings only, and `print_int` is the marker §7 named for this wave. It is
also the smallest cut that produces something visible rather than internal.

Four facts were established by reading the code before this ADR was written, and they decide
the shape:

- **`IntKind` is already generic over width and signedness.** `jr-pool`'s `arith.rs` carries
  `IntKind { signed: bool, bits: u16 }` with `mask`, `min`, `max`, `decode`, `wrap` and
  `check` all computed from `bits`. Nothing in it is specialised to 64 or 8.
- **Both back ends already read the width generically.** `jr-codegen-clif`'s `repr.rs` matches
  `Item::IntType { signed, bits }`; `jr-vm`'s `interp.rs` and `ffi.rs` go through
  `IntKind::of`. Neither has a list of supported widths.
- **Interning is structural and on demand.** `Pool::intern` dedupes on the `Item`, so
  `Item::IntType { signed: false, bits: 16 }` becomes a `PoolId` the first time anything asks.
  The well-known prefix pre-interns `s64` and `u8` only because ADR-0004's string layout and
  the libc `write` signature reach them before user code does.
- **Only four sites name the builtin types**, and all four are string matches:
  `jr-sema`'s `resolve_type_name` and its two diagnostic helpers, and `jr-lsp`'s
  `BUILTIN_TYPES`.

So the tower is not a representation change. It is a **naming** change, and the plan's sizing
of W1 reflects the whole wave rather than this part of it.

## Decision

### 1. The tower is ten names mapped onto `Item::IntType`, with no new `PoolId` constants

`s8 s16 s32 s64` and `u8 u16 u32 u64` resolve by parsing the name into `(signed, bits)` and
interning. `s64` and `u8` keep their pre-interned constants because the prefix's indices are
asserted by a test and are load-bearing for `PTR_U8`; the other six are interned on first use
like any other type.

**Rejected: a `PoolId` constant per width.** Symmetrical, and it would make the well-known
prefix the one place the tower is written down. Rejected because the prefix exists for types
reached *before* user code (ADR-0015), and `u32` is not one — adding six constants would grow
every pool in the system for types most programs never mention, and would renumber the prefix
that a test pins and `PTR_U8` depends on.

**Rejected: parse the name with a regex or a `strip_prefix` at each site.** The mapping lives
in one function in `jr-pool`, `IntKind::from_name`, so that `jr-sema`, `jr-lsp`'s completion
list and any future consumer cannot disagree about which names exist. Four sites currently
hardcode the list and three of them are diagnostics — exactly the drift ADR-0022 §2 refuses
for arithmetic.

**Not in this wave: `float32`/`float64`.** They are the one part of the tower `IntKind` cannot
absorb — a new value representation in the pool, the VM and Cranelift, plus literal parsing the
parser refuses today with E0120. Left to its own wave, and the parser's refusal message
already says W1, which stays true.

**Not in this wave: bitwise operators.** `& | ^ ~ << >>` are refused with E0122 and are their
own feature; the tower does not need them and `print_int` does not either.

### 2. `cast(T, x)` is Jai's form, and a literal that does not fit is a compile error

```jr
cast(u8, 300)        // error: 300 does not fit u8
cast(u8, n)          // truncates at runtime
cast(s64, some_u8)   // widens, always safe
```

The syntax is Jai's, per `PLAN.md` decision #1. `cast` is already a keyword the lexer produces
and the parser refuses with E0121 "arrives in wave W1", so this wave makes an existing
reservation good rather than adding surface.

**A narrowing cast of a literal is rejected at compile time.** This is the same rule ADR-0016
§1 already applies to `x: u8 = 300;`: an integer literal has no intrinsic type, it takes the
type of its context, and a literal that does not fit its context is E0204. A `cast` supplies a
context, so `cast(u8, 300)` is the same error about the same source text and reuses the same
code.

**A narrowing cast of a runtime value truncates**, silently, as C does.

**Rejected: trap on a narrowing cast at runtime.** Tempting, and arguably more consistent with
ADR-0002, which makes integer *overflow* always trap. Rejected on the distinction ADR-0002 is
actually about: an overflowing `+` is a computation whose result the program did not ask for,
while a narrowing `cast` is the program explicitly asking for the low bits. Trapping would
also cost a branch on every cast and would leave no way to write the truncation `print_int`
needs. Jai does not trap here either.

**Rejected: `cast` truncates and `checked_cast` traps.** Two operators, explicit at the call
site, and more honest than either alone. Rejected as a language-surface decision Jai did not
make, on a wave whose job is to make an existing keyword work — and because nothing in the
corpus yet wants the checked form. When something does, it is a new ADR.

**Not in this wave: `xx`.** Jai's autocast infers the target from context, which means it is a
*sema* feature rather than a syntax one and interacts with every context-typing rule ADR-0016
fixed. `cast` is explicit and self-contained; `xx` earns its own decision.

### 3. `cast` is a call-shaped expression whose first argument is a type

`cast(T, x)` parses as its own expression node rather than as a call to a procedure named
`cast`, because its first argument is a *type* and Jairs has no way to pass one as a value in
a call (ADR-0012 makes a struct name a constant of type `type`, but a procedure cannot take
one until W4's RTTI).

The consequence, stated so it is not discovered: `cast` is not a name that can be shadowed,
completed, hovered or renamed. It is syntax. `prepareRename` already refuses a keyword
(ADR-0030 §3), and this is one.

### 4. `print_int` goes in `modules/Basic`, and it is the wave's acceptance test

Written in Jairs, dogfooding the tower and the cast exactly as `PLAN.md` §2.0's per-wave
checklist requires. It must produce identical output in the VM and the native back end,
asserted by `differential.rs` — which is what makes this wave's claim checkable rather than
plausible.

`print_int` needs a digit buffer, and `[N]u8` is not in this cut. It is therefore written by
recursion, emitting one byte at a time through `write`: correct, obviously slow, and honest
about which feature it is waiting for. A comment says so, because the alternative is a reader
assuming recursion was a choice.

## Consequences

- **No representation changes.** `Item::IntType` already carries what is needed, both back ends
  read it, and the differential harness already covers both engines. The risk in this wave is
  in *sema* — context typing, the fit check, and the cast's own rules — not in codegen.
- **`IntKind::from_name` becomes the one list of integer type names**, and `jr-lsp`'s
  completion reads it rather than repeating it. A name added there appears everywhere at once.
- **The corpus grows a file per rule**, per `AGENTS.md`: the tower's widths, a widening cast, a
  truncating cast, and a literal that does not fit. The last one is a `type-errors/` file, so
  the E0204 wording is snapshotted.
- **`print_int`'s recursion is a placeholder for `[N]u8`**, not a design. It is the shape
  `PLAN.md` §1.2's caution block predicted, one wave later and with the cast that unblocks it.
- **The parser's "arrives in wave W1" message for `enum`, `union`, `xx` and `null` stays true**,
  and stays accurate for `cast` no longer. That refusal is removed for `cast` alone; leaving it
  listed would be the plan contradicting the code.
