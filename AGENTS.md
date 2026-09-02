# Working conventions for this repository

Read [`PLAN.md`](PLAN.md) §7 first — it is always the current handoff, and it is
rewritten at the end of every wave. §1.5 is the per-crate status table.
[`CONTRIBUTING.md`](CONTRIBUTING.md) has the human-facing rules; this file records how
work actually proceeds, including the things that have cost real time.

---

## The rhythm

Work happens in **waves**. One wave is one component of the slice, and it follows the
same five steps every time:

1. **Put the design forks to the decider before writing code.** Not after. Every wave's
   forks turned out to be expensive to undo, and two of them were only *visible* as
   forks because someone asked. Use the options-with-tradeoffs form: name what each
   choice costs, and name a recommendation.
2. **Record the decisions in an ADR**, with the rejected alternatives argued at their
   point of decision and the project that chose them named where one did. An ADR is
   written once the decision is made and is immutable; a later decision that overturns
   an earlier one gets a *new* ADR that says so (ADR-0018 §5 amends ADR-0017 this way).
   Add the index row in `docs/adr/README.md`.
3. **Implement on a branch named `feat/<component>`.**
4. **All six gates green** (below), then update `PLAN.md` §1.5 and rewrite §7 as the
   *next* wave's handoff, and refresh the README's **"Status, honestly"** section — the
   wave name and test count in its first line, plus any row of its four tables the wave
   changed. That section is the project's only outward-facing honest inventory, and it
   has rotted before: it went a whole wave claiming "a trap still reports no source
   location" after both engines had learned to report one. A capability table is easier
   to keep true than a paragraph, which is why it replaced one.
5. **Commit each wave as it goes green** — a `git commit` on the wave's `feat/<component>`
   branch the moment all six gates pass, *before* starting the next wave. This is not the
   same as merging: merging to `main` still needs the decider's explicit say-so (step 6).
   Committing is the wave's own safety net and does not.

   **Why this is a rule and not a preference.** Fourteen waves once sat uncommitted in one
   working tree at once, and a careless `git checkout tree-sitter-jairs/grammar.js` — run as a
   casual undo during a teeth-check — reverted the grammar *nine waves*, because `HEAD` was
   nine waves behind the working state. It cost an hour of rule-by-rule reconstruction. A
   per-wave commit bounds the blast radius of any such slip to a single wave: `git checkout`
   or `git restore` then takes a file to the end of its own wave, not to a HEAD from before the
   feature existed. `grammar.js` is the sharpest case — gate 6 checks it against *drift* by
   regenerating, never against *reversion* — but the rule is general.

6. **Merge to `main` with `--no-ff`**, one logical change per commit — but only when the
   decider explicitly says so.

## The six gates

Six, plus a seventh that needs an LLVM installation. The six are the ones a contributor with
no LLVM can make green, which is why they stayed six (ADR-0143 §1).

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo run -q -p jr-cli -- fmt --check tests/corpus/valid tests/corpus/imports/valid \
    tests/corpus/type-errors tests/corpus/cfg-errors tests/corpus/modules modules tests/fixtures
# corpus drift + query validation (tree-sitter is not installed locally):
cd tree-sitter-jairs && npx --yes tree-sitter-cli@0.26.11 generate \
  && npx --yes tree-sitter-cli@0.26.11 parse --quiet ../tests/corpus/valid/*.jr \
     ../tests/corpus/imports/valid/*.jr ../tests/corpus/type-errors/*.jr \
     ../tests/corpus/cfg-errors/*.jr ../tests/corpus/modules/*.jr \
     ../tests/corpus/modules/*/*.jr ../modules/*/*.jr ../tests/fixtures/*/*/*.jr \
  && for q in highlights folds indents locals; do \
       npx --yes tree-sitter-cli@0.26.11 query "queries/$q.scm" \
         ../tests/corpus/valid/024-hello.jr > /dev/null || exit 1; \
     done
```

### Gate 7 — the LLVM back end

`jr-codegen-llvm`'s dependency is behind a default-off `llvm` feature, because `llvm-sys` needs
an LLVM 21 it can find and homebrew's `llvm@21` is keg-only (ADR-0143 §1). So the six gates do
not compile that crate at all, and this one does:

```sh
export LLVM_SYS_211_PREFIX=$(brew --prefix llvm@21)
cargo clippy --workspace --all-targets --features jr-cli/llvm -- -D warnings
cargo test --workspace --features jr-cli/llvm
```

It is a *gate* and not a suggestion because of what the ungated Neovim checks cost: editor
integration rotted while nobody ran them, which is why `verify.lua` exists. There is a
precedent for a gate that shells out to a tool the workspace does not depend on — gate 6 uses
`npx tree-sitter-cli` — so needing an external toolchain does not make a check optional.

The three-way differential (VM ≡ Cranelift ≡ LLVM) lives in `crates/jr-cli/tests/differential.rs`
behind `#[cfg(feature = "llvm")]`, so a default `cargo test` does not appear to run a test it
silently skips. **Run gate 7 in any wave that touches MIR, `jr-pool`'s layout, `jr-codegen`, or
either back end** — those are exactly the places where a third engine has something to say.

Track the workspace test count in the §7 handoff, so a silent loss of coverage is
visible. It has gone 376 → 429 → 511 → 596 → 909 → 916 → 918 → 919 → 924 → 928 → 930 → 935 → 936
→ 969 (W5 sub-waves 1–4) → 974 (W5 sub-wave 5, polymorphic structs) → 976 (W5 sub-wave 6a, `$N` surface)
→ 977 (W5 sub-wave 6b, `$N` instantiation) → 978 (W5 sub-wave 6c, `[N]T` over `$N`; 7a `#expand` surface) → 979 (W5 sub-wave 7b, the `#expand` splice) → 980 (W5 sub-wave 7c, reflecting a bound type)
→ 981 (W5 sub-wave 7h, `#bake_arguments` specialisation — **W5 complete**). W6 sub-waves 1–4 hold at
981 — each adds corpus files that the existing differential and snapshot tests iterate rather than adding a test
case, which is why the *corpus* count is tracked too — and sub-wave 5 reaches **984** with three `jr-cli`
integration tests, because the driver's behaviour is not something a corpus file can observe (210 corpus files).

W7 sub-waves 1–17 reach **986** (211 corpus files). The audit sub-waves then go **988** (ADR-0120, the
expansion fixed point, +2 corpus files) → **990** (ADR-0121, the comptime step budget) → **1001**
(ADR-0122, `BUILD_OUTPUT` confinement — nine of the eleven are unit tests on the predicate, which is
why a wave can move this number a long way without touching the corpus) → **1005** (ADR-0123, the
cross-crate code check) → **1007** (ADR-0124, two latent traps) → **1008** (ADR-0125, `print_int`
executed at last, +1 corpus file = **213**) → **1009** (ADR-0126, the foreign-call pointer span — **no**
corpus file, because the VM traps where native writes short, so a program exercising it has no home in
`valid/`, whose whole premise is that the two engines agree; the test lives in `jr-vm` instead).
ADR-0127 holds at **1009** and adds one corpus file = **214**: a wording sweep changes no behaviour,
and `type-errors/073` is iterated by the existing harness rather than adding a test case. ADR-0128 reaches
**1010** (the instantiation-backtrace test) with no new corpus file. ADR-0129 **holds at 1010** and adds
two corpus files = **216** — both are iterated by harnesses that already exist, which is the clearest case
yet for tracking the corpus count separately from the test count. ADR-0130 **also holds at 1010** and adds
one = **217**: an all-library wave, where the only new coverage a corpus file can carry is a corpus file.
**ADR-0131 also holds at 1010** and adds one = **218** — the same pattern for the same reason, since
Matrix4 like Vector4 is exercised by the differential and snapshot harnesses rather than by a Rust unit
test. **ADR-0132 also holds at 1010** and adds one = **219**, closing sub-wave 3 with a Quaternion —
the third all-library wave in a row to move only the corpus count. **ADR-0133 also holds at 1010**
and adds one = **220** — a language wave (parser + HIR) but no new Rust test, since the coverage is
the corpus program that reads `it` and `it_index` in every combination. **ADR-0134 also holds at
1010** and adds one = **221** — another HIR-shaped wave (nested procedures + local constants) with
its coverage in `valid/107` and a repurposed regression test in `jr-hir` guarding the flip.
**ADR-0135 also holds at 1010** and adds one = **222** — a follow-up MIR change closing ADR-0133 §2
(range iteration with an index), with its coverage in `valid/108`.
**ADR-0136 also holds at 1010** and adds one = **223** — Wave 6 (`[..]T` dynamic-array syntax),
another all-corpus-file wave since the coverage is what a corpus program can observe.
**ADR-0137 also holds at 1010** and adds one = **224** — Wave 7 (`$$T`, poly + baking), whose
coverage in `valid/110` is what a corpus program can observe.
**ADR-0138 also holds at 1010** and adds one = **225** — Wave 8 (variadic `..T` declaration
surface), with `valid/111` exercising the callee-view shape and explicit-view passing. The
call-site packing sugar is deferred to a follow-up wave.
**ADR-0139 also holds at 1010** and adds one = **226** — the follow-up completing Wave 8: MIR
packs trailing arguments into a stack `[N]T` view. `valid/112` exercises the sugar (zero,
one, several trailing args; fixed+variadic mix; pass-through view).
**ADR-0140 also holds at 1010** and adds one = **227** — the first of the programme's owed
follow-ups: `modules/List` converted to operate on the native `[..]s64` (the hand-rolled
`List :: struct($T)` deleted), `Type_Info_Kind.DYNAMIC_ARRAY` added to `Basic`, and a dump defect
fixed (a `[..]T`'s `.data`/`.count`/`.capacity` all printed `.view_count`, so the mir snapshot could
not tell them apart). `valid/113` exercises the converted operations and the reflection; `valid/088`
and `089` now declare `[..]s64` with their exit codes unchanged — the one wave here that touched a
crate (`jr-mir`'s dump) and still moved only the corpus count, because the fix is display-only.
**ADR-0141 also holds at 1010** and adds one = **228** — the second owed follow-up, a `..Any` variadic,
probed and found already composing (ADR-0138's callee view ∘ ADR-0139's packing ∘ ADR-0076 §1's
`*U`→`Any` coercion). One gap fixed in `jr-sema` (the exactly-one-trailing disambiguation bypassed the
coercion, so `f(*a)` errored while `f(*a, *b)` worked); the decision is now shared by one helper. No new
code, no MIR change. `valid/114` pins mixed-type `..Any` (the `print(fmt, ..)` shape); bare values stay
E0214 (ADR-0076 §4's deferred bare-value→`Any`).

**ADR-0142 reaches 1018** and adds **no** corpus file (228 unchanged) — **W8 sub-wave 1**, the
optimisation level. The clearest case yet of a wave whose deliverable no `.jr` file can carry: `-O0`
is a *build setting*, and its proof is a sweep over the 114 programs that already exist. Four of the
eight tests are `jr-db`'s (the level is a byte-identity, it invalidates as a salsa input, it is
independent of the bounds check), three are the differential harness's (the corpus sweep at both
levels, the native path, the backtrace difference) and one is the clap surface — which is a test
because refusing `-O2` is a *decision*, so the day a level is added something must record that the
surface used to be closed.

**ADR-0143 reaches 1019** by default and **1020 under gate 7**, and adds no corpus file (228
unchanged) — **W8 sub-wave 2**, the LLVM back end. The split count is the point: the default build
gains one test (that `--backend llvm` is *refused* with a message naming the feature), and gate 7
replaces it with two (the three-way corpus sweep and a trap compared byte for byte, backtrace
included). A test that is `#[cfg]`-ed out of existence is better than one that passes vacuously,
which is why the LLVM axis is not a run-time skip.

**ADR-0144 reaches 1027** (1028 under gate 7) and adds three corpus files = **231** — W8 sub-wave 3,
`#align` and `#place`. Six of the eight new tests are `jr-pool`'s, on the layout fold, because that
is where the whole feature lives: no engine changed for it. The corpus files are `valid/115` (which
exits 114, a checksum of offsets and sizes) and two refusals in `type-errors/`. **The enforced code
registry earned its keep here**: `crates/jr-cli/tests/codes.rs` failed the moment E0283 was declared
while this file still claimed E0282 was free, which is exactly the rot it was written to catch.

**ADR-0150 through ADR-0154 reach 1034** (1035 under gate 7) and **243 corpus files** — PLAN §8.6's first
three steps, which closed **W6**. ADR-0150 turned the ninth leaked internal error into E0286. ADR-0151
implemented `#must`, filling ADR-0008's reserved effect-row slot for the first time since the slice, and
unblocking five W7 modules. ADR-0152 built the compiler-emitted static-data table ADR-0078 §3 deferred,
delivering `Type_Info.fields` with it; ADR-0153 put the message loop on top; ADR-0154 added a second build
option and **declined** plugin hooks and Jai-style workspaces with the poll as the stated reason.

**Two traps re-confirmed in this stretch.** The formatter dropped `#must` on the first attempt (tenth wave
running), and it was the *unsound* direction — losing the attribute deletes a check. And a shell mistake
cost real time twice: `cmd | head -1; echo $?` reports **`head`'s** status, so two apparent VM divergences
were the harness, not the compiler. Rebuild `jr-cli` before every hand-run, too; a stale binary produced a
third false divergence.

**ADR-0155 holds at 1034** (1035 under gate 7) and adds four corpus files = **247** — PLAN §8.3's first
three W7 modules: `Time`, `Bucket_Array`, and the stable merge sort ADR-0104 §3 owed. An all-library wave
on paper that turned into a compiler wave: **the sort would not compile, and four separate polymorphic
instantiation defects came out of finding out why** — `typed(T, …)` refusing a bound type variable while
`size_of(T)` beside it accepted one; an instantiation's pointer views never threaded into MIR; E0268
refusing a template that calls a template; and `check_polymorphic_call` **deleting** a shadowed type
binding instead of restoring it, which PLAN's known-defects list had recorded as masked and was not.
`valid/126` isolates three of the four, so a regression names which one broke, while `valid/125` needs all
four at once and cannot. The wave moved the corpus count and not the test count, for the reason the
all-library waves before it did: what a corpus program can observe is a corpus program.

**ADR-0156 holds at 1034** (1035 under gate 7) and adds one corpus file = **248** — PLAN §8.3 item 4,
`modules/JSON`. The first module in this library that is not a utility: a data model, a grammar, a failure
mode and two kinds of allocation. A value is an **index** into one `[..]Json_Node` rather than a pointer in a
recursive type, so freeing is one call and a handle carries no ownership question. Two of §8.3's own guesses
about this module were wrong and are corrected in place: a `variant` is not the right JSON value, and `Map`
cannot be an object. Numbers get their *extent* from JSON's grammar and their *value* from `strtod`, because
`strtod` alone accepts `0x1p3` and `inf`; integers are converted in Jairs, since `float64` cannot hold
2^53 + 1. Serialisation is deferred with a reason — a correct `dtoa` — rather than half-built.

**Two things that wave is worth reading for.** A `malloc` allocation handed to `String.free_string` (which
frees through `context.allocator_free`) was written and caught: it is invisible while the installed allocator
*is* libc and corruption the moment a caller installs an arena. That exposed the library's real **allocator
seam** — `List` and `Map` use `malloc`, `String` uses the context — which `JSON` is the first module to
straddle. And writing the module's *test* found a MIR gap: `mk().count`, a field of a call's **result**, does
not lower. That is the third capability gap a library has surfaced rather than a compiler test.

**ADR-0175, ADR-0176 and ADR-0177 reach 1069** (**1073** under gate 7) and add one corpus file = **255**.
**W11 — Concurrency is DONE, and it was the last of the twelve waves.**

**The blocker PLAN named was not the blocker, and this is the entry to read before trusting any plan here.**
§8.3 said W11 needs a per-thread stack, atomics as language operations, and a comptime rule. It did not say a
thread body could not be **named**: `#c_call` was a *declaration* attribute with no spelling in a **type**, so a
`#c_call` procedure could be declared, called directly, and handed to nothing — and `pthread_create` takes a
function pointer. `jr-pool` had modelled the two conventions as distinct types since ADR-0001, and `ctx.rs`
interned the distinction away with a comment explaining why that was safe.

**Found by three probes in four minutes**, the third of which reported **`expected (s64) -> s64, found (s64) ->
s64`** — two identical types, because `describe` did not render the convention either. So the diagnostic that
would have explained the wall was itself broken.

**Three engines had each hard-coded the convention at an indirect call**, each with a comment saying it was safe
because no `#c_call` pointer type existed. MIR and Cranelift then failed **loudly**; **LLVM did not** — it would
have passed the context where C expects the first argument. Two of three failing loudly is not a safety net.

**The comptime fork is closed on a fact.** Refuse a spawning `#run`, serialise it, or grow a scheduler? The VM
cannot marshal a *procedure* to C — C needs a machine address, an interpreter has no machine code — so refusing
is **forced**, and the scheduler option is not expensive but **unreachable**: a scheduler still needs a body to
run. It was on the table because nobody had asked what it would have to produce.

**Atomics are a MIR variant, and the exhaustive-match rule is why they are correct.** Nine mid-end sites became
compile errors that each had to be *reasoned* about: `forward_stores` would have forwarded a store across one,
const-prop would have folded a load another thread writes, DCE would have deleted a compare-exchange whose
*effect* is the lock. A `_` arm would have compiled and produced a program that works until it is optimised.

**And `file_consts`'s early-out bit for the third time.** It returns an empty `ConstValues` unless the file has a
`#run`, a `type_info`, a fold, an `any_of` or a `pointer_view` — a **list of features that nothing enforces** —
so an atomic's callee resolved to nothing and `scan` refused the body with "a name failed to resolve" on an
obviously fine program. The comment directly above that condition already records the previous occurrence,
ending "Found by running the feature's own probe." **Whoever adds a fifth intrinsic family will hit it a fourth
time.**

**Three tooling traps fired on this wave's own files, each caught by its own gate**: the formatter silently
dropped `#c_call` from a procedure type (gate 5 — the twelfth consecutive wave that loop has had to learn a
construct, and the *unsound* direction, since the reformatted file no longer type-checks); the tree-sitter
grammar reported an `ERROR` node over it (gate 6, which is what that gate exists for) and then reported a
**genuine** ambiguity — `f :: () -> (s64) #c_call` — that the hand-written parser resolves greedily in favour of
the type, verified by writing it; and `codes.rs` caught a code collision when this wave first reached for E0290,
which `jr-hir` owns.

**ADR-0178 reaches 1071** (**1075** under gate 7) with no new corpus file, and fixes a defect the W11
audit turned up rather than a planned one. `jr check` on a file-scope mutable variable is **good** —
E0245, and a note reading *"calling it is an error; leaving it uncalled is not."* `jr build` **panicked**:
`function "jr$0$0" with linkage Export must be defined but is not`, exit 101. Phase 1 declares every
procedure `Export`; phase 2 *skipped* a refused body, under a comment reasoning about **diagnostics** in
a place whose problem was **linkage**. The bytecode VM refused honestly the whole time, which is what
made this an asymmetry rather than a shared gap. A refused body now gets a trapping stub, which is that
note made true at run time.

**And the second finding is the one to remember.** `TrapKind::ALL`'s guard asserted `len() == 11` — a
**proxy**, since a variant left out of `ALL` keeps the length right and `[Self; 11]` compiles beside a
twelve-variant enum. It fired only because this wave happened to bump the array length first. Replaced
with an exhaustive match, it *immediately* found that **four of fifteen kinds were never in `ALL`** —
and that `ALL`'s doc comment described a driver loop that **does not exist**, which is exactly what let
the omission stand: the list looked load-bearing, so nobody audited it. Those four were therefore never
checked by `reasons_are_distinct`, whose purpose is that no two kinds share a sentence — because the
corpus differential compares *rendered messages*, so one shared wording hides a real engine
disagreement. **Third instance of one shape: a hand-maintained list, a comment claiming something
enforces it, and nothing that does** — after the E0290 collision and `file_consts`'s feature list.
**A count is not an enforcement. An exhaustive match is.**

**W12's last item was probed and respecified, and ADR-0173 §4's premise was wrong.** That section said the
blocker was `enable_value_labels` in Cranelift's ISA flags. **That flag does not exist** in `cranelift-codegen`
0.134 — not in `settings.rs`, not in the meta crate, nowhere. The real gate is one `func.dfg.collect_debug_info()`
call plus a `set_val_label` per definition, and wiring both produced **ten real register ranges for a four-line
program**. What makes it a wave anyway is what the measurement showed next: each label holds its register for
**4 to 40 bytes**, never the whole function, so a single `DW_OP_regN` location would print confident garbage
outside the range — correctness needs a `.debug_loclists` location list, the first section beyond
`.debug_line`/`.debug_info` this project would emit. **A plan entry saying "blocked on X" is worth twenty
minutes of checking that X exists.**

**Still owed after W11**: a per-thread shadow call stack, so a trap in a spawned thread names the right frames —
§8.3 put it *in* this wave, and it needs thread-local storage in both back ends plus a change to the trap path
every existing program uses, so it is its own wave and `modules/Thread`'s docs say so.

**ADR-0174 reaches 1066** (**1070** under gate 7) and holds at **254** corpus files. Stack-resident locals now
work in **both** back ends, and the ADR **amends ADR-0172 §3** — the second time in one session that this habit
has caught this project's own accepted ADR.

**ADR-0173 §4 said to *probe* Cranelift's frame layout rather than assume, and it paid**: `MachBufferFrameLayout`
carries `frame_to_fp_offset` and a per-slot offset, populated unconditionally. But a Cranelift `StackSlot` is not
a MIR slot — this back end also creates *unkeyed* slots for aggregate temporaries — so a `StackSlotKey` carries
the MIR index through the compile. Correlating by creation order would work today and break the first time a
temporary is created before a body's own, which is a change nobody would connect to wrong debug info.

**§3 is the lesson.** ADR-0172 §3 concluded from **one program** that "an aggregate local is not named — its slot
carries no `LocalId`". It is not that general: an aggregate **passed by value to a procedure** is bound to its
slot and *is* named, in both engines, while one only field-assigned is not. The rule is about **usage**, and that
sentence described its test program as though it described the language.

**So the pattern is now specific enough to state: a negative result from one program is evidence about that
program.** Generalising it needs a second program that differs in the suspected dimension — and here that program
was one line away. The habit is nine for nine, and its two most valuable catches were both against ADRs written
minutes earlier in the same session (ADR-0165 → ADR-0164 §5, and this one).

**One DWARF detail worth keeping**: the frame base is the **frame-pointer register**, not `DW_OP_call_frame_cfa`,
because the CFA form needs `.eh_frame` this compiler does not emit. The register number is per-ABI (29 on
AArch64, 6 on x86-64), and an unknown architecture gets `u16::MAX` so a consumer *rejects* the expression rather
than reading a real register that means something else. The test asserts the offset is **negative**, because
forgetting to subtract `frame_to_fp_offset` yields a location that parses perfectly and reads the wrong memory.

**W12's remaining debug work is now one item**: a **register-resident** local, which *neither* engine shows — so
it is a property of the project rather than of one back end. ADR-0173 §4 lists its three pieces.

**ADR-0173 reaches 1065** (**1069** under gate 7) and holds at **254** corpus files. Cranelift now emits a
`.debug_info` — type DIEs and a subprogram per function — so **both back ends agree about a struct's layout in
DWARF by two entirely different routes**, and the test asserts the agreement rather than each emitter separately.

**It was the prerequisite PLAN never listed.** "Locals through Cranelift value labels" needs a variable DIE, a
variable DIE needs a subprogram to live in and a type to point at, and Cranelift had a line program pointing into
a `.debug_info` that did not exist. Worth generalising: **when a plan item seems to need only one new thing,
check what that thing needs to live in.**

**§1 is forced structure, not a preference.** A struct's members need field names, which need the driver's
`SourceInfo` — and that implementor is *per body*, available only during `define`. The DIEs can only be written
once the object exists, at `finalise`. **The two moments do not overlap**, so a `TypeDescription` is built in the
first and emitted in the second. Threading a `SourceInfo` into `finalise` was rejected: it needs a second
module-scoped name resolver beside the per-body one, which is a new channel for a question the existing one
already answers at the wrong granularity.

**Two DWARF details worth not rediscovering.** `DW_AT_type` is a `UnitRef`, so every type DIE must exist before
anything points at one — hence two passes rather than interleaving. And `DW_AT_low_pc` is a *relocation* while
`DW_AT_high_pc` is a *length* (DWARF 4's form, one relocation instead of two); getting them wrong makes every
backtrace frame resolve to the object's first function. The subprogram's symbol must append to the **same** side
table the line-program sequences use, because gimli indexes one list per writer.

**And the process note from ADR-0172 needs widening.** It is not only gates 3 and 7 that must not run
concurrently: **any `cargo` invocation without `--features jr-cli/llvm` races gate 7**, because they share
`target/debug/jr` and gate 7's differential harness shells out to it. Gate 5 (`cargo run -p jr-cli -- fmt`) broke
gate 7 this way, after the same trap had already been recorded for gate 3. Run gate 7 alone.

**ADR-0172 holds at 1064** (**1068** under gate 7) and holds at **254** corpus files. W12's third item for LLVM,
**half delivered — and the partition is the point.** An escaped *scalar* local reaches DWARF with its name, type
and stack location; `lldb` can print it.

**Two boundaries, both found by writing the test rather than by reasoning.** A **register-resident** local is
invisible, because only an *escaped* local gets a MIR slot (ADR-0017 §2) and a slot is what a `dbg.declare`
describes — that is precisely what "locals through value labels" is for, and it is a *different DWARF
expression* rather than a missing call. And an **aggregate** local is unnamed even though it escapes: its slot
carries no `LocalId`. **Both are asserted, the second negatively**, with a message telling whoever fixes MIR to
invert the line — an absence that is asserted is a boundary, an absence merely omitted is something the next
reader rediscovers.

**§2 is the mistake worth remembering.** The name lookup first keyed on `MirSpan::Local`; it found `total` and
**silently missed `pair`**, because an aggregate's slot carries the span of the expression that created it.
`SlotData::local` is the authoritative answer and the back end already held it. **A lookup that names *some*
locals is worse than one that names none**, because the gap then looks like a property of the program rather
than of the compiler.

**And a trap for anyone touching LLVM debug info here: inkwell 0.9's insert helpers panic on LLVM 21.** LLVM 19
replaced the `llvm.dbg.declare` *intrinsic call* with a debug **record**, which is not a value — inkwell casts
the returned `LLVMDbgRecordRef` to an `LLVMValueRef` and wraps it in `InstructionValue::new`, which asserts
`is_instruction()`. **Both** `insert_declare_at_end` and `insert_declare_before_instruction` fail, at a message
naming inkwell's internals and no call of ours. The raw
`inkwell::llvm_sys::debuginfo::LLVMDIBuilderInsertDeclareRecordAtEnd` is used instead, and `inkwell::llvm_sys` is
re-exported so no new dependency was needed.

**One process note.** Gates 3 and 7 were run concurrently once and gate 7 failed with "this compiler was built
without LLVM support" — the two share `target/debug/jr`, and the non-LLVM run rebuilt it underneath the
differential test. **Never run the two test gates in parallel.**

**ADR-0171 holds at 1064** (**1067** under gate 7 — two new `llvm`-gated tests) and holds at **254** corpus
files. W12's second item for LLVM: a struct's layout now reaches DWARF with source field names and real offsets.

**The offsets come from `jr_pool::field_offset`** — the same function both engines use to *compile* a field
access — so a debugger cannot disagree with the code about where `p.y` is. Hand-computing them would be a second
layout implementation, which is the thing ADR-0009 exists to prevent.

**§3 is the finding, and it corrects the plan.** The struct mapping was written, it was correct, and `dwarfdump`
showed base types and **no struct at all** — which looks exactly like the mapping being broken. It was not:
**LLVM prunes a type nothing *declares*.** A `DISubroutineType` listing a struct is a *signature*, not a
declaration, and signatures are metadata LLVM will drop. What retains a type is a variable *of* it. So each
parameter gets a `DILocalVariable`, and **W12's items 2 and 3 are coupled** where PLAN had them as separate
lines: a type DIE with nothing declaring it is not emitted, and a declared parameter with no type DIE has
nothing to point at.

**Two smaller rules worth carrying.** Holes are kept in the parameter DIE list — a `filter_map` would silently
shift every later parameter's name onto the wrong type, producing debug info that is *confidently wrong* rather
than absent, which is this project's least favourite failure mode. And a `None` type propagates: a struct with
one undescribable field gets **no** DIE, because a struct listing *some* of its members shows a type whose
fields do not add up to its size.

**`TrapLocations` is now `SourceInfo`.** It gained `symbol(Symbol) -> Option<String>`, because a struct's members
need field names and a back end has no interner — the same wall `FileInput::names` hit. Renamed rather than
extended under the old name: a trait called `TrapLocations` with a `symbol()` method teaches a reader the wrong
thing about where the next driver-supplied lookup belongs. Clean cutover, no alias.

**Two honest gaps.** The struct DIE is **anonymous**, because the pool records no *declared* name — it carries a
`DeclId` and the name lives on the HIR item, which a back end cannot see. Faking one from the `DeclId` would
print a number no reader recognises, so DWARF's unnamed-struct form is used and `lldb` shows the members, which
is where the value is. And **Cranelift still has no `.debug_info`** — only the line table — so its types must be
written by hand with `gimli`, exactly the split ADR-0170 predicted.

**ADR-0170 holds at 1064** (**1066** under gate 7, since its one new test is `llvm`-gated) and holds at **254**
corpus files. It completes W12's first item by giving the **LLVM** back end a line table, and the useful lesson is
how little was shared.

**None of ADR-0169 was reusable.** Cranelift's table is written by hand — a `SourceLoc` indexing a vocabulary,
`gimli` writing the section, a relocation writer for sequence addresses. LLVM writes DWARF *itself* from `!dbg`
metadata, so that back end attaches a `DILocation` per statement hung off a **per-body** `DISubprogram`, because
LLVM rejects a location whose scope is not the enclosing function's. The two paths share exactly one thing: the
`TrapLocations::position` lookup ADR-0169 §2 introduced.

**So the test is separate, deliberately.** A shared test would assert the intersection of two unrelated emitters
and miss the property worth having — that both, reading one span source, agree about which lines exist. The LLVM
test names the same three statements the Cranelift one does.

**ADR-0170 §3 is this wave's wrong result, and it is a shape to watch for**: every subprogram initially hung off
the *compilation unit's* file, so the file table had one entry and `modules/Basic`'s statements were attributed
to the root program. A line table naming the wrong file is worse than none — it sends a reader to a line that has
different code on it — and **a check on the root file alone would have passed it**. Both DWARF tests now assert
the *imported* module has its own entry, which is the same reasoning as "not every row is the same line".

**Two traps worth knowing before touching LLVM debug info.** It **silently** strips every `!dbg` from a module
whose `llvm.module.flags` lacks `"Debug Info Version"` — a module that verifies, emits, and carries no line table
— which is the one good reason to use inkwell's `create_debug_info_builder` rather than the raw API. And
`finalize()` must run before `verify()`, or the verifier rejects temporary metadata nodes with a message about the
node rather than about the missing call.

**And expect the remaining W12 items to need two implementations too.** LLVM wants
`create_basic_type`/`create_struct_type` metadata while Cranelift wants `.debug_info` DIEs written by hand — the
same split as the line table. A plan budgeting one implementation for "type DIEs" is already wrong.

**ADR-0169 reaches 1064** (1065 under gate 7) and **holds at 254** corpus files — no corpus file, because its
subject is a *section of an object file* and no `.jr` program can observe one. **W12's first item**, and the first
debug information this compiler has ever produced: a built object now carries a valid DWARF `.debug_line` whose
rows name real statements.

**The decision to carry forward is §2.** `TrapLocations` already resolved a `MirSpan` for trap messages, as a
*formatted string*, which DWARF cannot use. So the trait now defines `position()` returning a structured
path/line/column and **`location()` became a provided method that formats it** — an implementor cannot supply one
without the other. That is ADR-0020 §2's one-formatter rule applied to two *consumers* rather than two engines,
and it is what stops a `.debug_line` row saying line 41 while the trap says 40, which is a bug nobody finds
quickly.

**Two wrong results before the right one, both one string, both now comments in the code.** Mach-O spells the
section `__debug_line`, not `.debug_line` — the wrong name produces a section `dwarfdump` silently ignores, which
is indistinguishable from emitting nothing. And a Mach-O debug section outside the `__DWARF` segment **fails the
link**: `ld: pointer not aligned`, because `ld` lays it out among the pointers. Each looked exactly like "the
feature does not work".

**Verified by parsing, not by grepping `dwarfdump`** — a macOS tool whose output is not a contract, and a grep
would have passed on both wrong results. The test asserts rows name lines that *are* statements (a `return`, a
`while`, an `if`, spread through the file so one wrong constant cannot satisfy all three), that not every row is
the same line, and that the file table holds **both** files since the program imports `modules/Basic`.

**Two things a later wave needs to know.** `gimli` is pinned to **0.33**, matching what `cranelift-object` already
pulls, so there is exactly one DWARF library in the tree — the workspace had declared 0.34 and never used it. And
**`ld` on macOS leaves DWARF in the object**: `jr build` deletes the object after a successful link, so a linked
binary carries none today while `--emit-object` carries all of it. A `dsymutil` step is owed and is a *driver*
decision, not a back-end one.

**ADR-0168 holds at 1059** and adds one corpus file = **254**. Not a wave: a defect found by auditing this file
and `PLAN.md` against each other at W10's close, and the most instructive entry here for a reader who wants to
know how much to trust a document.

> **This sentence was wrong when first written**, and the error is the ADR's own subject. It said "holds at 253 —
> the fixture *moved* directories rather than being added", reasoning that `type-errors/` lost one as
> `imports/invalid/` gained one. But the fixture was only ever *briefly* in `type-errors/` inside this same
> session, so that directory's count never changed and the total went 253 → 254. Caught by **measuring** rather
> than by reasoning, one screen after an ADR arguing that a claim about the code is only as good as the last time
> someone ran it. The reasoning was plausible and the count was not.

**Three of PLAN's inline `[NOT DELIVERED]` markers were stale** — `it`/`it_index` (ADR-0133/0135), `[..]T`
(ADR-0136/0140) and `$$T` (ADR-0137) had all shipped. That is this project's rot **one level up from where it is
usually warned about**: those markers were *added* in one wave to correct a different rot, and then went stale
themselves.

**Each was re-verified by probe, because this file and PLAN disagreed** — the table said `$$T` was undelivered,
this file said ADR-0137 delivered it. Both were partly right, and only running it established which part. So:
**two documents disagreeing is a signal to probe, never to pick**, and a claim about the code is only as good as
the last time someone ran it.

**The probe found an ICE.** `$$T` as a *parameter* works (`valid/110`); `$$T` as a **return** type checked clean
and the call died with `internal compiler error: no routine for file 0 proc 3` — the **tenth** instance of the
leaked-internal-error shape, in a position nobody had ever written. It is now **E0290**, and it is *refused*
rather than implemented: `$$` is `$` plus "and the argument is a compile-time constant", and a return has no
argument, so the construct is **meaningless** rather than unimplemented — which is the strongest case there is for
a diagnostic. The check walks the result list too, so `-> (s64, $$T)` cannot reach the ICE by one extra character.

**The fixture moved from `type-errors/` to `imports/invalid/`**, where it failed two harness assertions first: that
directory's contract is "parses, lowers and resolves cleanly, rejected by *sema*", and E0290 comes out of
lowering. The rule was met by **moving the file, not weakening it** — the sixth such move, after E0250, E0262,
E0271, E0273 and E0276.

**ADR-0167 reaches 1059**, still **253** corpus files, adds the **nineteenth** module — and **closes W10 —
Graphics**, four waves: `Window` + 2D renderer (ADR-0164), the event loop (ADR-0165), `UI` (ADR-0166), `Image`
(ADR-0167).

**`modules/Image`** is BMP only, and that is a scope decision rather than a shortfall: `SDL_LoadBMP_RW` is in
SDL's **base** library, so nothing new is depended on. PNG would need `SDL_image` (a second library's version
skew, for a format that proves nothing extra) or zlib's inflate (the largest single thing this stdlib would
contain, and it belongs beside a `Compress` module). Deferring images was also rejected: a texture path that has
never carried a decoded image is untested, and the *decode* is where the interesting failure lives. The test
**builds its own BMP**, so no binary file is in the repository.

**Two things worth carrying forward.** `Surface_Data` is a second `#place` overlay of somebody else's struct, and
its guarantee is **explicitly weaker** than `SDL_Event`'s — offset 0 there is documented in SDL's own header,
`w` at 16 here is only ABI — recorded because a reader seeing both overlays would assume they were equally solid.
If a third arrives, the pattern deserves a helper that can assert an *offset* rather than only a size.

**And the flat namespace bites for real.** ADR-0166 §7 recorded it as a note; one wave later, `Image` written with
short names gave a file importing `Window`, `Basic` and `Image` **four E0211 ambiguous-name errors at once** —
`fill` and `destroy` from `Window`, `free` from `Basic`, `layout_is_sdl2` from `Window`. E0211 firing is the good
outcome. **The rule: in a flat namespace a module must prefix as though the namespace were its own**, because
there is no qualification to fall back on and a short exported name is a claim on every importer. `Window` gets
away with `fill` and `close` only because it was first, which is not a principle. **Qualified imports are owed**,
and were deliberately not built mid-wave: a feature designed by an inconvenience is the wrong feature.

**ADR-0166 reaches 1058**, still **253** corpus files, and adds the **eighteenth** module — `modules/UI`, an
immediate-mode widget layer, and the second module `jr run` cannot execute. It is the wave that shows the
graphics stack **composes**: one test holds a window, an event queue and a renderer open together and drives a
real interaction through all three, which is a stronger claim than three modules each working.

**The lesson worth carrying is §6, a real bug the wave's own tests caught.** `is_hot` was `return ui.hot == id`,
and `begin_frame` sets `hot` to the `NONE` sentinel — so **`is_hot(ui, NONE)` answered `true` on every frame**: a
widget that does not exist, reported as hovered. `button` already refused a zero id. The accessors did not, and
that inconsistency is the shape that survives review — the guard was written where the *obvious* misuse was, and
comparing against a sentinel is not obviously a misuse until you notice the sentinel is what the field holds most
of the time.

**The general rule, because this project will meet it again: a sentinel meaning "nothing" must not be askable
about through the same accessor as a real value**, or every "is this the one" question has an answer of yes for a
thing that is not there. Found by an assertion written because the zero id *existed*, not because a bug was
suspected — which is the argument for testing a sentinel's behaviour rather than only a value's.

**Also worth knowing before building a module on another one: `#import` is flat.** There is no `Window.Event`
syntax (probed — it does not parse), so a module's names land in the importing file's scope unqualified and a
module building on another must not collide with its names.

**ADR-0165 reaches 1057** (1058 under gate 7), still **253** corpus files — and it **amends ADR-0164 §5 by
contradicting it**, which makes it the most instructive entry in this file.

ADR-0164 §5 recorded that `modules/Window` could not have an event loop, because `SDL_Event` is a union and
E0286 refuses one at a `#foreign` boundary. The refusal is right and **irrelevant**: E0286 refuses an aggregate
crossing **by value**, and `SDL_PollEvent` takes a **pointer** — the same shape as the `*Rect` that module had
been passing successfully for the whole of the preceding wave.

**So the habit this file names — confirm a wave's premise by *writing* the thing before planning around it — is
now seven for seven, and this is its most valuable catch: against an accepted ADR of this project's own, from
the same session.** ADR-0164 §5 planned around a premise it never wrote, then built a story on it: "four waves
at one boundary", plus a claim that settling this fork also settles ADR-0163's Objective-C question. Both are
withdrawn. The correction cost one probe — four assertions, four passes, **no compiler change**. An ADR is
evidence of a decision, not evidence of a fact.

**`#place` (ADR-0144) turns out to be the union mechanism**, since two fields at one offset is what a union is.
`key_sym` and `mouse_x` share offset 20 and the test *asserts* the sharing rather than tolerating it. Fields are
widened to `s64` and constants never narrowed, because widening a `u32` cannot be wrong.

**Two smaller findings, both from writing rather than reasoning**: SDL does not promise one-push-one-poll — a
test that polled once per push passed on the first and failed on the second, which is why `wants_to_close`
drains — and a synthetic `KEY_DOWN` is pushed *successfully* and then dropped by SDL, so the keyboard
assertions read a locally-built event.

**Two language items are owed**, both found here and neither invented: a **typed constant** (`QUIT : u32 : 256`
does not parse; one module wants nine), and `size_of` of an **imported** struct from a **file-scope constant**
(E0230 — `Socket` and `Window` have both moved the check into a procedure instead).

**ADR-0164 reaches 1056** (1057 under gate 7) and adds **no** corpus file — still **253** — for a reason worth
recording, because it is a *new* one: `modules/Window` is the seventeenth module and **the first that `jr run`
cannot execute at all.** The VM resolves a foreign symbol from the compiler's own process image, so it reaches
libc and nothing else; SDL2 is unreachable by construction. A corpus file in `valid/` asserts the two engines
agree, and here one engine cannot participate, so the test is a native-only `jr-cli` integration test — the
same call ADR-0158 made for `Process` and against `Socket`.

**And the wave's finding is more useful than the wave**: there is no event loop, because `SDL_PollEvent` fills
an `SDL_Event`, which is a **union**, and E0286 refuses one at a `#foreign` boundary. ADR-0160 §3's reason is
unarguable — members overlap, so every C ABI treats the bytes as opaque. That makes **four waves at one
boundary**: `stat` (ADR-0157), `sockaddr` (ADR-0158), structs (ADR-0161, which opened it), now a union. The
first three could route around it. This one cannot, and settling it — a C shim compiled during a build, or a
`#place` overlay carrying per-version offsets — **also settles ADR-0163's deferred Objective-C question**,
which reaches the same fork from the other side. Rejected on the spot: hard-coding `event.type` at offset 0,
which is four lines and a silent break on any SDL2 point release that reorders a member.

**ADR-0163 reaches 1055** (1056 under gate 7) and adds **no** corpus file — still **253** — because its
subject is a *link line*, which no `.jr` program can observe. PLAN §8.5's correction, and the most instructive
kind: **that section's own correction was itself wrong.**

§8.5 said W10 needs "Cocoa via `#foreign`". Every Cocoa call goes through `objc_msgSend`, which is variadic, and
ADR-0162 established the blocker is **upstream** in Cranelift. That does not delay the wave — it removes an
option. So **W10 is built on SDL2's C API**, and the choice is proven rather than argued: a Jairs program opens
a window, creates a renderer, sets a colour, clears, fills a rect through a `*SDL_Rect`, presents and tears
down. Six calls, six successes, no `objc_msgSend` and no aggregate by value.

**The probe failed once first, and the failure was the deliverable**: `ld: library 'SDL2' not found`. A
`#system_library` names *what* to link and never *where*, and `-lc` had always resolved from the driver's
defaults, so no program had needed a search path in sixteen waves of library work. `jr build -L` and
`JR_LIBRARY_PATH` now exist — **`-L`s before `-l`s**, which `ld` requires, and **not** a source directive, since
a file naming `/opt/homebrew/lib` is unbuildable anywhere else (the `-o`-over-`BUILD_OUTPUT` asymmetry again).

**And the test builds its own library rather than using SDL2**, with the negative half first: without the flag
the link must *fail*. A success-only test passes even when `-L` is ignored, which is ADR-0055's "a test that
passes without the code it tests is worse than no test", met again.

**ADR-0162 holds at 1054** (1055 under gate 7) and adds two corpus files = **253** — the `#c_variadic`
marker, which is the first half of ADR-0157 §2's two and W10's other gate. A fixed-arity declaration of a
variadic C function puts the extra argument in the wrong place *silently*, and **nothing can infer
variadicity**: a Jairs signature cannot say the C one ended in `...`. So it is a marker, its **absence** means
"not variadic" (the safe default), and a *call* is E0289. **E0290 is now the first free code**, and the
enforced registry caught the stale claim immediately — which is what it is for.

**Refused in all three engines rather than only Cranelift**, even though libffi has a variadic CIF and LLVM has
variadic function types: `jr build` failing where `jr run` succeeds breaks the premise the differential harness
rests on. Cranelift's `Signature` has no variadic boundary at all — probed — so supporting the call is blocked
upstream, and `objc_msgSend` stays uncallable.

**The formatter trap fired for the eleventh consecutive wave**, and this was the most unsound direction yet:
`jr fmt` silently *deleted* `#c_variadic`, and dropping it restores the very miscompile the marker exists to
prevent. Round-trip and idempotence both passed — a formatter re-emitting `node.text()` verbatim passes both.
**Eleven repetitions in, the rule is: a new node kind must join the emitter, and round-trip assertions do not
prove it did.**

One smaller lesson: **a refusal that poisons its expression makes every neighbour speak up.** Getting
`type-errors/080` down to one diagnostic needed a real pointer instead of `null` and `_ =` instead of a
binding, because the refused call's `ERROR` type drew E0257 and an untyped `null` drew another.

**ADR-0161 reaches 1054** (1055 under gate 7) and adds one corpus file = **251** — PLAN §8.1.2 **part 2**,
which closes the project's highest-leverage blocker. An aggregate crosses a `#foreign` boundary now, and
**W10 — Graphics is unblocked** along with `readdir`/`stat` and `getaddrinfo`.

**Three engines, three different correct shapes.** The VM *describes* the struct to libffi and delegates — it
consults `classify` only to bound its return buffer, because libffi implements the ABI itself. Cranelift emits
an `AbiParam` per register and moves **whole words from the layout's start**, never per-field, since the class
counts words from the *size*. LLVM emits **separate scalars rather than `byval`**, matching Cranelift so the
differential harness compares like with like; its one delegation is the return, which is a struct of the
class's pieces.

**Two traps worth carrying.** The `#[repr(C, align(16))]` on the VM's return buffer is load-bearing:
`libffi::low::call` writes into a `MaybeUninit<R>` directly once `R` is a word wide, and a returned struct is
stored *from registers*, so a one-aligned `[u8; 32]` is undefined behaviour. And the Cranelift verifier caught
an early `return` in the signature builder that pushed the results and dropped every parameter —
"mismatched argument count: got 2, expected 0" at the first call site. A builder with more to append must not
return early.

**The verification is the point.** A test calling a Jairs `#c_call` procedure passes with both sides wrong,
because one classification emits the call *and* reads it. So: libc's `ldiv` (a real sixteen-byte struct return)
in all three engines, checking quotient and remainder **separately** so a register swap shows; plus a
`cc`-compiled shim at `-O1` for the argument direction, a field-swapping return, and a nested four-`double`
HFA. When testing an ABI, link against something a C compiler produced.

**ADR-0160 reaches 1053** (1054 under gate 7) and adds **no** corpus file — still **250** — because it adds no
language behaviour at all. PLAN §8.1.2 **part 1 of 2**: the C ABI classification for an aggregate, in
`jr-pool` beside the layout computation, so that the VM, Cranelift and LLVM *ask* instead of each deciding.
The reasoning is ADR-0020 §2's about trap messages, with more force: a mis-rendered message is visible and a
**mis-placed register is not**.

**Two things from it to carry into part 2.** An HFA has **no size limit** — a `CGRect` is four `float64`s and
thirty-two bytes, so a byte test rejects exactly the type W10 needs most; the limit is four *scalars*. And
`Class::Memory` is a **refusal**, not an indirect pass, because the case covers a large composite (where
indirect is right) *and* a small mixed one (where System V and AAPCS64 disagree about which register file each
field uses). One case with two correct answers gets refused until it is split.

**Part 1 deliberately changes no behaviour**, so the engines can be wired one at a time with no window in
which two of them disagree. Part 2 must still land **atomically** across all three, and must be verified
against a **real C compiler** — `ldiv` returns a sixteen-byte integer struct from libc, and a `cc`-compiled
shim covers parameters and the HFA. A test checking Jairs against Jairs passes with both sides wrong.

**ADR-0159 reaches 1040** (1041 under gate 7), adds **no** corpus file — still **250** — and takes the
Neovim check count 166 → **170**. PLAN §8.4, W9 — Tooling depth: semantic tokens, the fourteenth and last LSP
capability. All five new tests are `jr-lsp`'s, because a token classifier's behaviour is not something a `.jr`
program can observe — the same reason the compile-throughput wave moved only the test count.

**Two things from it worth carrying forward.** The provider classifies by **CST context first** and resolution
only for a bare `NAME_EXPR`, which is what makes it work in a file that does not parse — the state an editor is
in most of the time, and a case the tests pin with `return p.` mid-expression. And the delta encoding is
guarded by sorting the tokens before encoding and by computing each length from **two positions** rather than
a byte range: one out-of-order token corrupts every position after it, and a byte length overruns under UTF-16.
The tests decode the stream back rather than asserting on raw integers, for both reasons.

**§8.4's DWARF row was written from a false premise, and correcting it is the wave's second deliverable.**
It said "line tables exist"; there is **no DWARF at all** — probed: empty `.debug_line`, no `__DWARF` segment,
no `gimli` consumer, no source location on any instruction. The README's capability table was right the whole
time, which is the argument for keeping it. The item is a from-scratch writer and is now **W12 — Debug info**,
named the way §8.3 named W11 rather than left as a mis-estimated line. **When a plan row and the README
disagree, probe before planning around either.**

**ADR-0158 reaches 1035** (1036 under gate 7) and adds one corpus file = **250** — PLAN §8.3 items 6 and 7,
`modules/Process` and `modules/Socket`, which **close W7 — Stdlib**: nine of nine, with `Compiler` delivered
inside W6 and `Thread` split out to W11.

**The finding that decides where a test can live: the VM cannot pass a pointer to memory that itself contains
pointers.** A foreign call's pointer argument is translated from the VM's region-relative address to a host
address (ADR-0061), one level deep — and one level is all a *type* can support, because the VM knows a
parameter is a pointer and cannot know the bytes behind it hold more. `execvp`'s `argv` is an array of
pointers, so `Process.spawn` works in a compiled binary and fails under `jr run`. Refusing such a call was
considered and rejected: "the pointee contains a pointer" is decidable and would also refuse `strtod`'s
`char **end`, which `JSON` uses and which works. So `Process`'s test is a **native `jr-cli` integration
test** — the conclusion ADR-0126 reached for its own case, and the rule generally: a program whose two engines
legitimately differ has no home in `tests/corpus/valid/`. `Socket` is unaffected, and the contrast inverts the
intuition — a `sockaddr_in` passed *by pointer* is the easy case, because it holds only integers.

`Pool::view_of` now interns `*elem`. The obligation used to sit in `static_array` alone, on the ground that
every other view came from a `[]T` annotation; a `view(p, n)` over a **struct** element type did not, and
leaked "a view's element pointer type was never interned" out of the VM. **An invariant enforced per-caller is
one a caller will miss** — put it in the single constructor everything goes through.

**ADR-0157 holds at 1034** (1035 under gate 7) and adds one corpus file = **249** — PLAN §8.3 item 5,
`modules/File` and `modules/File_Utilities`. The first modules whose correctness depends on something outside
the program, and that changed what the wave found: **two silent defects, neither of them in the modules.**

**A fixed-arity `#foreign` declaration of a *variadic* C function passes the extra argument in the wrong
place.** `open(path, flags, mode)` created a file with permissions `---------x` on arm64 macOS — variadic
arguments go on the stack, a fixed third argument goes in a register — with no diagnostic in either engine.
Creation now routes through `creat`, which is genuinely fixed-arity. **Check every `#foreign` signature
against the C declaration's arity**; a plausible-looking result is the failure mode.

**Freeing a string literal aborts natively and runs clean under `jr run`.** The VM satisfies `malloc`/`free`
from its own region (ADR-0061) and quietly drops a pointer it does not recognise, so `out := "";` followed by
`free_string(out)` — the shape any accumulate-into-a-string loop has — passed every check and died as a
binary. Start such a loop with `substring("", 0, 0)`, whose data is null. **Run the native binary, not just
`jr run`**: this is the divergence class the differential harness is for, and it only catches it when a corpus
program does it — which is why `valid/128` writes to a real `/tmp` instead of mocking a filesystem.

`String` now exports `borrow` beside `adopt`: one construction, two obligations, so the call site says which.
The pair exists because this wave wrote a double free *with* the names available.

**Two lessons worth keeping from ADR-0155.** First, `cmd | head -1; echo $?` bit *again* — the note above was
already in this file, and it still cost several false "silent miscompile" findings, including a spurious
conclusion that indirect calls through a procedure pointer return the wrong answer (they are fine). Check a
status with no pipe in the way, every time. Second, the sort's *first* failure was neither a language gap
nor one of the four: `modules/Sort` had **no `#import` at all**, so `talloc` did not resolve — and because a
module's diagnostics are not shown when a *root* file is checked, and `typed`'s operand check returns
silently when its argument did not type, the whole thing surfaced as one E0245 warning on the body. When a
body is refused for "a local has an error type", check the module's own diagnostics first.

**ADR-0149 holds at 1033** (1034 under gate 7) and adds **no** corpus file = still **237** — W8
sub-wave 8, which closes W8 by *measuring* parallel sema and refusing it. A wave whose deliverable is a
measurement and a revert adds no test and no corpus file, and that is the honest shape for one: the
evidence lives in the ADR, and the code change that lands (`Mutex<Pool>` → `RwLock<Pool>`) is a
refactor the existing suite already covers.

**ADR-0148 reaches 1033** (1034 under gate 7) and adds two corpus files = **237** — W8 sub-wave 7,
`#simd`. Only **one** new Rust test (the formatter's survival-and-canonicalisation assertion), which is
the pattern by now: the coverage a vector needs is a corpus program the three engines must agree on,
and the differential, snapshot and `type-errors` harnesses already iterate the directory. The enforced
registry moved to E0286, and the *parser* range grew for the first time in three waves (E0133).

**The nvim parser is a checked-in `.so` and it goes stale.** `editors/nvim/parser/jairs.so` predates
any grammar change, so `verify.lua` fails "the highlights query loads" the moment a query names a new
node — which is the AGENTS.md trap arriving through the one check that can see it, since gate 6's
`query` run uses the *freshly generated* grammar. Run `./editors/nvim/build.sh` after touching
`grammar.js`, then re-run the verification.

**ADR-0147 reaches 1032** (1033 under gate 7) and adds two corpus files = **235** — W8 sub-wave 6,
`#soa`. Two new tests are the formatter's (survival *and* canonicalisation, because dropping the
attribute changes the program's *layout* rather than its formatting) and the corpus files are
`valid/118` and a `type-errors` refusal. The enforced code registry moved again, from E0284 to E0285.

**ADR-0146 reaches 1031** (1032 under gate 7) and adds one corpus file = **233** — W8 sub-wave 5,
the compile-throughput number and `heap_sort`. One new test is the throughput mode's (asserting the
mode runs *and* that an empty input set is an error, which is the interesting half — a rate over no
files reads as "infinitely slow" rather than "you gave me nothing") and `valid/117` is the sort
comparison. **Two findings were recorded rather than fixed**, both from writing `heap_sort`: a `$T`
template cannot call another `$T` template even with the variable bound (E0268), and a file-level
mutable variable leaks an internal error — the **eighth** of that shape. Both are in PLAN §7.

**ADR-0145 reaches 1030** (1031 under gate 7) and adds one corpus file = **232** — W8 sub-wave 4,
inliner maturity. Three of the new tests are the inliner's own eligibility rules and one is
`valid/116`. **Two existing differential tests failed and only one of them was a test to update**:
the recursive-backtrace test caught the draft's decision to unroll recursion, which flattens frames a
diagnostic cannot get back, so the *decision* changed rather than the test. The other pinned "a
callee that was not inlined names its own line" and only its *premise* had expired — it made the
callee ineligible by having it call something, which is no longer a reason — so it now makes it
recursive instead. Telling those two apart is the whole skill in a wave that changes a pass.

**A number in this file is now partly enforced.** `crates/jr-cli/tests/codes.rs` fails when the
"first free code" claim below rots. The test count and the corpus count are still prose, and both were
wrong in three places each when the audit looked — which is the argument for reading §7 rather than
trusting a count you find anywhere else. **That advice has itself been wrong once**: at ADR-0126 §7 said
"214 corpus files" while this file said 213, and 213 was right — so §7 now carries the *definition*
(the `.jr` files under `tests/corpus/` outside `tests/corpus/modules/`; 223 counting those) rather than
only the figure.

## House style

Enforced by the first four gates, so it is not a matter of taste:

- `[lints] workspace = true` in every crate, and **no crate-level `#![warn]`**.
- `missing_docs` is a workspace warning, so **every** public item — including enum
  variants and struct fields — needs a `///`.
- Private `mod` plus a curated `pub use` in `lib.rs`. Do not make a module public to
  satisfy an intra-doc link; link the item instead.
- Module `//!` docs argue **why**, and name the rejected alternative. A module whose
  docs only restate its type names is not finished.
- **Exhaustive matches** rather than `matches!` or a `_` arm, so that adding a variant
  is a compile error at every site that must change. This has caught real bugs.
- Stable Rust only; the toolchain is pinned. No nightly rustfmt options.
- `unsafe` needs a `// SAFETY:` comment stating the invariant.

## Verifying a split commit

When a wave contains a separable bugfix, give it its own commit *and prove it stands
alone*:

```sh
git add <the fix's files>
git stash push -u --keep-index -m "wave remainder"
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo test --workspace
git commit
git stash pop
```

This is not always possible. In the `jr-vm` wave the aggregate-parameter fix and the
`ConstValues` API change were interleaved hunks in `crates/jr-mir/src/build.rs`, so the
fix went into the wave commit with its own paragraph at the top of the body. Prefer that
over hunk-level surgery.

## Two failure modes this project actually has

### Silent miscompiles from well-typed placeholders

Twice now — braceless control bodies lowering to `Stmt::Error`, and a field of an
aggregate parameter lowering to `Rvalue::Undef` — the shape was identical:

> a construct the grammar allows, no representation on the lowering path, filled in
> with a placeholder that is a **legitimate value**.

Neither the verifier nor ADR-0017 §4's poison gate can catch one, because `Stmt::Error`
and `Rvalue::Undef` are both things a correct program produces. So:

- **A `None` from a place, callee or resolution helper must refuse the body**, never
  fall back to a placeholder. `jr-mir`'s `Lower::give_up` is the channel for a failure
  discovered mid-build; `scan` is the channel for one visible before it starts.
- **If a construct is legal in the corpus, something must execute or snapshot it.**
  `modules/Basic` hid a bug for a whole wave because it is not in
  `tests/corpus/valid/` and `file_mir` is per file, so its bodies never appeared in a
  snapshot.

### Plans that contradict themselves

`PLAN.md` §7 once put `jr run` and the slice exit criterion in scope while assigning a
refusal that criterion depends on to a later wave. Check the handoff's scope against
what the named test actually needs, early, and raise the contradiction rather than
picking a side quietly.

## Tooling notes

- **Subagents have been unreliable on this codebase.** Three of four stalled on the MIR
  wave. Write the modules that define an API yourself; delegate only single-file work,
  with the consumed signatures stated verbatim and a short reading list.
- The agent shell in use **rejects any command containing `grep`, `find` or `rg`** — and
  it rejects the *whole* command, so a `python3` heredoc chained after a `grep` silently
  never runs. If an edit appears not to have applied, check whether its command was
  refused. Use the dedicated search tools instead.
- A query naming a node the grammar has not got used to be **undetectable**, and the
  failure is silent: highlighting simply stops. `tree-sitter query` exits 1 with
  `Invalid node type`, which is why gate 6 now runs it over all four query files
  (ADR-0025 §4).
- **Compile throughput is verified, not gated** (ADR-0146): `jr bench --throughput
  tests/corpus/valid --module-path modules --iterations 10`, with a `--release` compiler for the
  published figure. It reports and never judges, so there is nothing to fail — a timing assertion on
  a shared machine fails for reasons unrelated to the code (ADR-0033 §4). The published number lives
  in the README with the machine beside it.
- Editor integration is **verified, not gated**:
  `nvim --headless -u NONE -l editors/nvim/verify.lua` (166 checks, non-zero on failure).
  Neovim is not a build dependency, so it is not one of the six — but run it after
  touching `jr-lsp`, `grammar.js` or the queries.
- `insta` snapshots: review the `.snap.new` diff, then move it over the `.snap` and
  delete the `assertion_line:` header line, which is noise that changes whenever a test
  moves.
- Never print a `FileId` into a snapshot. It is an index assigned in database load
  order, so one new corpus file renumbers every occurrence — churn that defeats the only
  thing a snapshot is for. `jr-mir`'s dump prints `extern proc3` for this reason.

## Diagnostic codes

There is no central registry of *constants*; the ownership table below is the central record, and
`crates/jr-cli/tests/codes.rs` enforces the part of it that is mechanically checkable. Each crate
keeps its codes near where they are raised — most in a `code.rs`, with one constant per code and
a `///` saying exactly what raises it. Ranges: E0001–E0006 lexer, E0100–E0199 parser,
E0200–E0211 `jr-hir` (E0210 actually raised by `jr-db`'s module loader, E0204 relocated
to `jr-sema`), E0212–E0226 `jr-sema`, E0227–E0229 `jr-mir`, E0230 `jr-db` const-eval,
E0231 `jr-db` unused imports, E0232–E0247 and E0250–E0270 `jr-sema` and `jr-hir` past
their original blocks (E0250/E0253 and E0262–E0264 in `jr-hir`, the rest in `jr-sema`).
E0262–E0264 are `#insert`'s: a non-literal operand and a parse error in the text (ADR-0072),
and expansion nested too deep (ADR-0073). E0265–E0268 are comptime/reflection refusals
(ADR-0075/0076); E0269–E0270 are parameterised-struct refusals — a `Name(args)` that is not a
parameterised struct, and a wrong type-argument count (ADR-0085); E0277 is `has_note`/`note_value`'s single refusal — an unreadable note name *or* a first argument that is not
a procedure (ADR-0099), one code because they are one intrinsic's two ways of being unaskable — and E0278 is
`==` on an aggregate, a `string` included (ADR-0099 §4), which was a leaked ICE until this wave probed it.
E0279 is `typed`/`untyped`'s single refusal — a `typed` operand that is not a `*u8`, or an `untyped` operand that is not a pointer (ADR-0106) — one code for one boundary's two directions. E0277 also covers `noted_count`/`noted_name`'s two refusals (ADR-0100) — an unreadable note name or index —
because all four note intrinsics are one mechanism and share its one way of being unaskable.
E0271 is a `$N` comptime-value
argument that is not a compile-time constant (ADR-0088) — **owned by `jr-db`** beside E0230,
because constancy is a const-eval judgement, defined in `crates/jr-db/src/consts.rs`.

E0272 is a **cross-file** `#expand` macro call (ADR-0091 §3 — repurposed from ADR-0090's
pending-splice refusal, which the splice lifted); E0273 is an early `return` in a macro body or a void macro
in expression position — **owned by `jr-hir`**, continuing its block (E0262–E0264 are `#insert`'s), because
it is raised in lowering where the splice is built.

E0274 was a call to a `#modify` procedure while its predicate was unevaluated;
ADR-0095 **retired** it when the predicate began running, the way E0120/E0122 were retired. E0275 is an
instantiation **rejected by its `#modify` predicate** — **owned by `jr-db`** beside E0230/E0271, because the
predicate is evaluated in `file_mir`.
E0276 is `#bake_arguments` refusing a **non-literal** baked value or an
operand that is not a locally-declared procedure (ADR-0096/0097) — **owned by `jr-hir`**, since a directive's
validity in expression position is judged in lowering.

**E0292 is the first free code**; E0134 is the first free *parser* code. **E0290** refuses `$$` in a **return**
type — **owned by `jr-hir`**, continuing its block, because the validity of a type decoration at a declaration
site is judged where the signature is built. It was a leaked internal error until an audit of PLAN's wave table
probed it: that table said `$$T` was "NOT DELIVERED — E0107" while this file said ADR-0137 delivered it, and both
were partly right — the *parameter* works and is exercised by `valid/110`, the *return* position had never been
tried and died with `no routine for file 0 proc 3` (ADR-0168). E0286 is a `#foreign` signature
carrying a type with no C representation (ADR-0150), E0287 a discarded `#must` result and E0288 `#must` on
a `void` procedure (ADR-0151). E0285 is `#simd`'s single
refusal (ADR-0148) — a width that is not one machine register, an element a lane cannot hold, integer
division, or a trapping integer add — one code because each is "this is not how a vector works".
E0133 is the parser's `#simd` with no array type. E0284 is `#soa`'s single refusal (ADR-0147) — an unusable count, a `using` field, or an index that is not a field receiver — one code because each is "this is not how an `#soa` struct is used". E0282 and E0283 are `#align`'s and `#place`'s refusals (ADR-0144), one per attribute because the two have different rules, and E0132 is `jr-syntax`'s for either attribute written with no value at all. E0280 refuses an
instantiation family that never settles and E0281 a `$N` call in a file whose `#insert`
operand is computed (both ADR-0120, **owned by `jr-db`**). E0231 is `jr-db`'s
unused-import warning — the first code in this project that is a *warning* rather than an
error, so a consumer filtering by severity has something to filter.

**This table is the authoritative one, and it is partly enforced.**
`crates/jr-cli/tests/codes.rs` reads every code declaration in the workspace and checks the
invariant no per-crate test can state — that no two crates declare the same code — plus that a
constant named after a code binds that code, and that the "first free code" sentence above is
true. So the number in bold fails a test when it rots; the prose around it still does not, and
`AGENTS.md` is the only place the ownership story is written down. Two other copies of this table
existed, in `jr-syntax/src/code.rs` and `jr-db/src/imports.rs`, and by the time the audit at
`354d900` looked they had drifted three ways — `jr-syntax`'s claimed E0131 was free while E0131
was in use. Both copies are now pointers here.

`jr-syntax` used to be the exception that proved the rule — it had no `code.rs`, its codes
were inline `&str` literals, and so its parser emitted **E0200/E0201/E0202** for three
"arrives in wave Wn" refusals, colliding with `jr-hir`'s duplicate-declaration,
unresolved-name and use-before-declaration. A `&str` cannot collide at compile time, so
this stood for waves behind a warning here telling people not to filter tests by those
codes. The codes are now E0120–E0122 and the crate has a `code.rs` whose tests assert that
no code is used twice and that every one falls inside a range the crate owns.

**`jr-hir` and `jr-db` still have no `code.rs`** — their codes are inline constants at or near
their emission sites (`jr-hir/src/lower.rs`, `jr-hir/src/resolve.rs`, `jr-db/src/consts.rs`,
`imports.rs`, `module_loader.rs`, `mir.rs`, `sema.rs`). That contradicts the first sentence of
this section and is recorded rather than quietly tolerated: the cross-crate test above closes the
*collision* risk those files carried, which was the reason the rule existed, so consolidating them
is now tidiness rather than a defect. `jr-mir` shows the other legitimate convention — it names its
codes semantically (`USE_OF_UNINITIALISED`) and binds the code as the value, which the test
accommodates deliberately.
