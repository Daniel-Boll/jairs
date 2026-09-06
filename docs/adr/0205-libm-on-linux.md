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
familiar; it is *whether this process is guaranteed to contain that library's symbols*. Rust's
standard library links both `libc` and `libm` on every Unix target, so resolving `sqrt` from the image
when the program asked libm for it is the **right** answer. Nothing guarantees SDL2 is loaded, so a
hit there would be luck and a miss a confusing error — which is what the guard is for.

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

- **The Linux fix is verified by CI, not locally.** No Linux machine is available here, so the claim
  in this ADR rests on the next run being read — which is now a habit this wave established rather
  than a hope.
- **`libm` is not declared in `modules/Basic`** beside `libc`, deliberately: only `Math` needs it, and
  a second module declaring a library it does not use is how the wrong-library bug started.
