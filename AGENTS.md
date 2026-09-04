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
visible. **It is 1082 today, with 279 corpus files** — ADR-0190 to ADR-0194 held the test count and moved
only the corpus one, which is the pattern every wave whose deliverable a `.jr` program can observe
follows, and the reason the two counts are tracked apart. It has gone 376 → 429 → 511 → 596 → 909 → 916 → 918 → 919 → 924 → 928 → 930 → 935 → 936
→ 969 (W5 sub-waves 1–4) → 974 (W5 sub-wave 5, polymorphic structs) → 976 (W5 sub-wave 6a, `$N` surface)
→ 977 (W5 sub-wave 6b, `$N` instantiation) → 978 (W5 sub-wave 6c, `[N]T` over `$N`; 7a `#expand` surface) → 979 (W5 sub-wave 7b, the `#expand` splice) → 980 (W5 sub-wave 7c, reflecting a bound type)
→ 981 (W5 sub-wave 7h, `#bake_arguments` specialisation — **W5 complete**). It reaches **1073** after the
Simp-shaped-graphics programme (ADR-0179–0182), with **262** corpus files. W6 sub-waves 1–4 hold at
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
**W11 — Concurrency is DONE, and it was the last of the twelve waves *started*.** (W6 was the last one
*finished* — closed on an overclaim, and completed eleven waves later by ADR-0195.)

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

**ADR-0196 amends ADR-0195 §2, and the entry to read first is *why that section was wrong*.** It said a
`#run` "cannot read a file, shell out, print, or even allocate". Two of the five were false, and the
decider found them by asking one question — does allocation really need a foreign library? **It does not,
and it never did here:** `ffi.rs` has served `malloc` from the VM's own linear region since ADR-0061, and
its own comment says "a comptime-adjacent runtime `malloc`". The refusal was keyed on the `#foreign`
*declaration* rather than on whether foreign code is reached, so it refused a call that reaches nothing.

**The rule that generalises: a refusal must be keyed on the behaviour it forbids, not on the syntax that
usually implies it.** ADR-0006 forbids compile-time code reaching *the host*. Three symbols never do —
`malloc` (own region), `free` (no-op in a bump allocator), `write` (capture buffer) — and
`ffi::serves_itself` is now the shared predicate, so the refusal and the dispatch cannot disagree.

**And Jai does it the way the question guessed**, read from source because Jai's compiler is closed:
compile-time `context.allocator` is `Default_Allocator.allocator_proc`, an **ordinary Jai module** (a port
of rpmalloc) bottoming out in OS pages, not libc `malloc`. Decisive: theOS-2's kernel replaces the default
allocator and must route the comptime path back to the stock module with an explicit `if #compile_time`.
Enumerating every `#compiler` declaration across five vendored `Runtime_Support.jai` copies gives exactly
four — `write_string`, `write_strings`, `compile_time_debug_break`, `get_current_workspace` — and **no
allocator is among them**.

**Two compilers independently special-cased the same primitive**, which is the strongest confirmation this
project has ever got for a design: Jai marks `write_string` `#compiler` so compile-time output "syncs with
the compiler's output", and this wave made comptime `write` capture-only so a *memoised* query cannot print
on one build and not the next.

**Getting from "allocation works" to "print works" cost four more fixes, and every one was a claim about
the code that had stopped being true.** Read them as a set, because the shape repeats:

1. **ADR-0053 §2 said supplying the fold maps "would make const-eval depend on the check phase, which is
   the cycle ADR-0018 §3 exists to prevent".** `file_consts`' **first statement** is
   `let checked_file = checked(db, file, search_paths)`. The dependency arrived when `type_info` needed
   sema's folds and nobody re-read the comment. Cost: an operator overload and a default argument were
   unusable in a `#run` — and a **variadic** was worse than refused, giving
   `internal compiler error: called a procedure taking 3 arguments with 2`, because that ADR's claim that
   `scan` refuses such a body held for a default argument and had never been true for a variadic.
2. **`variadic_calls` and `soa_fields` were copied by `optimized_file_mir` and not by const-eval.** Two
   paths populating one `ConstValues` differently is the defect under all of this, which is why they now
   share `record_checked_folds`.
3. **A module's `ConstValues` was empty for a reason that is only *half* true.** Its own `file_consts`
   really would be a salsa cycle — import cycles are legal, and `Cycle_A` ↔ `Cycle_B` are fixtures — but
   almost nothing in one needs *evaluating*: which pointer type a `typed` produces, which opcode an atomic
   is, which `Type_Info` describes a type are all facts `checked` established, and the line building each
   module's frontend already calls `checked` on it.
4. **A constant whose value is a literal was refused for want of an evaluator.** `4096` is already a
   value. `talloc` reads `TEMP_REGION_SIZE` and `out_byte` reads `OUT_CAPACITY`, so *every* library body
   reading one of its own constants was refused. The predicate is shared by `scan_name` and the emit site
   deliberately: if `scan` admitted a body the emit site lowered to `Rvalue::Undef`, that is a
   **legitimate value** and invisible to both the verifier and the poison gate.

**The diagnostic fix is what made the other four findable, and it is the eleventh leaked internal error.**
`add_file` skips a refused body, so calling one in a module gave `no routine for file 1 proc 6` — neither
number means anything outside the database's load order. Naming the procedure turned four rounds of
guessing into four minutes: each fix produced a diagnostic naming the next blocker.

**A `#run` build script works, with no `main`** — `examples/11-run-build-script.jr`. What it cannot do is
**compile** from inside itself: salsa says `Cannot change database mid-query`, measured. So it calls
`Compiler.request_build` to *declare* a target and the driver builds after const-eval. **Jai has the same
division** — `add_build_file` queues, the compiler compiles — but not the same *ordering*: a Jai script
blocks in `compiler_wait_for_message()` while the compiler works on threads, so it can patch icons and
build a `.dmg` inside the same `#run`. That interleaving is precisely ADR-0153 §1's rejected poll, and a
memoising query engine cannot have it. So the shapes agree, the ordering does not, and saying so is the
honest version.

**Four tests had their premises expire and were retargeted rather than weakened** — a foreign call that
really is foreign (`getpid`), a constant that really needs evaluating (`#run pick()`). Fourth recorded
instance of that shape. One MIR snapshot moved on pool ids only; a pool id in a snapshot has the same churn
property as the `FileId` this project already refuses to print.

**ADR-0195 delivers a build script written in Jairs, and it closes W6 eleven waves after that wave was
declared done.** `jr build build.jr` compiles the script, runs it in the bytecode VM, and performs the
compilations it recorded. `examples/10-build-script.jr` shells out for a git hash, reads `-- release`,
branches on the OS, and builds a real Mach-O executable. Tests 1082 → **1090** (seven integration tests, one driver unit test, and seven moved with `confined_output`), no new corpus file (a
build script has no native form, so `valid/`'s two-engine premise does not apply — ADR-0164's reasoning),
and **no new diagnostic code**: `#compiler_library`'s refusals reuse E0293.

**Read §2 of that ADR before copying any design from another language.** W6's row claimed "`#run build()`
build scripts replacing makefiles" and had shipped two *settings*. Copying Jai's shape — the script in a
`#run` — **cannot work here**, because compile-time code may call no `#foreign` procedure, so a `#run`
cannot read a file, shell out, print, or even allocate (`Basic.malloc` is `#foreign`). And the deeper
finding: **Jai's build power is not in its `Compiler` module at all.** Across 23 real `build.jai` files,
what they *do* is clone a dependency, stamp a git hash, build a `.dmg`, format a bootable disk image —
all `Process`, `File`, `String`. **A plan that ports the compiler API and leaves the script unable to open
a file has copied the wrong half.** So the script is an ordinary program, and the whole feature needed no
`#foreign_at_comptime`.

**That retires a "non-negotiable" locked decision by shipping the thing it justified.** PLAN §0 said
comptime FFI was non-negotiable *because* build scripts must read files. Build scripts read files now and
comptime FFI does not exist. It is still wanted for its own sake and blocks nothing.

**The cheapest mechanism was an existing declaration form, and that is why three planned waves became
one.** `#foreign compiler "set_output"` reuses `#foreign`, so the feature needed **no grammar rule, no HIR
node, no MIR variant, and no change to either native back end**. A build script is not something you
compile, so a library that cannot be linked is the right shape. The formatter did **not** drop
`#compiler_library` and tree-sitter parsed it first time — the second wave in fifteen — for the same
reason ADR-0184's file-scope `#insert` was clean: a construct reusing a node kind needs no new emitter arm.

**And the security detail generalises: key a dispatch on a kind, never on a name.** The VM forwards on
`jr_pool::LinkKind::Compiler`, which only `#compiler_library` can produce. Keying on the *string*
`"compiler"` would have handed the driver's vocabulary to any program declaring a library with that name —
probed, and the forgery attempt takes the C route and is refused by the library loader.

**Two recorded blockers had already dissolved, and one by a wave from the same session.** ADR-0154 §2 said
a `Build_Options` struct was blocked on struct literals; the **read-then-mutate** idiom needs none, and it
is what 23 of 23 real Jai scripts use. ADR-0102 said a module-path setting "wants a list-valued
constant"; ADR-0194's array literals answered that one wave earlier. **Both were checked by running a
program, not by reading the ADRs.**

**One silent failure measured and then routed around rather than fixed.** `Process.run` under `jr run`
returns exit code **127** with `ok = true`: `argv` is an array of pointers and the VM marshals one level
deep (ADR-0158 §3). Fixing it *in the VM* needs information no **type** carries — `char **` is `argv`
here and `strtod`'s out-parameter there, and the second **works** — so a build script shells out through
the **driver** instead, with ordinary Rust strings. The VM defect still stands for a general program, and
saying so is better than a rule that breaks working code to describe broken code.

**Four defects found by running it, none visible in review**, and the first is the instructive one: the
default output was `file.with_extension("")`, so `add_file("/tmp/p/main.jr")` was refused as "an absolute
path" — **confinement blamed for a default the driver itself had chosen.** It is the source's *basename*
now. The others: compiling a script gave a wall of `ld` output naming `_jr$2$17` instead of the missing
`--script`; a tool pipeline ate a Rust line continuation, and scanning for the same shape found **two
pre-existing** mangled diagnostics in `jr-sema`; and `Compiler.arguments()` needed an allocator without
saying so.

**The test count earned its keep for the second session running.** The suite went 1082 → **1081** while
the wave *added* six tests: moving `confined_output` into `jr-driver` had dropped its seven unit tests,
every one guarding an escape ADR-0122 found — `.git/hooks/pre-commit`, a leading `-`, a NUL byte. A count
that only ever goes up would have hidden it.

**ADR-0190 through ADR-0194 hold at 1082** and add nine corpus files = **279**, with **one** new
diagnostic code (E0295, an empty array literal) after four stretches with none. Five language utilities the
plan had owed: typed constants, a pointer type as an intrinsic's argument, `type_of(x)`, an enum's member
names and a view's elements, and **array literals** — 39 uses in real Jai code, the most used construct
this language lacked.

**Read this stretch for how much each wave paid the next, because that is the transferable part.**
ADR-0191 put a pointer arm in `described_type`; ADR-0192 put a `type_of` arm in the same function;
ADR-0194 routed an array literal's element type through it and got `Point.[…]`, `(*u8).[…]`,
`Slot(s64, s64).[…]` and `type_of(x).[…]` for **no code at all**. Choosing *where* the first arm went is
what made the last wave small. `described_type` is now the one place four intrinsics and one literal all
ask "what type is this argument?", and the next construct that takes a type should go there too.

**A plausible fix that made the probe pass was still the wrong fix** (ADR-0192 §2). `type_of`'s argument
is a value, so the type-position flag looked like it needed clearing for it — and clearing it was
*unnecessary* (a local is resolved during lowering, so the flag never decided anything) **and worse**
(`type_of(s64)` then reported "unresolved name `s64`", a name that is perfectly well known, on top of the
honest E0261). Found by probing the **refusal** case after the success case already passed. The rule:
after a fix makes the good input work, run the bad input before believing the diagnosis.

**Two absent things looked like one** (ADR-0193 §2). A view's `element` had never been populated — the
`(count, element)` match handled arrays and pointers and a view fell through to `(0, 0)` — and that was
invisible for waves because nothing *used* `element` for a view. Adding the stride beside it is what
surfaced it, as a formatter that had a stride and no element type printing `[.., ..]`. **A gap nothing
consumes is a gap nothing can find.**

**The prescribed fix for the biggest owed item does not work, and writing it is how that was learned.**
ADR-0189 §6 said a `*Type_Info` per type, nested per member. That **diverges** on
`Node :: struct { next: *Node; }` — which is precisely why ADR-0077 chose opaque ids. Three of its four
gaps then turned out to need no table at all, and the fourth needs a **flat** one. A plan entry naming a
mechanism is worth checking against the first recursive type anybody would write.

**The formatter needed three constructs in one stretch, and the array literal was the worst so far**: not
a dropped attribute but the **value** — `a := s64.[1, 2, 3];` formatted to `a := ;`. It also needed **two**
entries, the arm *and* `is_expr_kind`, because the arm alone leaves it unemitted at every nesting site.
And the typed constant's first fix was wrong in a way gate 5 caught immediately: it asked whether any
child was a type kind, and `Array :: struct($T)` has one — its *value* — so it emitted
`Array : struct($T) {`. **The discriminator was the token all along** (one `::` versus two `:`), which is
the only place the difference is recorded.

**The flat top-level walk needs an entry for every construct that puts a type in an expression arena, and
the array literal is the second** (ADR-0194 §2, ADR-0180 §4). `A :: s64.[1, 2];` at file scope reported
`unresolved name s64` — a name that is perfectly well known — because the top-level arena is walked flat and
reached `s64` as an expression in its own right before ever reaching the literal that makes it a type. Worse
than a bad message: it **masked** the honest "an array literal has no compile-time value yet" refusal behind
it, so a reader would have gone hunting for a misspelled type.

The fix needs **both halves** — an entry in `intrinsic_argument_exprs`' skip set *and* the flag in the
recursive walk — and only the second was written first, because the body form is what every test exercised.
**Found by auditing the refusal at file scope after the feature was already merged.** So: when a construct
can appear at file scope *and* in a body, check both, because the top-level arena reaches expressions by a
different route.

**A checksum that lands on zero proves nothing** (ADR-0192 §5). `valid/143`'s total is 15060 and its first
version exited `total % 251`, which is **exactly 0** — the same status a program that did nothing exits.
Check the modulus before trusting it.

**Historical: ADR-0189 holds at 1082** and adds one corpus file = **270**, and adds **no** diagnostic code — E0295
is still the first free one, for the fifth consecutive stretch. `print(fmt, args: ..Any) -> s64`: Go's `%`,
Go's `%!(MISSING)`/`%!(EXTRA …)`, written in Jairs.

**Read this wave for what happens the first time something composes four shipped features.** The variadic
(ADR-0138/0139), `Type_Info` (ADR-0075), `Any` erasure (ADR-0076) and a file-scope global (ADR-0186) were
all built and all believed. `print` is the first caller to use them together, and **it found four compiler
defects, three of them in shipped code**. None was reachable before, which is the whole lesson: a feature
nothing has composed is a feature nothing has tested.

**The proxy-guard family reached four, and this instance is the clearest.** Three
`if self.imports.is_empty() { return None; }` early-outs — `library_struct`, `library_enum`,
`any_struct_quiet` — sat *above* the lookup they guarded. The comment was right that a checker run with
no module resolution must not invent E0265; it was blind to `modules/Basic`, which imports nothing and
**declares `Type_Info`, `Type_Info_Kind` and `Any` itself**. The fallback three lines below already reads
`self.sigs`, exactly where a declaring file's own types live, so the guard did nothing but hide them.

It stood for waves because **`type_info(` appears seventeen times in `Basic` and all seventeen are doc
comments**. The first *code* use said "the compiler could not lower the body of `format_field`", blaming
the body; `print("%", n)` inside `Basic` was "variadic argument expected `Any`, found `s64`" while the
identical call one file away worked. After `TrapKind::ALL`'s length assertion (ADR-0178 §2) and
`file_consts`' feature list (ADR-0176 §6), the rule is: **a proxy is not wrong until something legitimate
sits on the other side of it** — which is why they survive review and surface in a program nobody
suspected.

**And ADR-0186 §3 refuted ADR-0186 §1, in the same ADR.** §1 said only same-file globals occur; the VM
enforced it. §3 made a `GlobalRef` **absolute** so the inliner could copy one unchanged — which is
exactly how a body ends up holding another file's global. `Basic.print` reads the output buffer, so
inlining it into any caller produced `internal compiler error: a cross-file global reference, which this
engine does not yet support` **on an ordinary print**. Neither section is wrong alone; the contract was.
So: **when one section of a decision grants a property, check every invariant that assumed its absence.**

**A flag was built for this and removed, and the removal is the instructive half.** Marking a comptime
program "globals unobservable" — so a comptime read got ADR-0186 §2's honest message instead of a lookup
miss — moved the refusal from **execution** to **assembly**. Assembling `modules/Basic` then failed
outright, and *every constant in the file* reported "a global variable's current value cannot be read
here". A comptime program must still **type** globals so bodies holding one compile; whether a `#run` may
read one was already enforced upstream. **A refusal moved one phase earlier stops being a refusal about
the thing and becomes a refusal about the file.**

**`print("%", f())` refused the body**, because the coercion check excluded `Expr::Call` — true for
`any_of`/`any_as`, which *are* calls, and false for the implicit coercion, which has no call node and is
merely recorded against whatever expression the argument happens to be. Found by writing the shape every
caller writes.

**Six tests changed and the two reasons are worth separating.** Two snapshots. Three **stale
expectations** a library change should invalidate — `print`'s signature in two LSP cards, and the
reference count inside `Basic` falling three → one. One **stale premise**:
`print_line_loses_the_spill_slot_it_never_reads` asserted `slot_count() == 1`, a proxy for "lowering made a
slot at all", while the property that matters is asserted on the next line. And one **stale semantics** —
`a_pointer_coerces_to_any_at_a_call_in_both_engines` expected `size == 16` from an implicitly coerced
`*Point`. ADR-0189 §2 amends ADR-0076 §1: an argument describes **itself**, because otherwise no `Any` has
a pointer type and `print("%", p)` cannot say "pointer". The test now asserts the **difference** (8 vs 16),
because one asserting agreement would pass if both engines lost the distinction.

**One root cause holds four visible gaps together.** An enum prints its ordinal, a nested field prints
`…`, a view prints `<view>`, and a structural type's `name` is the lowercased kind (`array`, not
`[3]s64`) — all because `Type_Info_Field.ty` and `Type_Info.element` are type **ids** and ADR-0077 §1
makes an id opaque, so nothing can recurse. A fixed array escapes only by arithmetic (stride =
`size / count`); a **view** cannot, since its `size` is the header. Lifting all four is one change — a
`*Type_Info` per type via ADR-0152 §3's table — and it is recorded rather than half-built.

**The formatter did not drop anything this wave**, and gate 5 passed first time: no new syntax node was
added, so there was nothing for the emitter to learn. That is the second wave in fifteen to escape that
trap, and for the same reason as ADR-0184's — reuse of an existing node shape.

**And the first thing the formatter was used for found a test that depended on where the mouse was.**
`an_immediate_mode_button_fires_on_release_inside` began failing deterministically — exit 10 where 24 was
wanted — while its three sibling graphics tests passed, so the display was not the cause. It fails
identically at the pre-wave commit, which is how it was told apart from this branch's work: **run the
failing test in a worktree at the parent commit before spending anything on a diagnosis.**

The cause was that its `send` helper folded *every* queued event. A window under the physical cursor gets
real `MOUSE_MOTION` from the OS, and `SDL_PollEvent` pumps more of them **while the drain loop runs**, so a
pre-drain could not have helped. A real motion arrived behind the synthetic click and overwrote the
coordinates the assertions depend on.

**Two things about the diagnosis are worth copying.** The obvious suspect was the `#place` overlay, since
ADR-0165 §2 records it as an ABI-only guarantee a point release could break — ruled out in one minute with
a `cc` probe (SDL 2.32.10 still has `button` at 16, `x` at 20, `y` at 24, `sizeof` 56, all matching
`modules/Input`). Then `print("event kind=% x=% y=%\n", …)` showed the queue directly: `kind=1024 x=60
y=129` behind the pushed `kind=1025 x=20 y=20`. Before this wave that program could not have been written,
which is a concrete answer to what a formatter is worth.

**The general shape: a test that synthesises input into a real queue is not isolated from the real
device.** This one had been green for waves on a machine whose cursor happened to sit elsewhere.

**Historical: ADR-0185 through ADR-0188 reach 1082** and **269** corpus files, and add **no** diagnostic code —
E0295 is still the first free one. Four ADRs: a string literal's `.data` (0185), file-scope mutable
variables (0186), `Simp` and `Window` on Jai's **real** API over OpenGL (0187), and two stale claims in
the compiler (0188).

**Read this stretch for one thing above all: the exhaustive-match rule has a hole, and it is a
`let-else`.** ADR-0186 added `PlaceBase::Global` and nine sites in `jr-mir` failed to compile, each
having to *decide* what a global means to it — which is the rule working. The tenth site,
`forward::participating_slot`, is a `let PlaceBase::Slot(slot) = place.base else { return None; }`, so it
compiled silently and skipped globals **by luck**. It happens to be the right answer, and getting it
wrong would have been a real miscompile: store-to-load forwarding across a global drops the store the
callee was meant to see. So the guarantee this project trusts most — "adding a variant is a compile
error at every site that must change" — holds only where a `match` is written. **A `let-else` on an enum
is a silent `_` arm.**

**The API was recovered from source, not from documentation, and that is why ADR-0182 was wrong.** Jai's
compiler is a closed beta and its `modules/` tree is unpublished, but two open-source Jai applications
vendor it verbatim — `valignatev/hitboxer` and `focus-editor/focus` — so the copies were read and
**diffed against each other**. A divergence between them is visible rather than silently inherited, and
one exists: Focus changes `draw_text`'s colour from a `Vector4` to a `u8` index, so Focus is not a
source for that routine. Eight of Jairs' signatures were wrong. Two not cosmetically: the coordinate
origin was **mirrored** (Jai's default is bottom-left, y up; the SDL renderer used top-left) and every
call carried a state handle Jai does not have.

**ADR-0182 §1 claimed a caller-owned renderer was the *better* API. That is withdrawn.** It was a
limitation described as a choice: the reason two renderers looked like a feature is that one was
impossible, because a file-scope `var` was E0245. ADR-0186 removed the impossibility.

**Two stale claims cost a working program each (ADR-0188), and they are entries 5 and 6 in the family
this file tracks.** A constant's value is keyed by `ItemId`; a computed `#insert` adds an item and
renumbers every later one, so `modules/GL`'s last constants lost their values while earlier ones kept
them — and **moving a constant earlier broke a different procedure**, which is what named the cause.
ADR-0184 §2 *wrote that hazard down*, and the code three lines away already cleared and re-recorded the
other map keyed the same way. So the rule this earns is not "write better comments": **when a fix
re-keys one map because an identity moved, every map keyed by that identity is suspect.** Asking "what
else is keyed by `ItemId`?" at that moment was cheap and was not asked, because a fix that makes the
symptom go away feels finished.

The sixth was `callee_sig` returning `None` for an imported callee under a comment reading *"the other
file's signatures, which this crate does not hold"*. It holds them, and always did —
`entry_for_import` twelve lines away reads them. What was missing was an *index* from a name to a
`ProcId`. The cost: **a default argument silently did not apply across a module boundary**, so
`Simp.set_shader_for_color()` was "takes 1 argument, but 0 were supplied" while the identical call inside
the module worked. The gap looked like a property of the *call*.

**Three traps worth knowing before touching graphics.** A Jairs `string` is `{data, count}` with **no
NUL**, and `glShaderSource` with a null length array reads to a NUL — so the shaders compiled from
whatever followed them in memory, `GL_COMPILE_STATUS` was 0, and **`glGetError` said `GL_NO_ERROR`**.
Identical C succeeded, which is what proved it was this side. `SDL_PIXELFORMAT_RGBA8888` is **not** byte
order R,G,B,A on a little-endian host — `SDL_PIXELFORMAT_ABGR8888` is, and every target here is
little-endian, so the obvious constant would have swapped red and blue in every texture (found by a
sibling agent with a C probe, then verified by reading bytes back). And `SDL_VIDEODRIVER=dummy` **has no
GL at all**: `SDL_CreateWindow` with `SDL_WINDOW_OPENGL` fails outright, so always setting that flag
broke every headless test — `create_window` now asks for GL and **falls back** to a plain window, which
moves the failure to `Simp.is_ready()` where the requirement actually is.

**And a test naming an unimplemented thing has a one-wave shelf life — third instance, same test.**
`a_refused_body_builds_and_traps_instead_of_panicking` used a file-scope mutable variable and its own
comment called it "the shortest program that reaches it today". ADR-0186 implemented it, so the test
failed with *"the trap must name the compiler's gap, got \"\""*. Its construct is now an **imported**
global read directly, which ADR-0186 deliberately did not build.

**Historical: ADR-0183 and ADR-0184 reach 1076** (**1080** under gate 7) and **266** corpus files — per-OS support moved out
of the compiler and into the library. A module now selects a library, a link form, a flag or a value per operating
system in ordinary Jairs, and `modules/GL` is the proof: `#framework "OpenGL"` on macOS, `#system_library "GL"` on
Linux, `#system_library "opengl32"` on Windows, chosen by a `#run` reading `os()` and spliced by a file-scope
`#insert`. Built, linked, run, and the framework read back out of the binary with `otool -L`.

**Read this one before trusting a plan's stated blocker, because two shell commands demolished one.**
`docs/compatibility-plan.md` ruled OpenGL out on a query-order cycle: a per-OS library *name* needs a computed
`#system_library` operand, and library resolution happens inside `file_signatures` which `file_consts` depends on.
The cycle is real. It is also **second in line** — `cc probe.c -lOpenGL` fails on macOS (`library 'OpenGL' not
found`) and `cc probe.c -framework OpenGL` succeeds, and `jr-link`'s whole flag vocabulary was `-L` and `-l`. **A
perfect name mechanism would have emitted a name that does not link.** The real first blocker was a missing
*argument form*: two lines in the linker plus an enum, far smaller than the plan described.

**And the other half was one match arm.** The file-scope directive dispatcher had four — `#import`, `#run`,
`#scope_module`, `#scope_export` — so `#insert "X :: 7;";` at file scope was `error[E0101]: unexpected token at top
level`, while the *same* directive with a *computed* operand already chose per OS inside a body. That single gap is
what made per-OS support look like a compiler feature instead of a library one.

**The most instructive finding is a comment that expired.** `checked_expanded` reused the **unexpanded**
signatures under a comment reading *"because `#insert` adds no items"* — true when an insert could only splice
statements, false the moment it could add declarations. A generated procedure therefore had no signature, and it
surfaced as **"internal compiler error: called a procedure taking 2 arguments with 1"** — blaming the caller. The
polymorphic branch three lines away already recomputed signatures over *its* expanded tree for the same reason one
phase earlier, so the correct shape was visible from the wrong code. **Third instance in this project of a
hand-maintained claim with nothing enforcing it**, after the E0290 collision and `file_consts`' feature list, and
the same lesson each time: the enforcement is an arrangement in which the wrong input cannot be chosen, never a
better comment.

**The boundary is a phase order, and it is refused rather than left to leak.** A **literal** insert expands during
`file_hir`, before signatures and before const-eval, so it generates anything — `valid/136` generates a constant, a
struct, a procedure, a nested insert and an empty one, and exits 63. A **computed** operand expands *after*
const-eval, so a generated constant has no value and a generated procedure no signature; both were leaked
internals ("a file-level item has no value until jr-vm" was the other) and are now **E0294**, whose help names the
two things that work. What a computed operand *may* generate is a library declaration, needing neither — the case
the wave exists for.

**Three withholding sites had to learn "a file insert is pending"**: name resolution, unknown types, and the
`#foreign` library lookup. Each was found by running the feature's own probe and reading a diagnostic about a name
the generated text had not produced yet. `body_has_pending_insert` had established the pattern for statements; the
file-scope twin is `file_has_pending_insert`, and a fourth site will want it too.

**One rule this wave did not have to relearn, and one it re-proved.** The formatter did **not** drop the new
construct — the first wave in thirteen — because `#insert` at file scope reuses `RUN_DECL`'s node shape, so
tree-sitter needed no grammar rule either and gate 6 was clean first time. And the house exhaustive-match rule
earned its keep instantly: interning `LinkKind` into `ForeignLibraryValue` turned **nine crates'** pattern sites
into compile errors, each of which had to be reasoned about rather than defaulted.

**A process trap fired again, exactly as this file warns.** Gate 7 failed with *"this compiler was built without
LLVM support"* because a plain `cargo test --workspace` was running concurrently and rebuilt `target/debug/jr`
underneath the differential harness. **Run gate 7 alone** — it is the third recorded occurrence, after gates 3 and
5.

**Historical: ADR-0179 through ADR-0182 reach 1073** (1077 under gate 7) and **262** corpus files — the Simp-shaped-graphics
programme, on top of the twelve closed waves. Qualified imports (ADR-0179), the target OS as a compile-time value
(ADR-0180), a per-OS library value (ADR-0181), the graphics modules restructured onto `SDL_RenderGeometry`
(ADR-0182).

**Read this one for the score, because it is the highest that habit has ever paid.** The plan was wrong in
**five** places, every one found by *writing the thing* and none by review — and two of the five made items
**unbuildable as written**, not merely suboptimal:

1. **`Res::Imported` on an `Expr::Field`** was the plan's design for a qualified value. Sema reads a callee as an
   `Expr::Name` at a dozen sites and MIR at seven more, so it would have taught nineteen places a new shape — a
   construct half-represented on the lowering path, which is this project's first named failure mode. Carried on
   the *name* (`Expr::Name { module: Option<Symbol> }`), four construction sites became compile errors and **no
   MIR logic changed**.
2. **A second diagnostic code was drafted and refused.** E0293-as-planned ("the alias is not an import") has
   **no reachable condition**: a local of the alias's name makes the access an ordinary field (ADR-0014 §3,
   enforced by *where* lowering checks), and a colliding declaration is already E0200. **A code with no
   condition reads as a promise that something is checked.**
3. **A `BuildConfig` field for the OS**, citing ADR-0058 §2's invalidation argument — which does not transfer to
   a value that cannot change within a process (no `--target`; `jr-link` shells to the host `cc`, `jr-vm`
   resolves symbols from its own image). Cost measured at **≈50 `file_signatures` call sites across six crates**.
4. **"One arm in `thunk.rs`"** for the file-scope-intrinsic gap. Fixing that arm changed **nothing**: a named
   item's initialiser is typed by the **signature** phase, `SignatureOutput` had no `folded_calls` field, and the
   fold was computed and thrown away. Both halves were needed, and the plan named only the reader.
5. **Module-level state** for the renderer and the event buffer. **Jairs has none** — a file-scope `var` is
   E0245, probed for a scalar *and* an array — which made two of the five graphics items unbuildable. The answer
   was `modules/UI`'s own caller-owned-struct pattern, and it is the better API: two windows can have two
   renderers where a global describes one.

A sixth, smaller: **`get_render_dimensions` in `Window`** binds `SDL_GetRendererOutputSize`, which needs the
renderer `Window` no longer has after the split. It could not have compiled.

**Two gaps closed that the library had documented and worked around.** `Window` and `Socket` had both moved an
ABI size-check into a *procedure* because `size_of` of a struct could not reach a file-scope constant; both are
constants now. And `N :: size_of(s64);` failed for a **different** reason found by probing the neighbour:
`resolve_all` walks the top-level expression arena **flat**, so it visited `s64` as an expression in its own
right and reported E0201 before ever reaching the call that makes it a type argument. **The map ended up right
and the diagnostic was already pushed** — a phase whose walk order differs from another's will disagree about
context, and the disagreement shows up as a diagnostic rather than as a wrong answer.

**`file_consts`'s unenforced early-out list needed a fourth entry for a third distinct reason** (ADR-0180 §3).
The comment above it already records two previous occurrences and ADR-0176 §6 a third. It is still a list of
features with nothing enforcing it.

**Three tooling traps, each caught by its own gate.** The formatter dropped a qualified type — `f :: (e: W.Event)`
became `f :: (e: W)`, the **thirteenth consecutive wave** and again the *unsound* direction, since the reformatted
file no longer type-checks. `codes.rs` failed twice on the "first free code" claim, which is exactly what it is
for. And the checked-in Neovim parser needs `./editors/nvim/build.sh` after a `grammar.js` change, as ADR-0148
already recorded.

**One language surprise worth knowing:** `"literal".data` — a field of a string **literal** — does not lower
("a memory reference has no place"), while binding the literal to a local first works. Every program here does
the latter, and so did the pre-existing tests, so nothing was blocked; it cost one confused build.

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

**The nvim parser is a stale build product, and this paragraph used to call it "checked-in".**
`editors/nvim/parser/jairs.so` is **gitignored and was never tracked** — `.gitignore:35`, and
`git log --all -- editors/nvim/parser/jairs.so` is empty. It goes stale on *your machine* rather than in
the repository, which changes the advice: there is nothing to commit and nothing a reviewer can see, so
the only thing that catches it is running the check. It predates any grammar change, so `verify.lua`
fails "the highlights query loads" the moment a query names a new node — while gate 6's `query` run uses
the *freshly generated* grammar and passes. Run `./editors/nvim/build.sh` after touching `grammar.js`,
then re-run the verification.

**And the same audit found the one generated artifact that *is* tracked had rotted.**
`jairs-dashboard.pdf` was a commit behind `jairs-dashboard.typ`, so the file a reader opens still said
"ALL TWELVE WAVES DONE" after the commit that corrected it. `git log -1 -- <artifact>` against its
source is the whole check, and it is the *only* one that works here for two reasons:

- **The PDF is not reproducible.** Typst embeds `/CreationDate` and `/ModDate`, so recompiling an
  unchanged source still produces a four-line diff. Byte-comparing the artifact therefore proves nothing
  in either direction — a real change and no change look identical. Recompile only when the source
  changed, and `git checkout jairs-dashboard.pdf` if the only diff is the timestamp.
- **The PDF has no extractable text.** Typst subsets its fonts and emits glyph ids, so `strings` reports
  every correction missing whether it is present or not, and the content streams inflate to font
  programs rather than words. To check what the artifact *says*, rasterise it — `sips -s format png
  --resampleWidth 1400 jairs-dashboard.pdf --out /tmp/p1.png` — and look at it.

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

**E0296 is the first free code**; E0134 is the first free *parser* code. **E0293** gained a third
condition in ADR-0195 — `#compiler_library` written *with* a name — and it is the same code rather than a
new one because all three of its conditions are "which library is this" being unanswerable: no operand
where one is needed, a `#library` nothing links, and an operand where none can mean anything.
`#compiler_library` names the compiler a build script is running inside, which is not a library and is
never linked, so a name would read as though it named something. **E0295** refuses an array literal
with no elements — `T.[]` (ADR-0194 §2) — **owned by `jr-sema`**, continuing its block. A `[0]T` has no
use a caller could name: it cannot be indexed, `size_of` is zero, and a `for` over it runs no iterations,
so every operation on one is an error or a no-op. Its own code rather than E0261's, which is the
neighbouring refusal for a literal whose element *type* cannot be resolved: that one means "I do not know
what this holds" and this one means "it holds nothing", and a reader chasing the first would go looking
for a misspelled type name. **E0294** refuses a **computed**
file-scope `#insert` generating anything but a library declaration (ADR-0184 §4) — **owned by `jr-hir`**,
continuing its `#insert` block (E0262–E0264), because it is judged in lowering where the generated items are
built. The boundary is a *phase order* and not a policy: a **literal** insert expands during `file_hir`, before
signatures and before const-eval, so anything it generates is indistinguishable from what the file wrote and
every declaration works — verified, a struct and a procedure from a literal insert run to 42. A **computed** one
cannot expand until its operand is evaluated, which is *after* const-eval, so a generated procedure has no
signature when one is wanted and a generated constant has no value; both leaked internals before this code
existed ("called a procedure taking 2 arguments with 1", "a file-level item has no value until jr-vm"). A
`#system_library`/`#framework` needs neither, which is why the per-OS library case works. **E0293** refuses a
`#system_library` declaration that names no linkable library (ADR-0180 §5) — **owned by `jr-sema`**,
continuing its block, raised at the *declaration* because a `#library` nobody calls is still wrong. Two
conditions, one code, each a way of "which library is this" being unanswerable: `#system_library` with no
operand, and `#library "x"`, a directive the parser accepts and nothing links. Both type-checked clean and
emitted no `-l`, so a symbol failed at **link** time with nothing pointing at the cause. **E0292** refuses a qualified name
`Alias.member` whose module exports nothing by that name (ADR-0179 §4) — **owned by `jr-hir`**, continuing its
block, because it is raised in resolution beside E0253, which answers the neighbouring question ("the module
declares it and hides it"). The two are deliberately separate codes: sharing one would send a reader looking
for a `#scope_module` line that does not exist. Group A allocated **no** second code, and the reason is worth
keeping — a candidate refusal for "the alias names something that is not an import" turned out to have *no
reachable condition*: a local or parameter of the alias's name makes the access an ordinary field of a value
(ADR-0014 §3's shadowing rule, enforced by where lowering checks), and an alias colliding with a file-scope
declaration is already E0200. A code with no condition is worse than no code. **E0290** refuses `$$` in a **return**
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
