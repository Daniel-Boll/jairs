# ADR-0126 — A foreign call's pointer span is bounded by the VM's own check

**Status:** Accepted
**Date:** 2026-08-12
**Amends:** nothing decided. It makes ADR-0004's zero-copy foreign call keep the sandbox property
`crates/jr-vm/src/memory.rs`'s module docs already claimed for it.

## Context

The audit at `354d900` ([`docs/assessment-2026-08-07.md`](../assessment-2026-08-07.md)) recorded that
its **security scope was only partly covered** — the assessor responsible failed twice — and named
four surfaces as unexamined, one of them "comptime-FFI-gate bypasses". §7 of that document asks for a
second pass split into **three narrow dispatches** rather than one broad one. This is the first
dispatch's result.

Two of the named surfaces turn out to be sound, and saying so is half the value of looking:

- **The comptime FFI gate holds, and structurally rather than by luck.** `ffi::call` has exactly one
  caller (`crates/jr-vm/src/interp.rs:1004`), reached only from `interp.rs:362`, and the
  `Mode::Comptime` refusal at `interp.rs:998` dominates it. Only three production sites construct a
  `Vm`: `jr-db/src/mir.rs:892` and `jr-db/src/consts.rs:969` are both `Mode::Comptime`, and
  `jr-db/src/run.rs:130` is `Mode::Runtime` and belongs to `jr run` alone. So the composition worth
  worrying about — *a hostile file merely **opened in an editor** runs comptime code, which reaches
  libffi, which executes arbitrary native code inside the language-server process* — **cannot
  happen**. ADR-0121 established that an opened file is a real attack surface; this says the FFI half
  of it is closed.
- **ADR-0107 §2's heap fix is complete.** `Memory::allocate` bounds the upward frame bump on
  `heap_next` (`memory.rs:145`) and `Memory::allocate_heap` bounds the downward heap on `next`
  (`memory.rs:183`), so the two cursors cannot cross and either meeting the other is `Exhausted`.

The third thing found is a live defect.

### One byte is checked; `count` bytes are read

`marshal` translates a pointer argument with `Memory::host_pointer(address, 1)` — **one byte**
(`ffi.rs:154`). `dispatch` then captured a `write`'s output by building a slice of the program's own
`count` bytes from that pointer:

```rust
// SAFETY: `marshal` produced `buf` from `Memory::host_pointer`, which
// bounds-checked the address inside the VM's non-moving region, and
// nothing has allocated or released since.
let bytes = unsafe { core::slice::from_raw_parts(buf as *const u8, count) };
```

The comment is true at one byte and false at `count`. That is this project's **most-repeated failure
shape** — a stale comment asserting something checkable that nobody had checked — and `AGENTS.md`
names it; ADR-0104 found two bugs behind exactly such a comment, and ADR-0109 found a refusal whose
stated reason had expired.

It is reachable from a **correct** declaration. `write :: (fd: s64, buf: *u8, count: s64) -> s64
#foreign libc "write";` is POSIX's own signature, and `count` is an ordinary program value:

| `count`, on a two-byte string in a 1 MiB region | Before |
|---|---|
| `4_000_000` | exit **0**, **4,000,000 bytes** on stdout — 2,951,424 read past the end of the region's `Vec<u8>` |
| `2_000_000_000` | exit **138** — the compiler killed by **`SIGBUS`** |

In the observed 4 MB run the pages past the region happened to be zero, so **no disclosure was
observed**; what lies there is an allocator accident, and stating it as a confirmed leak would be the
kind of overclaim ADR-0125 spent a wave removing. The undefined behaviour is not conditional on that:
`slice::from_raw_parts` requires the whole span to sit in one allocated object.

**And the two engines disagreed.** The same program built with `jr build` writes **114,688** bytes and
exits 0, where the VM writes 4,000,000 and exits 0. Engine agreement is the invariant this whole
project rests on, and this is the third divergence the corpus differential's premise has exposed,
after ADR-0107 §2 and ADR-0116 §2.

The defect also defeats an invariant the code **states and tests**:
`memory.rs:410`'s `a_host_pointer_is_bounds_checked_like_any_other_access` asserts "the FFI boundary
must not be a way around the bounds check". It exercises `host_pointer` directly and never the
`marshal` path, so it could not see this.

## Decision

**The VM validates every span it dereferences itself, over its full length, through the same
`Memory::read` every other access uses — and an over-long span traps.**

`capture_write` runs in `call`, **before** `marshal`, because only there does the Jairs address still
exist; after marshalling nothing survives but a raw host pointer, and nothing can bound one. It reads
through `Memory::read(address, count)`, which returns a safe `&[u8]`, so the span is bounded **by
construction** rather than by a comment — the `unsafe` block is deleted rather than corrected, taking
the crate from nine `unsafe` blocks to eight.

An over-long count is `Trap::BadAddress`, reusing the trap it already deserves rather than inventing a
diagnostic code: passing a count past the end of a buffer is a program error exactly as an
out-of-range index is (ADR-0003). Refusing **before** the call also keeps the bogus `(pointer, count)`
pair away from the real `write(2)`, so one check fixes the VM's undefined behaviour and the host call
together. Both probes now exit **4** with a source location and a call stack.

A negative count is still skipped rather than trapped, matching what the previous
`usize::try_from(count).unwrap_or(0)` did. The fix is the missing bound, not a new refusal. The file
descriptor is still ignored, so whether a `STDERR` write belongs in `captured_output` stays the
separate question it was.

### What this does **not** fix, said plainly

The bound is the **region**, not the buffer. `write(1, s.data, s.count + 100)` still reads 100 bytes of
neighbouring VM memory and still captures them, because within one linear region an address is just an
offset — which is the model `memory.rs`'s module docs describe, and the same hazard native code has.
What can no longer happen is **leaving** the region.

And `marshal` still validates one byte for every *other* pointer argument, so a foreign callee that
reads further through a pointer the VM handed it — `strlen` on an unterminated buffer at the region's
end, a `memcpy` outrunning its source — still reads outside the region. That is recorded as owed in
the module docs rather than quietly left, because the two ways to close it are both bad bargains
today, which is the next section.

## Alternatives rejected

**A per-symbol table of `(pointer, count)` shapes** — `write`, `read`, `memcpy`, `memset` — would
bound the callee too, not just the VM. Rejected because it is a second list that must stay in step
with a first, which is precisely the token-set trap ADR-0124 replaced with an exhaustive enum after
this project counted **seven** bugs from one instance of it. A table keyed on libc's surface would rot
the same way, and its failure mode is silence.

**Clamping the host pointer to the region's end**, so `marshal` validates from the address to the
region boundary. It removes the VM's undefined behaviour with no symbol list, but it does not bound the
callee either, and it silently widens what a foreign call may see instead of refusing anything —
trading a loud trap for a quiet permission.

**A real sandbox that copies in and out**, never handing a raw pointer to libc. This is the correct
answer for a language that wants a sandbox, and it costs ADR-0004's stated payoff: a Jairs `string` is
already the `(pointer, length)` shape `write(2)` wants, and `024-hello.jr` hands `write` a pointer to
the actual literal bytes with no marshalling. Paying a copy on every foreign call to fix a bug whose
reachable form is one `unsafe` block is the wrong trade, and the deep reason is that the VM is a
*guest inside the compiler* rather than a sandbox for hostile code — `Mode::Comptime` is what keeps
hostile source away from libffi, and that gate holds.

**Validating in `dispatch` where the capture was.** Impossible rather than merely worse: `dispatch`
receives marshalled words, so the Jairs address is gone by then. This is why the fix is a *move* and
not an added check, and it is the whole reason the bug existed — the validation and the dereference had
been separated by the one function that destroys the information needed to connect them.

## Consequences

- An over-long `write` is a diagnosable trap with a source location instead of a silent overread or a
  `SIGBUS`. Nothing that worked stops working: `024-hello.jr`, `valid/101`'s `print_int` digits, and
  the existing `a_foreign_result_comes_back_as_a_value` all behave exactly as before.
- One fewer `unsafe` block in `jr-vm` — eight, all still carrying `// SAFETY:`.
- `crates/jr-vm/tests/execute.rs` gains
  `a_write_whose_count_leaves_the_region_traps_rather_than_reading_past_it`, whose comment records both
  what it pins and what it deliberately does not. Teeth-checked: dropping the bound makes it fail with
  `Ok(Scalar(4000000))`.
- The test lives in `jr-vm` rather than `tests/corpus/valid/`, and that is forced rather than chosen.
  The corpus differential asserts the **two engines agree**; here the VM traps at exit 4 while the
  native binary exits 0 with a short write. A program whose engines cannot agree by construction has
  no home in `valid/`, so putting it there would break the harness's premise to test one engine.
- The audit's security scope is now **one of three dispatches** discharged. `Any` and procedure-pointer
  forgery through the untagged `union`, and `jr-lsp` path handling, remain unexamined and are still
  owed.
