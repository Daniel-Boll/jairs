# ADR-0205: `modules/Math` was bound to the wrong library, and macOS made it invisible

- **Status:** Accepted
- **Date:** 2026-09-05
- **Deciders:** dboll
- **Amends:** PLAN §1.5's `jr-driver` row and §8.1's `#must` sentence, both stale; the README's and
  `docs/capabilities.md`'s "x86-64 Linux is unverified" claim, which was true only because nobody had
  looked.

## Context

`PLAN.md` §7 has carried the same item for several waves: *"x86-64 Linux is still unverified. The CI
matrix has been triggered; nobody has read the result."* It was the only platform claim in the README
with no observation behind it.

I read it. **The Linux leg was failing**, and had been:

```
/usr/bin/ld: modules/Math/module.jr:837: undefined reference to `sin'
/usr/bin/ld: modules/Math/module.jr:838: undefined reference to `cos'
/usr/bin/ld: modules/Math/module.jr:901: undefined reference to `sqrt'
/usr/bin/ld: modules/Math/module.jr:1014: undefined reference to `acos'
collect2: error: ld returned 1 exit status
```

**66 of 67 test targets passed.** The one failure was `every_corpus_program_behaves_identically_in_both_engines`,
the only test that links natively. So Linux was *narrowly* broken, not unported — which is worth
stating, because "the Linux leg is red" and "this compiler does not work on Linux" are very different
claims and the first was being read as the second.

## 1. The module's own documentation named the bug

`modules/Math` declared its routines against **libc**:

```jairs
sqrt :: (x: float64) -> float64 #foreign libc "sqrt";
```

while its module docs, since ADR-0114, have said:

> `sqrt`, `sin`, `cos`, `exp`, `ln`, `powf` are **`#foreign` wraps of libm**

The docs were right and the code was wrong. **On macOS the difference does not exist**: the math
functions live in `libSystem`, and `libm.tbd` is a symlink to `libSystem.tbd`, so `-lc` resolves
`sqrt` and `-lm` resolves it too. On glibc they are separate libraries and `-lc` alone leaves the
symbols undefined.

This is the **seventh** instance in this project of a hand-maintained claim that nothing enforced —
after the E0290 code collision, `file_consts`' feature list, `checked_expanded`'s "`#insert` adds no
items", `callee_sig`'s "this crate does not hold them", `TrapKind::ALL`'s length assertion, and
ADR-0184's `ItemId` re-keying. The shape is identical every time: **a comment and the code disagree,
and the only machine that runs is the one where the disagreement is invisible.**

## 2. The fix is one library name, and the per-OS machinery was reached for and refused

The obvious shape was `modules/GL`'s: a `#run` returning a declaration per operating system, spliced
by a file-scope `#insert` (ADR-0184). That is what this wave started to build.

**Then it was probed, and it is not needed.** `-lm` links on all three targets:

| | |
|---|---|
| macOS | `libm.tbd -> libSystem.tbd` — verified by compiling `cc m.c -lm` and reading the symlink |
| Linux | libm *is* the math library; this is the missing flag |
| Windows | MinGW ships a stub `libm.a` for exactly this portability idiom |

So `modules/Math` declares one name — `libm :: #system_library "m";` — and a three-branch generator
whose branches all produce the same text would be **machinery pretending to make a decision**. The
Windows question that looked like a fork worth asking about dissolved with it.

Habit paid again, and in the *opposite* direction from usual: normally probing shows a plan
underestimated the work. Here it showed **this ADR's own first plan overestimated it**.

## 3. The VM had an allowlist, and its reasoning argued for the change

Changing the name broke compile-time execution immediately:

```
error: `#system_library "m"` cannot be loaded yet; only "c" is available
```

`jr-vm`'s `symbol` resolves a `#foreign` declaration from **the compiler's own process image**
(`Library::this`), and refused any library but `"c"` — with a good reason, stated in its docs: finding
`write` in the process while the program asked some *other* library for it would be "a wrong answer
dressed as a working one".

**That reasoning admits `"m"` rather than excluding it.** The test is not whether the name is
familiar; it is *whether this process can be made to contain that library's symbols*. Nothing
guarantees SDL2 is loaded, so a hit there would be luck and a miss a confusing error — which is what
the guard is for.

> **The first version of this section claimed "Rust's standard library links both `libc` and `libm` on
> every Unix target". That is false, and §4c corrects it.** It is true on macOS, where both are
> `libSystem`, and that is precisely why it read as universal.

The allowlist is now a named list of two rather than a literal comparison, so the reason above governs
every entry and adding a third is one edit in one place.

## 4. What the test asserts, and why the negative half is the important one

`libm_resolves_and_an_arbitrary_library_does_not` checks both directions:

- **Positive:** `sqrt(4.0)` returns `2.0`, asserted on the **value**. A resolver that found some other
  `sqrt`, or returned zero, would pass an "it did not fail" check.
- **Negative:** a declaration naming `SDL2` is still refused, and the message still names it.

Without the negative half the fix could quietly become "allow every library", which is the thing the
guard exists to prevent. Verified by reverting the allowlist and watching the positive half fail.

**What it does not claim:** this test runs on macOS, where libm and libc are the same object, so it
cannot prove the Linux link. Only CI can, and reading CI is the other half of this wave.

## 4a. The first fix was necessary and not sufficient, and CI said so

Pushing the `modules/Math` fix and reading the next run is what turned this from a plausible fix into
a verified one. The Math errors were **gone**. A *different* file failed:

```
tests/corpus/valid/093-ffi-floats.jr:40: undefined reference to `sqrtf'
```

That file declares its own `libc :: #system_library "c";` — corpus programs do, because the
type-checking harness does not load `Basic` — and bound `sqrt`, `sqrtf` and `pow` to it.

**CI showed one file because it stops at the first.** So rather than push again and find out, the whole
tree was scanned for math symbols bound to a non-libm library: **four sites, two files**. One of the
four was a false positive worth knowing about — `valid/091-math.jr` *quotes* `#foreign libc "sqrt"` in
its prose while explaining why `Math` had no transcendentals, so any scanner here must strip comments
or it flags documentation.

**And that is why the rule is now a test rather than a comment.** `crates/jr-cli/tests/libraries.rs`
asserts that every math symbol in every `.jr` file is declared against a library named `m`. This would
have been the seventh hand-maintained claim in this repository that nothing enforced; the difference
between the six before it and this one is that the invariant is now checked on the machine that runs.

Its second test asserts the scanner reads declarations and **not** prose, with `091-math.jr` as the
witness — because a scanner that silently matched nothing would let the first test pass by finding no
violations at all, which is the failure mode a scanner-based check actually has.

## 4b. And a third failure, of a different kind: a library CI did not have

The second push cleared both math files. CI failed again, on a third program and for an unrelated
reason:

```
tests/corpus/valid/137-per-os-library-text.jr:  /usr/bin/ld: cannot find -lGL
```

**This is not the same bug.** The library name is *correct* — `modules/GL` selects
`#system_library "GL"` on Linux (ADR-0184) — and the runner simply has no OpenGL development package.
macOS ships `OpenGL.framework`, so the macOS leg never noticed.

Worth stating why the tree scan did not predict it: **`modules/GL` has no literal
`#system_library` in its source.** The declaration is generated by `#insert #run gl_library();`, so a
text scan sees nothing and only a compile knows. A scanner is the right tool for the libm rule and the
wrong one for this, which is the boundary between the two failures.

The corpus's real system-library needs were then measured rather than guessed, by taking each
program's transitive import closure: **`c` (126 programs), `m` (6), and OpenGL (exactly 1)**. Nothing
in `valid/` reaches `Window`, `Simp`, `Input`, `Image` or `UI`, so **SDL2 is deliberately not
installed** — putting a library on the runner that nothing needs is the same class of mistake as
naming one that does not contain the symbol.

**Rejected: making `jr-link` omit a library whose symbols are never referenced.** `137` imports `GL`
only to call `gl_library_for`, a compile-time text function; it never calls an OpenGL routine, so
under that rule the link would need nothing. It is a real improvement and it is **owed rather than
taken here**, because "only link what you use" is a behaviour change whose failure mode is a silently
missing library — not something to land inside a change whose purpose is turning a red leg green.

## 4c. A fourth failure, and it falsified this ADR's own §3

The third push cleared the link entirely: native built and ran correctly on Linux. The **VM** then
failed on six programs:

```
error: the foreign symbol `sqrt` was not found in this process
```

**So §3's premise was wrong.** On macOS every one of these symbols is in `libSystem`, which is always
loaded, so searching the process image always worked and the rule read as though it were universal. On
glibc `libm.so.6` is a **separate** library and `--as-needed` leaves it out of this binary's
dependencies unless Rust itself needed it — so `sqrt` was genuinely absent from the process, and
compile-time math failed on Linux only.

That is the same shape as the bug this ADR is about, one level up: **a claim that holds on the only
machine anyone runs.** Writing it down did not make it true; the CI leg did.

**The fix loads the library the declaration named**, which is what the refusal has always promised by
saying "cannot be loaded *yet*". Process image first, so the common case still costs no `dlopen` and no
platform where the image already answers can change behaviour; the named library second.

Two decisions inside it worth stating:

- **A fixed table of filenames, not a pattern.** `lib{name}.so` would quietly widen the allowlist the
  moment a third name is admitted for some other reason. `libc` is deliberately absent from the table:
  it is always loaded, so a `dlopen` for it could only be a slower route to the answer the image
  already gave.
- **A test asserts a candidate filename actually opens, with `sqrt` in it.** This is the assertion no
  amount of reading replaces: the fallback is only *taken* on glibc and is dead code on the machine it
  was written on, so a typo'd soname would have been invisible until CI — which is how this ADR spent
  four pushes. Whichever platform runs the suite checks its own name.

## 4d. A fifth failure — this one entirely mine, and its symptom was silence

The fourth push made the VM load libm. CI came back with the VM at `exit -1`, empty output, on five
programs. Not a refusal, not a diagnostic: **a crash**.

The cause was a two-line mistake in the previous section's fix. `LibraryHandle::new` returns a handle
whose `Drop` calls `dlclose`, and the handle fell out of scope at the end of the loop body — **while
the address it had just produced was being returned**. The library was unmapped under the pointer, so
the first compile-time math call jumped into nothing.

Three things about it are worth keeping:

- **The symptom was silence.** `exit -1` with empty stdout and stderr is what a signal looks like
  through the differential harness. A refusal would have named the symbol; a crash names nothing, which
  is a good reason for the harness to report the exit status rather than only the output.
- **macOS cannot reproduce it.** The fallback is never taken there, so the bug existed only on the
  platform with no local machine — the same asymmetry as the original libc/libm defect, one level up
  again.
- **The library is now leaked deliberately.** A cache was considered and rejected: it adds a mutex to a
  path whose purpose is to run once per distinct symbol, and a system library loaded for the remainder
  of the process's life is what `dlopen` is *for*.

`libm_resolves_and_an_arbitrary_library_does_not` **would** have caught this on Linux — it calls `sqrt`
through the whole VM and asserts the value is `2.0`, and calling a dangling pointer does not return an
`Err`. That it passes on macOS without exercising the path is now stated in its own doc comment, so the
next reader does not mistake a green run here for coverage of that path.

## 6. A sixth failure, and it was never libm: `modules/Socket`'s platform commitment

With the math programs agreeing, the differential went from **6 disagreements to 1**. The last one was
`129-sockets.jr`, and it had nothing to do with this ADR's subject: the *native* build crashed on Linux
while the VM was fine. The link failure had been masking it for waves.

`modules/Socket`'s own docs named the cause, in the same shape as `Math`'s:

> **The layout below is macOS's** … BSD has `{ uint8_t sin_len; sa_family_t sin_family; }` where Linux
> has `{ uint16_t sin_family; }`. So a Linux build needs `family` widened to 16 bits and `length`
> removed — **two lines, and wrong silently until then**.

Plus `SOL_SOCKET`, which the same paragraph records as `0xffff` on macOS and `1` on Linux.

**Eighth instance of the pattern, and the second in this one ADR.** A documented platform difference,
never implemented, invisible on the only machine that runs.

**It is a value difference, not a layout one** — which is the finding that made it small. Both
structures are 16 bytes; only the first two bytes' *meaning* differs. So no per-OS struct is generated:

| | byte 0 | byte 1 |
|---|---|---|
| BSD | `sin_len` = 16 | `sin_family` = 2 |
| Linux | `sin_family` low = 2 | high = 0 |

Both routines take the OS as a **parameter**, `modules/GL`'s convention (ADR-0184) and for its stated
reason: a function reading `os()` internally has one executable path per machine, so the Linux branch
would be text no test here could look at. `tests/corpus/valid/149-per-os-socket-values.jr` asserts both
branches on any host, asserts they **differ** — the mistake a reader would really make is editing one
and leaving the other aliased — and ties the parameterised functions to the value the module compiled
with.

**And writing that test found a gap.** `size_of(Sock.Sockaddr_In)` is E0261, "`size_of` needs a type":
a qualified name is accepted in a type *annotation* (ADR-0179 §5) but not as an intrinsic's type
argument. The assertion was dropped rather than worked around with an unqualified import, because an
import style chosen to dodge a compiler gap hides the gap. **Owed.**

## 5. Three stale claims corrected while here

Found by checking the plan against the code rather than by any failure:

| claim | reality |
|---|---|
| §1.5: `jr-driver` is "Not started… still a one-line stub" | **1430 lines** — `build.rs` 474, `script.rs` 919 |
| §8.1: "`#must` … is still owed its own ADR" | It has one: **ADR-0151** |
| README / capabilities: "x86-64 Linux is unverified" | It is **verified as failing**, which is worse |

The third is the one worth dwelling on. "Unverified" is an honest label for an unknown, but it decays
into a place to put a known problem — the CI answer existed for waves and the label kept reading as
though it did not. **A claim that something is unverified needs an expiry, or it becomes the reason
nobody checks.**

## Consequences

`modules/Math` links `-lm`, so the four undefined references are resolved on glibc. `#import "Basic"`
is **gone** from that module: it was imported for exactly one thing, `libc`, and E0231 said so the
moment `libm` was declared locally — the unused-import warning earning its keep on a real module.

Both engines still agree on every Math corpus program, at the same exit codes as before the change.

**One interned item churned 24 snapshot lines, and that is worth knowing.** Declaring
`libm :: #system_library "m"` adds one `ForeignLibraryValue` to the pool, which shifted every later
`PoolId` by exactly one — so `Type_Info.id` moved 839 → 840 in `mir_corpus__valid_corpus_mir.snap`,
in 24 places, with no structural change at all. `AGENTS.md` already says never to print a `FileId`
into a snapshot, because it is an index assigned in load order and one new corpus file renumbers every
occurrence. **`Type_Info.id` has exactly that property** and is printed there. The churn is benign and
was accepted, but a reader who sees a 24-line snapshot diff on a one-line library change should know
it means "an item was interned" rather than "the MIR changed".

### Owed

- **The Linux fix is verified by CI, not locally.** No Linux machine is available here, so every claim
  about glibc rests on a run being read. Three have been, and each exposed the next: the `Math` fix
  revealed `093-ffi-floats.jr`, whose fix revealed `137-per-os-library-text.jr` needing a package CI
  did not install. **Reading one run is not verification; reading until it is green is.**
- **`jr-link` links a library whose symbols are never referenced** (§4b). `137` imports `GL` for a
  compile-time text function and links `-lGL` for it. Omitting an unused library is the right
  behaviour and a wave of its own, because its failure mode is a silently missing library.
- **`size_of` rejects a qualified type name** (§6). `size_of(Sock.Sockaddr_In)` is E0261 while
  `x: Sock.Sockaddr_In` is fine, so an intrinsic's type argument does not accept what a type annotation
  does. One arm, by the shape of ADR-0191 and ADR-0192, but not this ADR's subject.
- **SDL2 is not installed on the Linux runner**, because nothing in `valid/` reaches it today. The day
  a corpus program imports `Window`, that step needs `libsdl2-dev` — and the failure will say so.
- **`libm` is not declared in `modules/Basic`** beside `libc`, deliberately: only `Math` needs it, and
  a second module declaring a library it does not use is how the wrong-library bug started.
