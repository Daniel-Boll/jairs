# ADR-0162: `#c_variadic` — a marker, and a refusal instead of a silent miscompile

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **The first half of ADR-0157 §2's two**, and the last item PLAN §8.5 lists in front of W10 besides the
  aggregate boundary ADR-0161 opened.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

ADR-0157 §2 found that a fixed-arity `#foreign` declaration of a **variadic** C function passes its extra
argument in the wrong place, silently: `open(path, flags, mode)` created a file with permissions `---------x`
on arm64 macOS, with no diagnostic from either engine and a file that existed and could not be read. Variadic
arguments travel on the stack there; a fixed third argument travels in a register.

It named two separable halves — **refuse** it, and **support** it — and this delivers the first. PLAN §8.5
lists C-variadic calls as W10's other gate, because `objc_msgSend` is variadic.

## Decision

### 1. `#c_variadic` is a marker, and its absence means "not variadic"

`f :: (a: *u8, b: s64) -> s64 #c_variadic #foreign libc "f";` says the declared parameters are the **fixed**
ones and the C declaration ended in `...`.

**The compiler cannot learn this any other way.** A Jairs signature says what the callee takes; it cannot say
that C permits more, and no amount of analysis recovers a fact that only the C header holds. So it is a
marker, and it goes through the mechanism ADR-0151 built: a `ProcAttr` variant, which the parser's attribute
loop matches **exhaustively** — so adding it was a compile error at the one site that had to change, exactly as
that mechanism's docs promise.

**Absence means not variadic**, which is the safe default: a caller who omits it gets the fixed-arity call
they wrote. The honest consequence is stated rather than glossed: this turns a **known** variadic into a
diagnostic and leaves an **unknown** one exactly where ADR-0157 found it. Detecting a missing marker needs the
C declaration, which this compiler never sees.

**Rejected: inferring it from the symbol name.** A table of "libc functions that are variadic" would be a
guess that looks like knowledge, wrong for any library but libc, and stale the moment a platform changes.
**Rejected: making it mandatory on every `#foreign` declaration**, which would be a mechanical change to every
existing binding for a fact almost none of them need.

### 2. A call is refused — E0289 — and the declaration is legal

The two are split deliberately. **Declaring must be legal on its own**, so a library author can annotate
`printf` today: the day a variadic call becomes possible their binding is already right, and until then their
caller gets a diagnostic instead of a wrong answer. A marker whose only effect is an error is a marker nobody
can adopt incrementally.

The call is refused because **Cranelift has no variadic calling convention**. Its `Signature` has no notion of
a variadic boundary at all — probed, not assumed — so every declared parameter is placed by the fixed-arity
rules and there is no way to express "the arguments past the second belong on the stack".

**Rejected: supporting it in the VM and LLVM and refusing only in Cranelift.** libffi has a variadic CIF and
LLVM has variadic function types, so two of the three engines could do it today. That would make `jr build`
fail where `jr run` succeeds, which breaks the premise the whole differential harness rests on — and it would
be a *diagnostic* divergence rather than a silent one, which is better than nothing and worse than a uniform
refusal. Two engines agreeing is not the property this project maintains; three are.

**Rejected: emitting the stack arguments by hand.** A call could in principle be built by writing the variadic
arguments to a stack area and calling with the fixed ones, but Cranelift chooses the stack layout, so the
compiler would be guessing at an offset the register allocator owns. That is the same class of guess ADR-0160
§3 refused for a mixed struct.

The diagnostic names a way forward rather than only a limit: `creat` for `open`, `vsnprintf` with a prepared
`va_list` for `printf`. `File` already took that route (ADR-0157 §2), which is the evidence that the advice is
practical.

### 3. Checked in the name path, and the cross-module hole is named

The refusal fires when the procedure's **name** is used, not only at a call, because being uncallable is the
whole fact and a binding nobody calls yet should still say so at its use.

An **imported** `#c_variadic` procedure is *not* detected, and this is a real hole rather than an oversight.
`is_foreign_proc` can ask an imported procedure's *type* — `ContextKind::CCall` is what `#foreign` means — but
variadicity is not in the type and nothing puts it in the pool. Recorded here and in PLAN: a module that
declares one is the module that knows, so the refusal belongs on the declaration side for the cross-module
case, and that is its own change.

### 4. The formatter trap fired again, for the eleventh consecutive wave

`jr fmt` **silently deleted** `#c_variadic` on the first attempt. Round-tripping and idempotence both passed —
a formatter that re-emits `node.text()` verbatim passes both — and the attribute simply vanished.

This is the eleventh consecutive wave in which a new construct had to be taught to `jr-fmt`'s attribute loop,
and it is the **most unsound** direction yet: `#c_call` changes a convention and `#must` deletes a check, while
dropping this one turns a variadic call back into the fixed-arity call whose miscompile started this ADR. A
formatter run would undo the fix.

Caught by a round-trip check on this wave's own file, which is what gate 5 is for. The lesson is unchanged
after eleven repetitions and is worth restating: **a new node kind must be added to the emitter, and
round-trip assertions do not prove it was**.

### 5. Two fixtures, split by which half they assert

`valid/131` asserts the *positive* half — a marked declaration compiles, formats, highlights, and does not
disturb its neighbours. It declares `open` marked beside `creat` unmarked and checks that the unmarked one
still calls with its ordinary convention, which is the property a mis-recorded attribute would break.

`type-errors/080` asserts the refusal, and getting it to exactly **one** diagnostic was instructive: the
refused call has an `ERROR` type, so binding it added E0257 and a bare `null` argument had no context to take
a pointer type from. A refusal that poisons its expression makes every neighbour speak up, which is worth
knowing before writing the next one.

## Consequences

- **`#c_variadic` parses, formats, highlights and lowers**; `E0289` refuses a call, and `E0290` is the first
  free code.
- **W10's second gate is now a stated limitation rather than a silent miscompile**, which is the same
  conversion ADR-0150 made for the aggregate boundary one wave before ADR-0161 opened it. `objc_msgSend` is
  still not callable, and now says so.
- **1054 tests** (1055 under gate 7), **253 corpus files**, **170 Neovim checks**.
- **`jr-hir::Proc` gains `c_variadic`**, filled at five sites the compiler located by refusing to build —
  which is the argument for a field over a side table.
- **Supporting the call remains open**, and its blocker is upstream: Cranelift would need a variadic
  signature. Recorded in PLAN rather than estimated.
