# ADR-0070: an array length may name a literal-valued constant — amending ADR-0039 §3a

- **Status:** Accepted
- **Date:** 2026-07-31
- **Deciders:** dboll
- **W4 sub-wave 2, rescoped.** ADR-0069 §0 scheduled "aggressive const folding" here. §0 below shows it
  is **already delivered** by ADR-0022's const-prop, verified by probing the optimized MIR — so the
  sub-wave is spent on a gap that is real instead.
- **Amends ADR-0039 §3a**, which refused `[COUNT]u8` and said it "becomes possible in the wave that makes
  sema and comptime mutually recursive". That is true of a length needing *evaluation*, and untrue of a
  length that is already a literal one name away. §2 draws the line the original could not.

## Context

### 0. The scheduled work was already done, and probing is what showed it

ADR-0069 §0 listed sub-wave 2 as "aggressive const folding — folding a `#run` result into the MIR that
consumes it", and ADR-0069 §4 said the value "reaches the body as a constant, but the arithmetic around
it is not re-folded". §7 repeated it. All of that is wrong about the *optimized* MIR, which is what both
engines consume.

The built MIR does keep it unfolded:

```text
v1: s64 = 5_s64 * 10_s64        // built MIR for  n := #run add(2,3);  m := n * 10;
```

which is what the earlier claim was looking at. But the optimized MIR for
`m := n * 10 + 7; return sink(m);` is

```text
Return(Some(Constant(PoolId(25))))     // 57, one constant — no arithmetic left at all
```

and `exit(m)` on that program exits **57**. ADR-0022's const-prop folds through a `#run` result exactly
as it folds through any other constant, because by the time it runs the `#run` *is* one. The distinction
that was missed is between the *built* query and the *optimized* one — and it took a probe of the
optimized body to see, because the built dump is what the corpus snapshots.

**So no folding work is needed**, and inventing some would be work for a claim rather than for a
capability. That is the second time this session a scheduled dependency turned out not to exist
(ADR-0067 §0 was the first), which is worth naming as a pattern rather than a coincidence: a plan's
stated reason is checkable, and checking it is cheap.

### The gap that is real

An array length must be an integer *literal*:

```jr
N :: 4;
buf: [N]s64;      // E0233: an array length must be an integer literal
```

`[4]s64` works. `[N]s64` does not, and neither does `[2 + 2]s64` or a `#run`-computed length. ADR-0039
§3a refused all of them together with one argument: const-eval is downstream of sema, so "`jr-sema`
cannot ask for `COUNT`'s value without inverting that dependency".

**That argument is right about evaluation and too broad about constants.** For `N :: 4` there is nothing
to evaluate: the literal is in the HIR, and `jr-sema`'s `Ctx` already holds both `hir` and `resolve` — it
resolves type *names* against the file scope on the line above. Reading a constant's literal initialiser
needs no `jr-vm`, no `jr-db`, and no phase inversion; `jr-sema`'s `Cargo.toml` still depends on neither,
which was checked rather than assumed.

## Decision

### 1. `[N]T` is legal when `N` names a constant whose initialiser is an integer literal

```jr
N :: 4;
buf: [N]s64;          // now legal, length 4
grid: [N][N]s64;      // and nested, since the element type resolves the same way
```

Resolved in `jr-sema`'s type resolution: the length's name is looked up in the file scope it already
consults, and if it resolves to a `ConstValue::Expr` whose expression is an integer literal, that
literal is the length. Everything ADR-0039 §3 already checks about a literal length — negative, too
large, zero — applies unchanged, because the value takes the same path once it is known.

**This does not invert any dependency**, which is the whole reason it fits here rather than in the RTTI
sub-wave: no evaluation happens, so nothing downstream of sema is consulted.

### 2. A length needing *evaluation* stays refused, and that is where ADR-0039 §3a still holds

Still E0233:

- `[2 + 2]s64` — arithmetic, which needs folding and therefore const-eval;
- `[#run four()]s64` and `[N]s64` where `N :: #run four();` — evaluation by definition;
- a length naming a constant in **another file** — its value crosses through `file_consts`
  (ADR-0055), which is downstream of sema.

ADR-0039 §3a's sentence — "`[COUNT]u8` becomes possible in the wave that makes sema and comptime mutually
recursive" — is therefore **half amended**: the literal-valued case arrives now without that recursion,
and every case that genuinely needs a *value* still waits for it. The line is "is the length already a
literal, one name away" rather than "is the length a literal".

**Rejected: accepting arithmetic by folding it in sema.** A small constant folder in `jr-sema` would make
`[2 + 2]s64` work, and it is the obvious next step. Rejected because it would be a *second* constant
folder — ADR-0022 already owns folding, and two implementations of "what does `2 + 2` mean" is exactly
the duplication ADR-0018 §2 refuses for layout and ADR-0020 §2 for trap messages. When sema and comptime
become mutually recursive, one folder answers.

**Rejected: threading `ConstValues` into sema.** That is the dependency inversion ADR-0039 §3a named, and
it stays refused for the reason it gave.

### 3. The diagnostic distinguishes the two cases

E0233's message is currently "an array length must be an integer literal", which after §1 would be wrong:
a literal is no longer required, a *literal-valued constant* is. The message now names what was found —
a length that needs evaluation says so, and one that names a non-constant says that instead — so the
reader learns which side of §2's line they are on rather than being told a rule that is no longer true.

**No new code.** E0233 already means "this length is not usable"; the refinement is in its wording and
its note. **E0261 is still the first free code.**

### 4. What is deliberately absent

- **Arithmetic, `#run`, and cross-file constants as lengths** (§2), all waiting on the RTTI sub-wave that
  makes sema and comptime mutually recursive.
- **A constant whose initialiser is another constant** (`A :: 4; B :: A; buf: [B]s64;`). One level of
  indirection is resolved, not a chain — a chain needs a fixpoint and a cycle check, which is the
  evaluation machinery §2 defers. Refused with §3's message rather than followed one step.
- **A length from an `enum` member**, which is a value of a nominal type rather than an integer.

## Consequences

- **ADR-0069 §0's sub-wave list changes**: sub-wave 2 is this, not folding, and §7 records that folding
  was already delivered. A plan that schedules work already done is the same failure as one that
  schedules work on a dependency that does not exist.
- **`[N]T` becomes writable**, which matters most for `modules/Basic`: `print_int`'s owed `[20]u8` buffer
  can name its size instead of repeating `20`. That is the first thing this unblocks, and it is left for
  its own change rather than bundled here.
- **E0233's message changes**, so the corpus file that pins it changes with it. `type-errors/` files
  assert the code rather than the text, so this is a wording update rather than a new expectation.
- **No new diagnostic code, no new pool item, no MIR change.** The length reaches `Item::ArrayType` as a
  `u64` exactly as a literal does — which is the evidence this fits where it was put: after §1, nothing
  downstream can tell how the length was written.
