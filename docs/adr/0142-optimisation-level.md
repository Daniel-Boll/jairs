# ADR-0142: An optimisation level, and the proof that the mid-end preserves meaning

- **Status:** Accepted
- **Date:** 2026-09-01
- **Deciders:** dboll
- **Opens W8 — Performance.** ADR-0058 §6 deferred `--release` and `opt_level` to this wave in
  those words: "`BuildConfig` has one field. The optimisation-level surface is W8's, and inventing
  one here would mean designing it around a single boolean." `jr-db`'s `BuildConfig` doc comment
  says the same thing and names W8 too.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

### The mid-end has four passes and no equivalence check

`jr_mir::optimize` runs the inliner, store-to-load forwarding, const-prop and DCE to a bounded
fixed point (ADR-0022 §3), and **every** `jr run` and `jr build` has always run all four. There is
no way to ask for the program as lowered.

That costs two things, and the second is the reason this sub-wave opens the wave rather than the
LLVM back end doing so.

- **A miscompile cannot be attributed.** When a corpus program gives a wrong answer, the candidates
  are lowering, a mid-end pass, and a back end. Two of the three are separable today: the corpus
  differential splits the back ends by running both engines. Nothing splits *lowering* from the
  *mid-end*, and this project has had two silent miscompiles in lowering and one in a pass
  (ADR-0106 §2's store-to-load forwarding bug), so the distinction is not hypothetical.
- **Nothing checks that optimisation preserves meaning.** The optimized-MIR snapshot pins the
  *shape* of the result — it caught a too-broad forwarding fix (ADR-0106 §2) — but a snapshot
  cannot say the new shape computes the same answer. The differential harness has five hand-written
  programs for individual passes (`folding_never_changes_an_answer_either_engine_computes` and
  friends). None of them covers the corpus, and the corpus is where the constructs are.

### What a level must not do

**ADR-0002 is the binding constraint**: overflow always traps and "never differs between debug and
release". ADR-0022 §4 already rejected "remove any dead assignment, traps included" for
contradicting it in terms. So an optimisation level in this language is not permitted to buy speed
with semantics — which makes "the answer is identical at every level" a *checkable* property rather
than a hope, and §3 makes it a test.

Compile-time execution is unaffected by construction, not by care: `file_consts` calls
`jr_mir::lower_file` directly and never reads `BuildConfig`, and ADR-0021 §2 freezes every body
comptime can reach so that both engines run the same MIR for it. A level therefore cannot change
what a `#run` computes. That is the same argument ADR-0058 §4 made for the bounds-check setting.

## Decision

### 1. `-O0` and `-O1`, on `jr run` and `jr build`

`--opt-level <LEVEL>`, short `-O`, accepting `0` and `1`, defaulting to **1** — which is what every
build does today, so no existing invocation changes meaning.

- **`-O0`** skips `jr_mir::optimize` entirely: no inlining, no forwarding, no const-prop, no DCE.
  The back end receives the MIR that `file_mir` produced, which is also what `jr check`'s
  diagnostics and the `mir` dump describe.
- **`-O1`** is the pipeline.

**Two levels, because there are two behaviours.** ADR-0058 §6's warning was against designing a
surface around something that does not exist yet, and a `-O2` that ran the same four passes would
be exactly that: a flag whose only content is a promise. When a later W8 sub-wave adds a pass whose
cost justifies opting into it, it adds a level *and a difference*, and `--opt-level` rejects the
value until then with clap naming the accepted ones.

**Rejected: `--release`.** It is a bundle — optimisation level, bounds checks, debug info, link-time
options — and this project has already unbundled one member of that set on purpose: ADR-0058 made
`--no-bounds-check` its own flag so that a build's safety was not a side effect of asking for speed.
A `--release` that implied `-O1 --no-bounds-check` would re-couple exactly what that ADR separated,
and one that implied only `-O1` would be a synonym for the default. It can be added later as a
composition of independent flags, which is the order that keeps each one's meaning readable.

**Rejected: `-O` as a bare boolean flag.** It reads well at two levels and has nowhere to go at
three, and the wave's own plan (§2.1: SIMD, `#soa`, parallel codegen) is a list of things that may
want one.

**Rejected: making `-O0` the default for `jr run`.** Tempting, because `jr run` is the interactive
command and the pipeline is pure cost for a program that runs once. It is refused because the
default would then differ between the two commands, and the corpus differential compares `jr run`
against a `jr build` binary: a per-command default would make that comparison silently
cross-level, so the harness's central assertion would no longer be about the two engines.

### 2. The level is a second `BuildConfig` field

`BuildConfig` gains `opt_level: OptLevel`, beside `bounds_checks`, for the reason ADR-0058 §2 made
that a salsa input rather than a parameter: configuration from outside the source must invalidate
every query that reads MIR, and an input does it automatically. `optimized_file_mir` already takes
the config, so **no signature moves** and no caller learns a new argument.

`OptLevel` is `Off | Standard` — an enum rather than a `u8`, so `optimized_file_mir`'s match is
exhaustive and adding a level is a compile error at every site that must decide (house style, and
the reason it has caught real bugs). The CLI's own `OptLevelArg` is a separate clap `ValueEnum`
with the display names `0` and `1`, because `clap::ValueEnum` cannot be implemented for a `jr-db`
type from `jr-cli` and `jr-db` must not depend on `clap`. The conversion is one `From` impl.

**`-O0` still strips bounds checks when asked.** The strip pass runs before the pipeline and is
gated on `bounds_checks`, not on the level: `--no-bounds-check` is a request to change what the
program *means*, and honouring it only at `-O1` would make a safety setting depend on an
optimisation setting — the coupling §1 rejected `--release` for.

### 3. Every corpus program computes the same answer at `-O0` and `-O1`

The sweep, in the differential harness, over every corpus program that declares `main`: run each in
the VM at both levels and require the whole observable behaviour — stdout, stderr, exit status — to
be identical.

**Why the VM and not both engines.** The mid-end runs *upstream* of both back ends on shared MIR,
so "did optimisation change the answer" is one question, not one per engine; the existing
discovered-corpus test already answers the orthogonal one by comparing the engines at the default
level. Sweeping both engines at both levels would be four native compiles per program for a
property that lives before code generation.

**Why the whole corpus rather than named files.** The opposite call to ADR-0058 §5, which named
three files for the bounds-check property and said so. That property lives where an index is; this
one lives wherever a pass can fire, which is every program with a call, a constant or a branch. And
`modules/Basic` hid a miscompile for a whole wave because nothing executed it — a sweep acquires
each new corpus program the day it is added, and a list acquires what somebody remembered.

**One assertion is deliberately weaker than the rest**, and it is the interesting one: see §4.

### 4. `-O0` changes a trap's *backtrace*, and that is the flag being useful

Inlining is one of the four passes, and ADR-0021 §3 makes an inlined callee's trap name the **call
site** rather than the callee's own line — with ADR-0066's frame list following the same rule. So a
program that traps inside a leaf reports a different location, and a shorter frame list, at `-O1`
than at `-O0`. That is a real observable difference between the levels, and it is not a semantic
one: the trap fires at the same point, for the same reason, with the same exit code 4.

Rather than weaken the sweep to ignore stderr, the sweep keeps full equality — every corpus program
in `valid/` exits 0 and traps nowhere, so it holds — and the difference gets its **own** named
test, asserting both halves: at `-O1` the trap names the call site, at `-O0` it names the callee's
own line and lists the extra frame. So the one thing a level may change is pinned as a fact instead
of being excluded from a comparison, and `-O0` acquires a second use: it is how a reader gets an
honest backtrace out of a program whose leaf was inlined.

**Rejected: comparing only stdout and exit status.** It would make the sweep pass over a genuine
divergence in a trapping program — precisely the class the harness's own docs record it failing at
once before, when comparing stdout alone reported two engines as agreeing while their trap messages
differed.

**Rejected: suppressing inlining's span rewrite at `-O0` so the two levels agree.** It inverts the
purpose: at `-O0` nothing is inlined, so the callee's own line is not a rewrite to suppress — it is
simply where the trap is.

## Consequences

- **`OptLevel::Off` is asserted to be a true identity**, in `jr-db`'s `optimized_mir` tests: every
  body is byte-identical to `file_mir`'s. That is stronger than "the answer is the same" and it is
  what makes `-O0` usable for attribution — a bug that survives `-O0` is not in a pass.
- **A new sweep of the corpus in the VM at `-O0`.** It is the harness's second discovered-corpus
  loop, and it costs one more compile-and-run per corpus program.
- **`set_build_config` takes two arguments**, so its callers change: `jr run`, `jr build`, and
  `jr-db`'s own tests. `build_config()`'s default stays `(checks on, Standard)`, which is what the
  LSP gets — an editor is not a build, and ADR-0058 §2's reasoning that it should see checked code
  applies unchanged to seeing optimised code.
- **No language change**: no new syntax, no new diagnostic code, no grammar change, no formatter
  change. The first W8 sub-wave is a driver and query change plus the equivalence proof the mid-end
  has never had.
- **Deferred, named**: `-O2` (owed a pass that justifies it), `--release` (§1), and `opt_level`
  reaching a *back end* — Cranelift's own optimisation level is untouched here, and is the LLVM
  sub-wave's business since choosing a back end is what `-O` will eventually select.
