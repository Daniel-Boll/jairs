# ADR-0206: System V's aggregate rules, and the claim that a `CGRect` travels in registers everywhere

- **Status:** Accepted
- **Date:** 2026-09-06
- **Deciders:** dboll
- **Amends:** ADR-0160 §"Why an HFA is not size-limited" (withdrawn) and its single `Class::Memory` variant

## Context

ADR-0205 got the x86-64 Linux CI leg from "fails at link" to one failing test:

```
aggregates_cross_a_foreign_boundary_as_a_c_compiler_expects
  left: Some(15)   right: Some(31)
```

Four of five aggregate shapes agreed with a `cc`-compiled shim. The fifth is `rect_total(Rect)`, where
`Rect :: struct { origin: Point; size: Point; }` is **thirty-two bytes of four `float64`s** — the `CGRect`
shape W10's graphics work is built on.

**This is the wave ADR-0160 said would need the Linux run first**, and it named the prerequisite in those
words. The run now exists.

## 1. The claim that was wrong, and it was load-bearing

`crates/jr-pool/src/cabi.rs` carried a section headed *"Why an HFA is not size-limited"*:

> A `CGRect` is `{ CGPoint origin; CGSize size; }` — four `float64`s, thirty-two bytes, and an HFA. **Both
> AAPCS64 and System V pass it in four floating-point registers**, and a size test would send it to memory
> and break every graphics call W10 needs.

**The first sentence is right and the second is false.** System V on x86-64 has **no homogeneous-aggregate
rule at all** — there is nothing to be unlimited. It classifies per *eightbyte* and sends anything over
sixteen bytes to `MEMORY`, on the stack. AAPCS64 is the ABI with HFAs, and its four-member limit is indeed
about members rather than bytes.

So for eleven waves this compiler passed a 32-byte struct in four SSE registers on x86-64 where C reads it
from the stack. **A silent miscompile**: no diagnostic, no crash, wrong data — and invisible on the arm64
machine every wave of this project has been developed on.

**Seventh in this project's family of hand-maintained claims with nothing enforcing them**, after the E0290
collision, `file_consts`' feature list, `checked_expanded`'s "`#insert` adds no items", `callee_sig`'s "this
crate does not hold them", `TrapKind::ALL`'s length assertion and ADR-0205 §3's "Rust links both libc and
libm". **It is the most expensive of the seven**, and the reason is worth stating: the other six produced a
diagnostic — a refusal, an ICE, a link error — and a diagnostic sends someone to the line. This one produced
a plausible number.

**What caught it was a test that links against a different compiler's output.** The shim is built with `cc`
at `-O1` precisely so a wrong answer cannot be self-consistent; ADR-0160 wrote that reasoning down and it is
what paid. **What delayed it eleven waves was that nobody ran that test on the target the claim was about.**

## 2. `Class::Memory` split, exactly as ADR-0160 predicted

That ADR refused to pass a `Memory` aggregate at all, and its reasoning was precise:

> One case with two correct answers is a case that has to be refused until it is split.

The two cases were a **large composite**, where an indirect pass is right, and a **small mixed** one, where
System V interleaves two register files and AAPCS64 does not. The Linux run is what splits them, so:

- **`Class::Stack { size }`** — System V's `MEMORY` past sixteen bytes. One correct answer, and Cranelift
  already models it as `ArgumentPurpose::StructArgument`. **Implemented.**
- **`Class::Refused`** — what is left: a mixed aggregate *inside* sixteen bytes, and a float aggregate whose
  members share an eightbyte. Both need a per-eightbyte register assignment this `Class` vocabulary cannot
  express. **Still refused**, and the diagnostic now names both targets' shapes.

Renamed rather than extended under the old name. `Memory` had meant "will not pass it", which is the
opposite of what `Stack` does, and a variant whose name says memory while its sibling implements memory is
how a reader learns the wrong thing.

## 3. The rules, and the one that surprised me

`CAbi` selects them, kept off `TargetLayout` deliberately — that describes pointer *width*, identical LP64
on both targets, and threading an architecture through `layout_of` would suggest a dependency the layout
computation does not have.

**AAPCS64 is exactly today's behaviour**, unchanged, so arm64 is byte-identical: HFA of up to four members
with no size limit; all-integer up to two words; everything else refused.

**System V**: over sixteen bytes → `Stack`. Within it, in registers only when **every member owns its own
eightbyte**.

That last clause is the one I got wrong first, and the unit test caught it. I wrote the guard as "the members
exactly fill the layout", which is true of `struct { float a; float b; }` — eight bytes, two members, four
bytes each — and that struct is **one** SSE eightbyte holding both floats, so C reads both from `xmm0` while
two `Class::Float` members would emit `xmm0` and `xmm1`. **A stack copy would be wrong too**: eight bytes
belongs in a register. Neither available answer is right, which is precisely when this compiler refuses. The
correct guard is `count == 1 || kind.bits == 64`.

**A `StructArgument` takes the address, not the contents.** Cranelift's x64 lowering copies `size` bytes from
that pointer into the outgoing argument area itself, so the call site pushes the address and loads nothing —
the one place in `push_aggregate_pieces` where a class does not become one value per register.

**A `Stack` *return* needed no new code at all.** System V returns a `MEMORY` aggregate through a hidden
pointer, which is the convention `returns_via_sret` already describes for Jairs's own aggregate returns
(ADR-0051 §1). The signature builder answers `None` — "not in registers" — and the existing block pushes the
`StructReturn` parameter. One convention reached by two routes, rather than a second implementation.

## 4. A buffer overflow this would have introduced, caught by reading the SAFETY comment

`jr-vm`'s foreign-return path reads into a fixed `[u8; 32]`, and its `// SAFETY:` block justified the size
like this:

> whose size is at most thirty-two bytes **because `classify` answered a register class**

That is an invariant *inferred from a property of the classification* — and `Class::Stack` is exactly the
change that stops it holding. libffi writes `layout.size` bytes into that buffer, so a `Stack` return of
forty bytes would have overflowed it. The `min(32)` on the line below truncates only the *read*.

The bound is now **checked**, with a refusal naming the size, and the SAFETY comment cites the check instead
of the classification. Worth generalising: **when a SAFETY comment justifies a bound by appealing to another
module's behaviour, changing that module is a memory-safety change.** Nothing in the type system connected
those two files.

## 5. A `_` arm is a silent opt-out from this project's strongest guarantee

`jr-sema`'s `aggregate_refusal` — the E0286 gate — read:

```rust
match jr_pool::classify(...) {
    Ok(Some(Class::Integer { .. } | Class::Float { .. })) => None,
    _ => Some(refusal),
}
```

Adding `Class::Stack` made that wildcard **silently refuse it**. Every program using a large aggregate at a
`#foreign` boundary would have been rejected before reaching the back end that now implements it, and only
the *arity* change made the compiler mention this file at all.

`AGENTS.md` already records this hole in its `let-else` form — ADR-0186's
`let PlaceBase::Slot(slot) = place.base else { ... }`, which skipped globals by luck. **This is its other
form, and it is more common.** The rule this earns: the exhaustive-match discipline is only as good as the
absence of `_` arms, and a *refusal gate* is the worst place to keep one, because the symptom is a
diagnostic on a program that should have built — which reads as the checker doing its job.

Both sites that had one are now exhaustive.

## 6. What is deliberately not built

**LLVM refuses `Stack`**, with a message naming the gap and pointing at the default back end. In LLVM's
vocabulary this is `byval` for a parameter and `sret` for a return — attribute work, not a type change, so
it is a real gap and not a hard one. It is refused rather than written blind because **nothing could run
it**: `classify` never answers `Stack` on AAPCS64, so gate 7 on arm64 cannot exercise a line, and CI compiles
no LLVM at all on x86-64. Unverifiable code at an ABI boundary is the shape that produced this ADR.

**`{ float, float }` on x86-64 is refused** rather than packed. A packed `<2 x float>` needs a vector type in
`Class`, and no program in this tree passes such a struct across a `#foreign` boundary — verified by forcing
`CAbi::SysV` on this arm64 host and checking all 149 corpus files and every module clean.

**Over-sixteen-byte aggregates on AAPCS64 stay refused.** They are passed *by reference* there, which is a
third convention, and implementing it would change arm64 behaviour that is currently green — for no test.

## 7. How this was verified without an x86-64 machine

**By forcing the classification and running locally.** One line — `CAbi::host()` returning `SysV` on
aarch64 — turns every local check into an x86-64 classification check:

- All 149 corpus files and every module still check clean, so nothing newly refuses.
- The aggregate test then failed with `StructArgument parameters are not supported on arm64`, **from inside
  Cranelift** — which proves the whole path end to end: `Rect` classifies as `Stack`, the signature builder
  emits `StructArgument`, and Cranelift receives it. That the panic comes from `isa/aarch64/abi.rs:249`
  while `isa/x64/abi.rs:157` implements the same purpose is not a coincidence — arm64 has no convention this
  models, which is why `classify` never produces it there.

That probe replaced a five-minute CI round trip per iteration with a five-second one, and it is the reason
the eightbyte-ownership bug was found before pushing rather than after.

## 8. The snapshot that had been wrong on Linux since ADR-0180, and nobody could see it

Fixing the ABI made the Linux leg report one more failure, and it is **not** this ADR's subject — it is an
eighth pre-existing defect, in the same family as ADR-0205's six:

```
snapshot assertion for 'valid_corpus_mir' failed
-    v31: string = call extern proc33(v0, 0_enum)
+    v31: string = call extern proc33(v0, 1_enum)
-    v35: s64 = call extern proc14(v0, 0_enum)
-    v36: bool = 65535_s64 == v35
```

`0_enum`/`1_enum` is `Operating_System.MACOS` against `LINUX`; `65535`/`1` is `SOL_SOCKET`. **`os()` is a
compile-time value** (ADR-0180 §2), folded in sema — so a corpus program that reads it has the host's answer
as a *literal* in its MIR, and a single checked-in snapshot cannot be right on two platforms.

**The first hunk belongs to `137-per-os-library-text.jr`, added in ADR-0180's own wave.** So this snapshot
has been wrong on x86-64 since the feature landed, and only macOS ever generated it. ADR-0180 introduced a
compile-time value and did not notice it had made a cross-platform artifact host-specific.

Two shapes, and they need opposite treatments:

- **`137` and `149` compare the mapping's answers for *named* operating systems** and ended with one line
  tying the mapping to the host — `gl_library_for(os())`, `SOL_SOCKET`. That line is the only host-dependent
  thing in them, and it is **moved** into `crates/jr-cli/tests/integration.rs`, where `cfg!(target_os = ...)`
  states each platform's expectation — the thing a `.jr` program cannot do — and no snapshot sees the MIR.
  Both directions are asserted, so a mapping returning one value for every OS fails instead of passing by
  coincidence. Verified to have teeth by swapping the expectation and watching it fail.
- **`134-target-os.jr` and `135-per-os-clock.jr` exist to read `os()`.** The dependence is their subject, so
  they are **excluded from the artifact** and keep their coverage from the differential and exit-code
  harnesses, which run per platform.

**The exclusion is a hand-maintained list, which this project distrusts on principle, and the justification
is the direction it fails.** `file_consts`' feature list and `TrapKind::ALL`'s length both failed *open* — a
missing entry meant a check silently did nothing. This one fails **closed**: an unlisted `os()`-folding
program passes on the host that generated the snapshot and **fails the other platform's CI job**, naming the
file. A list whose rot announces itself on every push is a different object from one whose rot is invisible.

**Verified the same way as §7, by forcing the answer.** `TargetOs::host()` forced to Linux, snapshot
regenerated, probe reverted, snapshot re-checked on real macOS: identical. That is a local proof of
OS-independence, and it found that the first fix was incomplete — 137 and 149 were not the only two files.

## Consequences

- Two classes where there was one, and the compiler names every site that must choose between them —
  except where a `_` arm opted out, which is §5.
- arm64 is byte-identical: 1215 workspace tests, same as before plus the six new ABI assertions.
- A `CGRect` crosses a `#foreign` boundary correctly on both targets, which W10's graphics work needs on
  Linux and silently did not have.
- The comptime engine declines a foreign return over thirty-two bytes. Visible, and it was previously a
  buffer overflow waiting for the classification to widen.
- `cabi.rs`'s module docs now state both ABIs' rules and record the false claim, because the next reader's
  instinct will be the same as the one that wrote it.
- **x86-64 Linux CI is green — all seven jobs — for the first time in this project's history**, verified by
  reading run `34010283168` rather than by inference. 1216 workspace tests, 1222 under gate 7.
- Two things are owed on that target and neither blocks anything: LLVM refuses `Class::Stack` (§6), and
  `{ float, float }` is refused rather than packed into one SSE register.
