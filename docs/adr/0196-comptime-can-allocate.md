# ADR-0196: Compile-time execution can allocate, print, and declare a build

- **Status:** Accepted
- **Date:** 2026-09-04
- **Deciders:** dboll
- **Amends:** ADR-0195 §2, ADR-0053 §2

## Context

ADR-0195 delivered a build script and justified running it as an ordinary program with this:

> compile-time code may call no `#foreign` procedure (ADR-0006), so a `#run` cannot read a file, shell
> out, print, or even allocate — `Basic.malloc` is itself `#foreign`.

The decider pushed back on exactly the right word: *allocate*. Does allocation really need a foreign
library, or does the compiler provide its own allocator?

**It does not need one, and the compiler already provided its own.** That sentence was wrong, and the
wrongness was mechanical rather than deep: `crates/jr-vm/src/ffi.rs` has intercepted `malloc` since
ADR-0061 and satisfies it from the VM's **own linear region**, never calling libc — its own comment says
"a comptime-adjacent runtime `malloc`". The comptime refusal was keyed on the `#foreign` *declaration*
rather than on whether a host is reached, so it refused a call that reaches nothing.

The cost of that was not small. Compile-time code could not allocate, so no `talloc`, no
`context.allocator`, no string building, no `Compiler.arguments()`. And it cascaded: a `#run` could not
print either, for four further reasons that each turned out to be its own stale claim.

**And Jai does it the way the decider guessed.** Read from source, since Jai's compiler is closed but
its modules are vendored in public projects: `context.allocator` at compile time is
`Default_Allocator.allocator_proc`, an **ordinary Jai module** — a port of rpmalloc — which bottoms out
in OS page mapping and *not* libc `malloc`. The decisive evidence is theOS-2's kernel, which replaces the
default allocator and has to route the compile-time path back to the stock one:

```jai
// dlandahl/theos-2 kernel/modules/Runtime_Support.jai:112-118
runtime_support_default_allocator_proc :: (mode: Allocator_Mode, …) -> *void {
    if #compile_time {
        Default_Allocator :: #import "Default_Allocator";
        return Default_Allocator.allocator_proc(mode, size, old_size, old_memory, allocator_data);
    }
    return kernel_default_allocator_proc(…);
}
```

Enumerating every `#compiler` (compiler-provided) declaration across five independently vendored copies
of `Runtime_Support.jai`/`Preload.jai` gives exactly four: `write_string`, `write_strings`,
`compile_time_debug_break`, `get_current_workspace`. **No allocator is compiler-provided.** So Jai's
compile-time allocation is Jai code, and the analogous thing here is the VM's own region — which already
existed.

## §1 The refusal is about reaching a host, not about the `#foreign` keyword

`ffi::serves_itself` names the three symbols the bridge answers from the VM's own resources: `malloc`
from its region, `free` as a no-op in a bump allocator, `write` into its capture buffer. The comptime
refusal now consults it, so the two cannot disagree about which symbols those are — the alternative was a
comment, and a hand-maintained claim with nothing enforcing it is the failure this project meets most.

ADR-0006 stands, unchanged, for what it was actually about: compile-time code must not reach **the host**
— arbitrary C with arbitrary effects on the machine doing the build, and, worse for a query system,
arbitrary *dependencies* a memoised result cannot record. A `malloc` served from the VM's own bytes has
neither property.

**This is not the comptime FFI Jai has.** Jai's interpreter dlopens libraries and resolves symbols on
demand, so a real `build.jai` runs `git rev-parse` inside a `#run` — `focus-editor/focus first.jai:4-8`
does exactly that. `#foreign_at_comptime` is still owed and is still the difference.

## §2 Printing: the capture is the whole call, and Jai does the same thing

`write` at compile time fills the capture buffer and returns the byte count, without reaching libc. The
bytes travel out in `ConstResult::output` and the driver emits them.

**Why not print from inside the query.** `file_consts` is memoised. A side effect inside it would appear
on the first build and silently not on the next, which is worse than either always or never. Carried out
instead, so the printing happens exactly when the evaluation does — `jr check` and `jr build` both emit
it, before the diagnostics, which is the order it happened in.

**Jai reached the same design for the same kind of reason**, which is the strongest confirmation available
here: `write_string` is one of its four `#compiler` procedures, and the comment says why —

> write_string is marked #compiler because, if called at compile-time, it involves a different
> implementation that also syncs with the compiler's output.
> — `focus-editor/focus modules/Runtime_Support.jai:336-340`

Two compilers, independently, special-case the same primitive at compile time so that output goes through
the compiler rather than around it.

## §3 A module's folds are not a cycle, and were empty anyway

Getting from "allocation works" to "print works" needed four more things, and every one was a claim about
the code that had stopped being true.

`file_consts` lowers every imported module's bodies so a `#run` can call them, and it passed
`OperatorCalls::new()`, `FilledArgs::new()` and `ConstValues::new()` for each. The last of those really
cannot come from the module's own `file_consts` — an import cycle is legal here
(`tests/corpus/imports/valid/005-import-cycle-is-legal.jr`, with `Cycle_A` ↔ `Cycle_B` as fixtures), so
that call would be a salsa cycle, which is exactly what ADR-0018 §3's separation prevents.

**But almost nothing in a `ConstValues` needs evaluating.** Which pointer type a `typed` call produces,
which opcode an atomic is, which call sema folded, which `Type_Info` describes a type, how a variadic
packs — every one is a fact `checked` established, and the line that builds each module's frontend
**already calls `checked` on it** for `types`. So `record_checked_folds` now fills them, for the root
file and for every module, from one function so the two cannot drift.

What stays absent for a module is a value that genuinely needs *running* that module's own `#run`. A body
needing one is still refused — and now says which body and why, rather than
`no routine for file 1 proc 6`.

## §4 A refused body in a module has to be nameable

`add_file` skips a refused body, so calling one reached the interpreter as
`internal compiler error: no routine for file 1 proc 6` — the **eleventh** instance of the
leaked-internal-error shape this project tracks, and the least actionable of them: neither number means
anything outside the database's load order, which is why snapshots never print a `FileId`.

The root file's refusals were already collected. A module's were not, for no reason other than that
nobody had called one. They are now, named by procedure, and preferred over the VM's own message when
this file has no refusal of its own. That one change is what turned four rounds of guessing into four
minutes: each fix produced a diagnostic naming the next blocker.

## §5 A constant whose value is a literal needs no evaluator

`talloc` reads `TEMP_REGION_SIZE`. `out_byte` reads `OUT_CAPACITY`. With an empty `ConstValues` every
standard-library body reading one of its own constants was refused — and `4096` is already a value.

`literal_const_of` interns it directly, and **both** `scan_name` and the lowering site use it. That
pairing is load-bearing rather than tidy: if `scan` admitted a body the emit site then lowered to
`Rvalue::Undef`, the result would be a *legitimate value*, invisible to the verifier and to ADR-0017 §4's
poison gate — this project's first named failure mode.

The case that genuinely needs evaluating — a constant computed by a `#run` — is still refused, honestly.
That split is the whole fix; it is not "give const-eval a checked view", which the cycle forbids.

## §6 ADR-0053 §2's cycle reason had expired

The two remaining empty maps were the root file's own, with this comment:

> **Empty, deliberately.** Const-eval runs before `checked`, so the overload map does not exist yet — and
> asking for it here would make const-eval depend on the check phase, which is the cycle ADR-0018 §3
> avoided.

`file_consts`' **first statement** is `let checked_file = checked(db, file, search_paths)`. The dependency
has been there since `type_info` needed sema's fold maps, so reading two more fields of a result already
in hand adds no edge and cannot introduce a cycle. The comment was true when written and became false
without anyone re-reading it.

**What it cost.** An operator overload and a default argument were unusable in a `#run`, both recorded as
owed. And a **variadic** was worse than refused: `print` in a `#run` gave
`internal compiler error: called a procedure taking 3 arguments with 2`, because with no packing the
trailing arguments went raw. ADR-0053 §2 claimed `scan` would refuse such a body instead — true for a
default argument, never true for a variadic, and nothing had ever run one.

`variadic_calls` and `soa_fields` were also copied by `optimized_file_mir` and not by const-eval. Two
paths populating one structure differently is the defect underneath all of this, which is why §3's
function exists.

## §7 The host is ambient for a `#run`, and that is a stated contract rather than a shrug

A `#run` is evaluated inside a salsa query. A query's arguments are its identity, so a `&mut dyn Host`
cannot be one: it is neither hashable nor comparable, and making it part of the key would mean a different
host produced a different memo.

So the host is installed for the duration of one build and read by whichever VM needs it. That is a side
channel the query engine cannot see, and the contract making it sound is a property of the caller: **the
driver builds a fresh `JairsDatabase` per build**, so `file_consts` runs exactly once per file and a
request is recorded exactly once. What would break it is a long-lived database — an editor session —
where a query may be invalidated and re-run; `jr-lsp` installs no host, so a `#run` build script in an
editor gets the ordinary refusal rather than silently building something.

`with_ambient_host` takes the host *out* of the slot for the duration of a call, which is what makes
re-entry safe: `Compiler.build` compiles a target, whose own `#run`s evaluate, and those find no host.
That is right rather than merely safe — a target's `#run` is a different program and was not asked to
build anything.

## §8 `Compiler.build` cannot run inside a `#run`, and salsa says so out loud

Measured: `thread 'main' panicked … Cannot change database mid-query`. A compilation needs its own
`JairsDatabase` and one is already open.

So a `#run` calls `Compiler.request_build`, which **declares** the target, and the driver builds it once
const-evaluation has finished. `Compiler.build` — the immediate form, which returns a `bool` — is refused
in a `#run` with a message naming the alternative.

**Refused rather than deferred-and-reported-as-success.** A `bool` meaning "queued" is
indistinguishable from one meaning "built", so a script branching on it would branch on nothing. The
deferring form returns nothing precisely because there is nothing to know yet.

**Jai has the same division, and reading it corrected this ADR's first draft.** `add_build_file` only
queues source into a workspace the script created; the *compiler* parses, typechecks, generates and links.
But it does **not** wait for the `#run` to return — the script blocks in `compiler_wait_for_message()`
while the compiler works on its own threads, so by the time `compiler_end_intercept(w)` returns the binary
is linked, and scripts go on to patch icons and build a `.dmg` *inside* the same `#run`
(`focus-editor/focus first.jai:101-166,192-212`).

That difference is exactly ADR-0153 §1's rejected poll. Jai can interleave because its compiler is
threads and a message queue; this one is a memoising query engine, where a compilation observed halfway
through is the thing that cannot be allowed. So the shapes agree and the *ordering* does not, and the
honest statement is that a `#run` here declares and does not observe.

## Decision

1. The comptime foreign refusal is keyed on `ffi::serves_itself` — whether a host is actually reached —
   not on the `#foreign` declaration. `malloc`, `free` and `write` are available at compile time.
2. A comptime `write` is capture-only; the bytes travel out in `ConstResult::output` and the driver emits
   them, so a memoised evaluation cannot print inconsistently.
3. `record_checked_folds` fills every fold a `checked` result knows, for the root file **and** every
   imported module, from one place.
4. A constant whose value is a literal is lowered without const-eval, by a predicate both `scan_name` and
   the emit site use.
5. A refused body in an imported module is reported by name.
6. A `#run` reaches the driver through an ambient host, installed per build against a fresh database.
7. `Compiler.request_build` declares; `Compiler.build` compiles immediately and is refused in a `#run`.

## Rejected

- **Supplying a module's own `file_consts`** (§3): a genuine salsa cycle, because import cycles are legal.
- **Reusing `file_mir` for module bodies** (§3): the same cycle, one query further out.
- **A thread per nested build**, to escape salsa's per-thread database attachment (§8): it would make a
  `#run` observe a compilation in progress, which is ADR-0153 §1's rejected poll wearing a different hat.
- **`Compiler.build` deferring and returning `true`** (§8): a `bool` that means two things.
- **Making the host a query argument** (§7): it is neither hashable nor comparable, and a host in the key
  would mean a different host produced a different memo.
- **Printing from inside the query** (§2): a memoised side effect appears once and then never.
- **`#foreign_at_comptime`**: still owed, still not needed for this, and now known to be what Jai actually
  has — its interpreter dlopens libraries, so a `build.jai` shells out inside a `#run`.

## Consequences

A `#run` can allocate, build strings, print, read reflection, use an operator overload, a default
argument, a variadic and an `#soa` field — and declare a build. `examples/11-run-build-script.jr` is a
build script with **no `main`**, which is the shape the decider asked for, and it builds a real binary.

Tests **1090 → 1096**: two `jr-vm` tests on the refusal's two halves, one `jr-mir` test on the literal
constant, and four `jr-cli` integration tests — comptime print, a `#run` script with no `main`, the
immediate-build refusal, and one asserting that **`jr check`, `jr run` and `jr build` all** emit what a
`#run` printed. That last one exists because `jr run` did not, and the inconsistency was invisible until
the same file was run two ways: a `#run`'s output must not depend on which command reached it. Four existing tests had their **premises expire** and were retargeted rather
than weakened — a foreign call that really is foreign, a constant that really needs evaluating — which is
the fourth time this project has recorded that shape.

One MIR snapshot moved: pool ids shifted because more literal constants are interned. No structural
change, and worth noting that a pool id in a snapshot has the same churn property as the `FileId` this
project already refuses to print.

No new diagnostic code. **E0296 is still the first free one.**

Owed, unchanged: `#foreign_at_comptime`, which is the remaining difference from Jai — a `#run` here still
cannot shell out or read a file, and `examples/10-build-script.jr` is the `main`-shaped spelling that can.
