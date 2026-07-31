# ADR-0069: a `#run` may call an imported procedure and appear in a body, and W4 is split into sub-waves

- **Status:** Accepted
- **Date:** 2026-07-31
- **Deciders:** dboll
- **Opens W4 — Comptime**, the wave PLAN.md §5 names the project's top risk. §0 splits it, because a
  10–14 week wave attempted whole is how a handoff stops being checkable.
- **Amends nothing.** ADR-0016 §4 said `#run` "has the type of `e` and is not folded — the value arrives
  when the VM does"; this is that arrangement reaching two positions it did not reach before.

## Context

### 0. W4 is split, and this is its first sub-wave

§2.1 gives W4 six deliverables — arbitrary `#run`, aggressive const folding, RTTI (`Type` values,
`type_info()`, `Any`), `#insert`, `#code`, the `Code` type — and 10–14 weeks. Every other wave in this
project has been one ADR and one branch. A wave five times that size cannot be verified the way the
others were: the handoff at the end of it would be a claim about work nobody could re-run in a sitting,
which is exactly how §7 "rots toward *what remains is small*".

So W4 becomes a sequence, each its own ADR and each shippable:

1. **`#run` reaches across files, and works in a body** — this ADR.
2. **Aggressive const folding** — folding a `#run` result into the MIR that consumes it.
3. **RTTI** — `Type` values, `type_info()`, `Any`. The largest, and the one that makes sema and the VM
   mutually recursive in the way §5 warns about.
4. **`#insert` and the `Code` type** — code as a value.

Numbered so that a later sub-wave can be reordered on evidence, as ADR-0067 §0 reordered W4.5 — not so
that the order is a commitment.

### What running found, and what it corrects in the handoff

§7 has said for several waves that the compiler has "one *trivial* `#run`: a call or a constant
expression, same file only". Two of those three qualifiers are wrong, and running is what showed it:

```jr
add :: (a: s64, b: s64) -> s64 { return a + b; }
sum :: (n: s64) -> s64 { t := 0; i := 0; while i < n { t = t + i; i = i + 1; } return t; }

A :: #run add(add(1, 2), 3);   // nested calls: works, gives 6
B :: #run add(1, 2) * 10;      // arithmetic around a call: works, gives 30
C :: #run sum(5);              // a *loop* in the callee: works, gives 10
```

All three already evaluate. "Trivial" was true of the first `#run` ever implemented and has been
understating the compiler for waves — the opposite of the rot §7 warns about, and worth correcting in
the same breath, because a handoff that undersells is as untrustworthy as one that oversells.

What genuinely does not work is two things, and the first is worse than a missing feature:

- **A `#run` calling an *imported* procedure reports an internal compiler error.**
  `N :: #run print_int(7);` gives
  `E0230: compile-time evaluation failed: internal compiler error: no routine for file 1 proc 11`.
  That is compiler internals shown to a user who wrote a reasonable program. The cause is one line:
  `file_consts` calls `jr_vm::add_file` for **the file being evaluated and no other**, so an imported
  callee has no bytecode.
- **A `#run` in a *body* does not lower.** `n := #run add(2, 3);` refuses with
  "`#run` has no value until jr-vm (ADR-0016 §4)". Only a file-scope `::` constant is evaluated.

## Decision

### 1. The comptime program contains every reachable file

`file_consts` adds the bytecode of every file reachable through `#import`, not just its own — the same
`reachable_files` walk `run_main` and `build` already use for exactly this reason (a cross-file call is
only resolvable if the callee's file is in the program).

**This is not the cross-file dependency const-eval refuses**, and the distinction is worth stating
because `consts.rs` argues the refusal at length. What it refuses is reading another file's *constant
values*: `ImportedValues` is passed empty, on the grounds that one module's constant folding must not
depend on another's. A *routine* is not a value, and supplying one is supplying code for a call sema has
already agreed exists.

**But the first implementation of this section was wrong, and salsa said so immediately.** Taking the
imported file's MIR from `file_mir` produced a dependency-graph cycle:

```text
file_consts(A) -> file_mir(B) -> imported_values(B) -> file_consts(A)
```

because `file_mir` folds imported constants, which needs the *importer's* `file_consts`. Three corpus
tests failed at once with salsa's own cycle panic. So the imported file's MIR is **lowered here**, from
`imported_procs`, `checked` and `resolved` — queries this module already calls — with the same empty
`ImportedValues`/`OperatorCalls`/`FilledArgs` it already passes for its own file. That is the honest
position rather than a workaround: const-eval runs before the check phase that fills those maps, and it
does so for an imported file exactly as for the local one, so an imported callee is subject to precisely
the same `#run` restrictions as a local one (§3).

The claim "adds no dependency that was not already there" was therefore **wrong about `file_mir`** and
right about the principle. Recorded rather than silently corrected, because a reader who tries the
obvious implementation will hit the same cycle.

**Rejected: refusing cross-file `#run` with an actionable diagnostic.** That is the *other* honest
answer, and it was seriously considered: a new code saying "a `#run` cannot call an imported procedure
yet" would at least stop showing internals. Rejected because the refusal would be arbitrary — the call
resolves, the callee has MIR, and the only thing missing was that nobody put it in the program. A
diagnostic that explains a limitation the compiler does not actually have is worse than no diagnostic.

### 2. A `#run` in a body evaluates at compile time and lowers as its value

```jr
main :: () {
    n := #run add(2, 3);    // 5, computed at compile time
    exit(n);
}
```

`jr-mir` lowers a body's `Expr::Run` to the **constant** const-eval computed, exactly as it already
lowers a file-scope `::` constant's. That means a body `#run` is evaluated once, at compile time, and the
body contains its result — which is what `#run` means. It is *not* a call the body performs.

**The evaluation happens in `file_consts`**, which therefore grows a second kind of target: today it
collects file-scope constants and `#run` declarations, and it now also collects `Expr::Run` inside
bodies. One query, one round-robin, one cycle detector — because two places evaluating `#run` would be
two chances to disagree about what a `#run` means.

**Rejected: lowering a body `#run` as a call into the VM at run time.** That would make `#run` a *runtime*
construct that happens to use the comptime interpreter, which reverses ADR-0016 §4 and would make a
`#run` in a hot loop a per-iteration interpreter call. `#run` runs at compile time; the body gets a value.

### 3. What a body `#run` may not do is what a constant `#run` may not do

The existing refusals are unchanged and now apply in one more position: an operator overload, a default
or named argument, and an imported *constant* are all still refused inside a `#run`, because const-eval
runs before the check phase that resolves them (`consts.rs` argues each, and ADR-0018 §3 is why). A
`#foreign` call is still refused by `Mode::Comptime` (ADR-0006).

Stated because the *position* changing might suggest the rules did: they did not. A body `#run` is the
same evaluation reached from a new place.

### 4. What is deliberately absent

- **Folding a `#run` result into the consuming MIR.** The value reaches the body as a constant, but the
  arithmetic around it is not re-folded — that is sub-wave 2 (§0), and doing it here would mean the
  corpus could not distinguish "the value arrived" from "the value arrived and was folded".
- **A `#run` reading an imported constant**, and the other refusals §3 lists.
- **RTTI, `#insert`, `#code`** — sub-waves 3 and 4.
- **A `#run` whose result is an aggregate**, in a body. A file-scope one already has this limit; the
  position does not change it.

## Consequences

- **The internal compiler error becomes a working program.** `N :: #run print_int(7);` prints at compile
  time, which is the first time this compiler has *executed a library procedure while compiling*. A
  corpus program covers it, so the capability cannot silently regress.
- **`file_consts` gains a second target kind**, and its round-robin now iterates over both. The cycle
  detector is unchanged, because a body `#run` is a target like any other.
- **No new diagnostic code.** §1 removes a failure rather than adding one, and §2 lifts a refusal.
  **E0261 is still the first free code.**
- **W4's row in §2.1 gains a note** that it is delivered in sub-waves, with this as the first. §7 records
  which are done, which is what makes the next one's scope checkable.
- **§7's "one trivial `#run`" claim is corrected** in both directions: nested calls, arithmetic around a
  call and a loop in the callee all already worked, and the cross-file case failed with an ICE rather
  than a refusal. A handoff that undersells is as misleading as one that oversells.
