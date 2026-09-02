# ADR-0161: An aggregate crosses a `#foreign` boundary — part 2, the wiring

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **PLAN §8.1.2, part 2 of 2**, completing what ADR-0160 decided. **W10's gate is open.**
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

ADR-0160 answered *which* aggregate shapes cross a C boundary and *where* their pieces go, in one shared
function, and deliberately changed no behaviour. This wires the three engines to it and moves E0286's line.

It also discharges §8.1.2's consequences: **W10 — Graphics is unblocked**, and `readdir`, `stat` and
`getaddrinfo` become expressible in `File_Utilities` and `Socket`.

## Decision

### 1. The VM describes the struct and lets libffi place it

`ffi_type_of` builds a faithful `libffi::middle::Type` field by field — real widths, so a `u8` field is
`Type::u8()` and not a word — and libffi does the ABI.

**This engine does not consult `classify` for placement at all**, and that is the right shape rather than an
inconsistency. libffi *implements* the C ABI; given a true description it places the pieces correctly,
including the mixed-register cases `Class::Memory` refuses. So the VM's correct move is to describe and
delegate. It consults `classify` in exactly one place — the return path, to bound its result buffer — and
would keep working unchanged if the refusal were ever relaxed.

**Rejected: describing every field as a word.** It would give libffi the wrong size and alignment: a
`{ u8, u8 }` would claim eight bytes where C says two.

An aggregate argument is `Marshalled::Bytes`, a second case beside `Marshalled::Word`, because
`Value::Aggregate` already holds the bytes in target layout. Two cases in the type rather than one word means
`dispatch` *cannot* hand libffi a pointer where the C signature says struct — the mistake that produces a call
which works on one platform and corrupts the stack on the other.

The return comes back into a `#[repr(C, align(16))] struct ReturnBuffer([u8; 32])`. Thirty-two bytes because
four eight-byte members is the largest class; the **alignment attribute is load-bearing**, because
`libffi::low::call` writes into a `MaybeUninit<R>` directly whenever `R` is at least a word wide and a
returned struct is stored from registers — a bare `[u8; 32]` is one-aligned and would be undefined behaviour
on a target that requires alignment for the store.

**This direction has none of ADR-0158 §3's nested-pointer problem**, and the reason is worth stating: the
bytes are *copied*, so a pointer inside them stays region-relative and would still be wrong. A struct of
pointers therefore does not cross, and `jr-sema` is where that is refused rather than here.

### 2. Cranelift turns a class into `AbiParam`s and moves the pieces itself

Cranelift has no aggregate type, so `signature()` emits `words` pointer-width params or `count` floats, and
the body loads them at the call site and stores them back on return.

**Whole words from the start, not per-field loads.** The classification counts words from the layout's *size*,
so a `{ s64, u8 }` is two registers with one meaningful byte in the second (ADR-0160 §4). A per-field load
would compute a different second register on each target and be wrong on at least one.

**The classified returns are pushed after every parameter**, and this was found by the verifier rather than by
reading: the first version returned the signature as soon as it had the results, producing a function with two
returns and *no arguments*. Cranelift reported "mismatched argument count: got 2, expected 0" at the first call
site. Recorded because the shape of the bug — an early return from a builder that has more to append — is one
any later ABI work here could repeat.

`returns_via_sret` stays the single answer for Jairs's own convention, and the C path asks `classify` *beside*
it rather than changing it: a C aggregate return in registers takes no `sret` parameter even though an
aggregate normally does. Both the signature and the call site compute that from the same two functions, which
is what ADR-0051 §1 made `returns_via_sret` exist for.

`Context` gains a `foreign` map. The signature builder does not need it — a signature is built from a
declaration, which knows its own kind — but a *call* has only a `ProcRef`, and without it an aggregate
argument to a foreign call would be passed as the pointer a Jairs-to-Jairs call wants: which compiles, and
puts an address where C expects a struct.

### 3. LLVM emits separate scalars, not `byval`

LLVM would accept the struct type with a `byval` attribute and do the classification itself. It is given
**separate scalar parameters** instead, one per register, matching Cranelift exactly.

The reason is the differential harness: two native back ends that describe the same call differently are two
things to keep in step, and `differential.rs` compares *observable answers* rather than IR. Emitting the same
call from both makes the comparison meaningful. The one place LLVM is allowed to delegate is the **return**,
which is a struct of the class's pieces — because a function returns one value and LLVM's ABI lowering places
its members. That is safe precisely because *which shapes are permitted* was decided before LLVM was asked.

LLVM hands a returned struct back as one value, so its members are `extract_value`d rather than read from a
result list. That is the only place the two back ends differ in shape while agreeing on the ABI, and the
helpers sit beside each other in both crates so the offsets can be compared by eye.

### 4. E0286 moves its line, and asks the same function the engines act on

`foreign_boundary_refusal`'s four aggregate arms now call `aggregate_refusal`, which consults
`jr_pool::classify`. So the diagnostic and the three capabilities **cannot drift** — which is the property
that makes relaxing a refusal safe at all, and the reason part 1 shipped separately.

Each arm keeps its own *wording*: a `string` still gets the sentence about `.data` and `.count` when it is
refused, an array explains that C decays one and Jairs does not, and a dynamic array names its third word. One
rule, four sentences, and a reader comparing them sees both.

A `string` is two words, so in practice it now crosses — which is a real gain nobody asked for: a C function
taking a `{ char*, long }` by value is a real signature.

### 5. Verified against a C compiler, not against itself

**This is the part that makes the rest trustworthy.** A test calling a Jairs procedure declared `#c_call`
would pass with both sides wrong, since one classification would emit the call *and* read it. Agreement with
itself is not evidence.

Two tests, split by what each engine can reach:

`valid/130` calls libc's **`ldiv`**, which returns a sixteen-byte two-integer struct. All three engines,
including the comptime VM, and the convention was fixed by a C compiler years ago. It checks the quotient and
remainder *separately* rather than their sum, so reading the two result registers in the wrong order is
visible; and it checks `-17 / 5`, where a wrong register and a wrong rounding rule would look alike and only
one of them is this compiler's business.

`jr-cli`'s `aggregates_cross_a_foreign_boundary_as_a_c_compiler_expects` compiles a **C shim** with `cc` at
`-O1` — an optimising compiler is freer to keep a struct in registers, which is the convention under test —
links it against an emitted object, and runs it. Five bits: a two-word struct passed, the same struct returned
*with its fields swapped* so a register mix-up shows, a two-`double` HFA passed, the same HFA returned
alongside a plain `double` so both register files are in use at once, and a **nested four-`double` HFA** —
thirty-two bytes, four registers, the `CGRect` shape a byte-count test rejects.

The VM is absent from the shim test for a stated reason: it resolves symbols from the compiler's own process
image, not from a link line, so it cannot reach a shim at all. Its *return* direction is verified against real
libc; its *argument* direction rests on libffi implementing the ABI from a faithful description, which is a
weaker claim and is recorded as one.

`type-errors/079` changed subject rather than being deleted. It was written when *every* aggregate was
refused; it now pins the case where the two targets genuinely disagree — a sixteen-byte `{ float64, s64 }` —
and its comment carries both the history and the six other shapes still refused.

### 6. What is still refused, and why that is permanent-ish

A struct past two words that is not a float aggregate; a union or variant, always; five or more float members;
two different float widths; a dynamic array's three words; a `#simd` vector. And the mixed case, which is the
interesting one — ADR-0160 §3 argues it at length, and the short version is that System V splits it across two
register files and AAPCS64 does not, so one case has two correct answers.

Lifting it means implementing System V's eightbyte classification, verified against a target this project has
never run. PLAN §1.5's Linux CI is owed first, and that ordering is the decision.

## Consequences

- **W10 — Graphics is unblocked.** `readdir`, `stat` and `getaddrinfo` are now expressible too, so one change
  discharged the three things §8.1.2 gated.
- **`jr-vm`, `jr-codegen-clif`, `jr-codegen-llvm` and `jr-sema`** all changed; `jr-pool` did not, because
  part 1 put the decision there.
- **1054 tests** (1055 under gate 7), **251 corpus files**. `valid/130` runs in all three engines; the shim
  test runs natively.
- **A `string` now crosses a C boundary**, as the two words it is.
- **The remaining refusals are narrower and better worded**, and `type-errors/079` documents all six.
