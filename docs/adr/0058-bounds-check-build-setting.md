# ADR-0058: The bounds-check build setting, and `#no_abc` on a procedure — amending ADR-0003

- **Status:** Accepted
- **Date:** 2026-07-30
- **Deciders:** dboll
- **Completes ADR-0003**, which decided in the *vertical slice* that bounds checking is a build
  setting carried as an explicit MIR operation strippable by one pass, and which named `#no_abc` as a
  local opt-out **at an individual index**. §1 and §2 build the setting and the pass. §3 makes
  `#no_abc` a **procedure-level** directive instead, which amends ADR-0003 on that one point and says
  why.
- **Second feature of W3**, after ADR-0057's `context`. Chosen because it is the only remaining W3
  feature that does not need indirect calls, which §6 records as the wave's real blocker.

## Context

ADR-0003 is the oldest accepted decision in this project that is still half-done, and `PLAN.md` §1.5
has said so in the same words for eleven waves: **"the operation exists and the pass that strips it
does not"**. ADR-0039 §7 restated it deliberately — "stated plainly so it does not become another
ADR-0003" — and then it did.

Six facts were established by reading the tree rather than the plan, and four shaped the decisions.

- **`Statement::BoundsCheck` is genuinely finished.** It appears across twenty files: `verify`, `dce`,
  `constprop`, `forward`, `inline`, `ssa`, `dump`, both back ends, sema and the VM's lowering. DCE
  already refuses to delete it, with a doc comment giving ADR-0022's `can_trap` as the reason. **So
  this wave adds no MIR variant**, which is ADR-0003's foresight paying off in the same way
  ADR-0001's reserved `ContextKind` paid off last wave.
- **`Statement::Nop` already exists**, documented as "nothing produces it yet; the mid-end will", and
  it exists precisely so a pass can delete a statement in O(1) without shifting later indices. **This
  is the fact that makes the strip pass four lines**: replacing a `BoundsCheck` with `Nop` needs no
  block rewriting and no index fixups.
- **The only trace of `#no_abc` in the whole tree is one lexer test** asserting that it tokenises like
  every other directive. There is no parser support, no HIR field, and no consumer.
- **`ModuleSearchPaths` is a salsa input**, and its own documentation explains why in general terms:
  configuration that comes from outside the source files must be an input, or changing it will not
  invalidate the queries that read it. **This is the fact that decides §2.**
- **`#c_call` landed last wave as a directive on a procedure header**, with a parser arm, a
  `Proc::c_call` flag and a lowering consumer. **This is the fact that decides §3**: the machinery for
  a procedure-level directive is one wave old and understood, and a per-index directive would need a
  syntax position the grammar has no precedent for.
- **Const-eval does not go through `optimize`.** `file_consts` calls `jr_mir::lower_file` directly —
  its own docs explain that calling `file_mir` from there would be a salsa cycle — so a pass inside
  `optimize` cannot reach the bodies comptime executes. §4 is about that, and it turns out to be a
  feature.

## Decision

### 1. `--no-bounds-check` on `jr build` and `jr run`, and one pass that strips

```sh
jr build prog.jr -o prog                    # every index checked
jr build prog.jr -o prog --no-bounds-check  # no index checked
```

A new mid-end pass, `strip_bounds_checks`, replaces every `Statement::BoundsCheck` with
`Statement::Nop`. It runs **once, before the pipeline** rather than inside it, because it is not an
optimisation that might expose more work — it is a configuration applied to the body, and running it
each round would re-scan a body that can never grow a new check.

**Rejected: making the flag a `#no_abc` on every procedure.** A build setting that is spelled as a
source edit is not a build setting; ADR-0003's whole argument is that the two are different things,
one belonging to the build and one to the code.

**Rejected: deleting the statement rather than replacing it with `Nop`.** `Nop` exists for exactly
this, and the reason is in its doc comment: removing an element shifts every later index in the block.
Nothing in MIR currently holds a statement index across a mutation, so deletion would work today —
which is what makes it the wrong choice, because it would work until something did.

### 2. The setting is a salsa input, `BuildConfig`

```rust
#[salsa::input]
pub struct BuildConfig {
    pub bounds_checks: bool,
}
```

Beside `ModuleSearchPaths`, and for the reason that input's own documentation gives: a configuration
that comes from outside the source files must be an input, or salsa serves a memo computed under the
old value. `optimized_file_mir` gains it as a parameter, so toggling the flag invalidates exactly the
queries that read MIR and nothing else.

**Rejected: a field on `ModuleSearchPaths`.** One fewer input to thread, and wrong in both
directions: changing a module path would invalidate MIR optimisation, and changing the bounds setting
would invalidate module lookup. Both are invisible until somebody measures.

**Rejected: a plain parameter threaded down the query chain.** It works — salsa memoizes per
parameter — but the flag becomes part of the cache key of every query it passes through, including
ones that cannot care.

**`jr check` does not take the flag**, and that is not an oversight: checking produces diagnostics
from *built* MIR, which this pass never touches. A flag that changed nothing would be worse than its
absence.

### 3. `#no_abc` goes on the procedure header — amending ADR-0003

```jr
read :: (buf: [8]s64, i: s64) -> s64 #no_abc {
    return buf[i];      // unchecked, whatever the build setting says
}

safe :: (buf: [8]s64, i: s64) -> s64 {
    return buf[i];      // checked unless the build says otherwise
}
```

ADR-0003 said `#no_abc` "suppresses the check locally at an individual index". This puts it on the
procedure instead, in the position `#c_call` occupies.

**Two reasons, and the second is the load-bearing one.**

A per-index directive needs a syntax position that does not exist. Is it `buf[i] #no_abc`, or
`#no_abc buf[i]`, or on the statement containing the index? Each is defensible and the grammar has no
precedent for any of them — whereas a directive between the return type and the body is a place the
parser learned last wave.

And the *representation* cost is asymmetric. A procedure-level flag is one `bool` on `Proc`, read once
where the body is lowered. A per-index flag has to reach `Projection::Index` from HIR through sema
into MIR, which means every consumer of an index — and ADR-0039's own Consequences list nine passes
plus two back ends that match on `Projection` — either carries it or ignores it. A flag that some
consumers ignore is the shape of this project's first named failure mode.

**This amends ADR-0003 rather than reversing it.** The decision that mattered — the check is explicit
in the IR, strippable as a unit, and the setting belongs to the build — is untouched and is what §1
and §2 implement. ADR-0003 chose the granularity in the slice, before arrays existed, before there
was a procedure-level directive to copy, and before `Projection` had eleven match sites.

**A per-index form stays possible.** Nothing here forecloses `buf[i] #no_abc` later; a procedure-level
flag is a coarsening, and the finer form can be added as a further amendment when someone has a
program that needs it. That asymmetry is why this direction is the safe one to pick first.

**`#no_abc` on a `#foreign` procedure is refused — E0255.** A `#foreign` declaration has no body, so
there is no index in it to leave unchecked, and accepting the directive would mean accepting a word
that does nothing.

### 4. Compile-time execution always checks, whatever the build says

`#run f(9)` on an eight-element array is a **compile error** even under `--no-bounds-check`.

This falls out of the architecture rather than being arranged: `file_consts` lowers its own MIR and
never calls `optimize`, so the strip pass cannot reach it. But it is also the right answer, for a
reason worth stating because the asymmetry looks like an inconsistency:

**A trap at compile time is a diagnostic, not a program behaviour.** It costs the finished program
nothing, and it turns an out-of-range comptime index into an error the user can read instead of a
folded constant containing whatever the VM's memory held. That second thing — a well-typed value
produced by a placeholder path — is this project's first named failure mode, and stripping the check
for comptime would create a fresh instance of it.

**Rejected: stripping for comptime too.** One rule for both, so a `#run` and a runtime call behave
identically under the flag. It needs a second strip site in `file_consts`, and it buys consistency by
making the compiler fold garbage.

**Rejected: refusing the flag on any file containing a `#run`.** It makes the asymmetry impossible,
and makes a build setting fail on a language feature for reasons no user could connect.

### 5. What the corpus must observe

A stripped check is **invisible in any program that stays in range** — which is every corpus program,
because the corpus must check and run cleanly. So the evidence has to be indirect, and three kinds are
needed:

- **A MIR snapshot** showing `nop` where a `bounds_check` was, which is the only direct evidence the
  pass ran.
- **A differential run under both settings**, proving a valid program computes the same answer either
  way. A build setting that changed an answer would be a miscompile.
- **A test asserting the check is still there by default**, because a pass that always strips would
  pass every other test in this list.

**An out-of-range access under `--no-bounds-check` is deliberately not in the corpus.** It is
undefined behaviour by construction — that is what the flag buys — and a test asserting what it does
would be asserting a fact about this machine's stack.

### 6. What this does not do, and the wave's real blocker

- **No `--release` and no `opt_level`.** `BuildConfig` has one field. The optimisation-level surface
  is W8's, and inventing one here would mean designing it around a single boolean.
- **No per-index `#no_abc`** (§3).
- **The rest of W3 needs indirect calls.** Allocators are a procedure pointer plus data; temporary
  storage wants an allocator. Neither engine can call through a pointer, and `PLAN.md` §2.1 assigns
  indirect calls to **no wave at all**. That is a plan contradiction, it is now recorded in §7 as one,
  and it should be resolved by its own ADR before allocators are scheduled rather than during.

## Consequences

- **`optimized_file_mir` gains a parameter**, so every caller changes — `jr run`, `jr build`, the
  dump, the LSP and the snapshot tests. The LSP passes checks-on, because an editor is not a build.
- **One new diagnostic code, E0255**, for `#no_abc` on a `#foreign` procedure. **E0256 is the first
  free code.**
- **`jr-fmt` needs `#no_abc`**, and the formatter has lost a construct in **seven of the last eight
  waves**. A test must assert survival *and* canonicalisation. This is no longer a discovery; it is a
  checklist item for any new node kind.
- **The tree-sitter grammar needs it too**, and ADR-0057's lesson is which failure to expect: a
  directive is a literal token in the `proc` rule, so a missing one is an ERROR node that gate 6
  catches. It is `context`-shaped drift — a rule that parses but means something else — that the gate
  cannot see, and a second directive alongside `#c_call` is not that shape.
- **`Statement::Nop` finally has a producer.** Its doc comment says "nothing produces it yet; the
  mid-end will", which has been true for twelve waves. Every pass that matches on `Statement` already
  handles it, so nothing has to learn it.
- **ADR-0003 is no longer the project's oldest half-done decision.** `PLAN.md` §1.5's "the operation
  exists and the pass that strips it does not" can be deleted rather than restated, and §7 should say
  which sentence replaced it.
