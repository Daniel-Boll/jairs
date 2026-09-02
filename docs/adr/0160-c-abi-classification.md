# ADR-0160: The C ABI classification for an aggregate — the decision, made once

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **PLAN §8.1.2, part 1 of 2.** This ADR decides *which* aggregate shapes cross a `#foreign` boundary and
  *where* their pieces go. Wiring the three engines to consume it is part 2, specified in §6 below and not
  delivered here.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

§8.1.2 is the project's highest-leverage open item: it blocks **W10 — Graphics** entirely (every windowing and
GPU API passes structs by value), plus `readdir` and `stat` in `File_Utilities` and `getaddrinfo` in `Socket`.
ADR-0150 turned the crash into a diagnostic (E0286) and deliberately left the feature.

The feature has two halves, and they are separable in a way that matters. The **decision** — which shapes are
supported and where each piece goes — is one shared answer that three engines must not each invent. The
**wiring** is three mechanical implementations of that answer. This ADR delivers the first, because it was the
part that was actually undecided, and because landing it alone changes no behaviour: every per-engine refusal
is untouched, so nothing can diverge while the engines are wired in turn.

## Decision

### 1. The classification lives in `jr-pool`, and every engine asks

`jr_pool::classify(pool, target, ty)` answers `Class::Integer { words }`, `Class::Float { kind, count }`, or
`Class::Memory`.

Three engines cross this boundary — the comptime VM through libffi, Cranelift, and LLVM — and each of them
*could* classify a struct itself. That is exactly what must not happen. **A struct in the wrong register is a
silent wrong answer with no diagnostic**, and three implementations of one platform rule are three chances to
disagree. ADR-0020 §2 made this argument about trap messages, where the two engines render at different times;
it applies with more force here, because a mis-rendered message is visible and a mis-placed argument is not.

It lives beside `layout_of`, which it depends on, rather than in a new crate: the classification *is* a
layout question, and a fourth crate for one function would put a module boundary between two things that
change together.

### 2. Two shapes are supported, and they are the two the rules make unambiguous

A **small integer aggregate** — every scalar in it a word (integer, pointer, `bool`, procedure), and at most
two words in total — travels in up to two general-purpose registers. Both AAPCS64 and System V agree.

A **homogeneous float aggregate** — every scalar the *same* float type, at most four of them — travels in up
to four floating-point registers. Both targets agree here too.

**The HFA has no size limit, and that is the point rather than an oversight.** A `CGRect` is
`{ CGPoint origin; CGSize size; }`: four `float64`s, thirty-two bytes, and an HFA. Both ABIs pass it in four
FP registers. A size test would send it to memory and break every graphics call W10 needs — so the limit is
**four scalars**, not sixteen bytes, and that distinction is the whole reason this is a classification rather
than a byte count.

Nesting does not change the answer, and neither does an array: `{ CGPoint, CGPoint }`, four bare doubles, and
`float64[4]` are indistinguishable to a C ABI, so all three flatten to the same class. A `string`, a view and
a dynamic array flatten to words, because a C function taking a `{ char*, long }` by value is a real signature
and `string` is what a caller would reach for to describe it.

### 3. `Memory` is a refusal, not an indirect pass — and that needs saying

An indirect pass (caller copies, passes a pointer) is the correct convention for a **large** composite on both
targets. It is *not* correct for a small **mixed** one, and `Class::Memory` covers both.

System V on x86-64 classifies each eightbyte **independently**: `struct { double a; long b; }` puts `a` in
`xmm0` and `b` in `rdi`, interleaving two register files. AAPCS64 does not — the same struct is not an HFA, is
sixteen bytes, and goes in `x0`/`x1`. So the two targets genuinely disagree about where a mixed struct's
fields live, and implementing that means implementing both classifications in full.

One case with two correct answers is a case that has to be **refused** until it is split. So `Memory` keeps
E0286, with a message naming the two supported shapes so a caller can reshape — pass a pointer, or split the
struct. An honest narrower rule beats a wrong wider one, which is the judgement ADR-0112 made about `sqrt` and
ADR-0157 made about a variadic `open`.

**Rejected: classifying a mixed aggregate as memory and passing it indirectly.** It would compile, it would be
wrong on both targets for the sixteen-byte case, and it would be wrong *silently* — the failure mode this
whole module exists to prevent.

**Rejected: implementing System V's eightbyte classification now.** It is the more general answer and it is a
second ABI's worth of rules, verified against a target this project has never run on (PLAN §1.5: no CI run has
happened). Guessing a second platform's rules from the specification, with no way to test them, is how the
`open` mode ended up in the wrong register.

A **union** or **variant** is `Memory` too: their members overlap, so there is no single scalar sequence, and
every C ABI treats a union's bytes as opaque.

### 4. A padded tail occupies a whole register

`struct { s64 a; u8 b; }` is sixteen bytes after padding and is `Integer { words: 2 }` — the second register
holding one meaningful byte. That is what both ABIs specify, and it is why the class counts **words from the
layout's size** rather than assigning registers per field. A per-field assignment would put `b` in the second
register's low byte on one target and get the offset wrong on the other; loading whole words from a padded
slot is correct on both and needs no per-field reasoning at the call site.

A zero-sized aggregate — a struct with no fields — is one word rather than none. C has no empty struct, so
there is no convention to match, and one word is the shape that cannot corrupt a later argument.

### 5. Thirteen tests, aimed at what a lenient implementation gets wrong

Not at the happy path. The padded tail that still occupies a whole register; the thirty-two-byte HFA that a
size test would reject; its nested and array spellings; five floats, which are one too many; two float widths,
which are not homogeneous however similar they look; the mixed struct that must not be guessed; and a `string`,
which must classify as the two words it is.

### 6. What part 2 is, and why it is separate

Each engine consumes `Class` in its own way, and none of the three is hard now that the answer is shared:

- **The VM** builds an `ffi_type` — `Type::structure` of words or floats — and lets libffi place the pieces.
  This is the engine that needs the *least* work, because libffi implements the ABI itself; what it needs is
  the struct's bytes copied into host memory for the call and copied back on return. The existing marshaller
  is one word per argument, so the argument plan gains a case rather than a rewrite.
- **Cranelift** has no aggregate type, so `signature()` turns `Integer { words: 2 }` into two `I64`
  `AbiParam`s and `Float { kind, count }` into `count` float ones, and the call site loads them from the
  value's slot and stores them back on return. `returns_via_sret` stays the predicate for `Memory`.
- **LLVM** does the same through its own types, and separately: the two back ends share no emission path.

They are separate from this ADR for one reason: **landing the decision alone changes no behaviour.** Every
refusal is where it was, so no engine can diverge from another while they are wired one at a time — and a
half-wired ABI is precisely the silent divergence ADR-0157 §5 and ADR-0158 §3 found the hard way. The wiring
also needs verification against a **real C compiler**, not against itself: `ldiv` returns a sixteen-byte
integer struct and is in libc, and a `cc`-compiled shim covers the parameter direction and the HFA. A test
that only checks Jairs against Jairs would pass with both sides wrong.

## Consequences

- **`crates/jr-pool/src/cabi.rs`** is new, exporting `Class` and `classify`. Thirteen tests; 1053 in the
  workspace.
- **No behaviour change.** E0286 refuses exactly what it refused before, and will keep refusing `Memory` after
  part 2.
- **§8.1.2's design question is answered**, so W10's gate is now a specified implementation task rather than an
  open decision. PLAN records both halves.
- **The `Memory` case is a permanent narrowing, not a temporary one**, unless someone implements System V's
  eightbyte classification against a target this project can run. That is its own decision and wants a Linux
  CI run first — which is itself still owed (PLAN §1.5).
