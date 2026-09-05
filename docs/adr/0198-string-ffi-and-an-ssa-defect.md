# ADR-0198: The rest of Jai's `String` API, and an SSA defect it uncovered

- **Status:** Accepted
- **Date:** 2026-09-04
- **Deciders:** dboll
- **Amends:** ADR-0197 §5 (`BuildCpp` and a custom link command are not gaps)

## Context

ADR-0197 surveyed Jai's `modules/String` and closed the algorithm surface — trimming, searching,
splitting, joining, replacing, case-insensitive comparison, parsing. Re-running that inventory against
the result found four more, and they are a **different kind of gap** rather than more of the same one.

The one that matters most is the FFI boundary. Every `#foreign` call returning `*u8` returns a C string,
and a Jairs `string` is `{data, count}` with **no NUL** (ADR-0004). Nothing in this library could turn one
representation into the other, so every caller counted bytes by hand.

## Decision

### §1. Four more procedures, and the pair that closes the FFI boundary

- **`c_style_strlen`** and **`to_string`** — a C string's length, and a `string` **borrowing** it. Jai's
  names.
- **`to_c_string`** — a NUL-terminated copy, for handing to C. The inverse, and it **allocates**, because
  borrowing is impossible by construction: there is nowhere to put the NUL that is not someone else's byte.
- **`wildcard_match`** — `*` and `?`, Jai's semantics. Confirmed in real Jai code rather than assumed:
  `polgartom/astex` and `okmatija/Prizm` both call it, both on paths.
- **`string_to_float`** — `to_integer`'s missing sibling, same three-value shape.
- **`find_nocase`** and **`contains_nocase`** — closing a real asymmetry. `equal_nocase`, `compare_nocase`,
  `starts_with_nocase` and `ends_with_nocase` all existed while the two *search* routines did not, so a
  case-insensitive match meant lowering a copy of the whole haystack first: an allocation, for a comparison.

**`to_c_string` exists because the corpus program hit the trap.** The first version passed `"PATH".data`
straight to `getenv` and got null — in **both** engines, because a literal's bytes are not followed by a
NUL, so `getenv` read past `PATH` into whatever the linker placed next and looked up a name nobody set.
That is the same defect that made `glShaderSource` compile the bytes after a shader (AGENTS.md).

**Both engines agreeing is the part worth keeping.** The differential harness compares the two engines, so
a bug that makes both read past the same string is invisible to it. Only running the program found this.

`string_to_float` splits **extent here, value in `strtod`**, which is ADR-0156's split for `modules/JSON`
and right for the same reason: a wrong last bit is a wrong *answer*. The extent cannot be left to `strtod`
either, which accepts `0x1p3`, `inf` and `nan`. The scratch copy comes from `context.allocator` rather than
`talloc`, so the module keeps importing nothing.

### §2. A collapsed SSA parameter could leave a dangling operand

Writing `string_to_float`'s extent scanner produced **`malformed MIR: value never defined: v13 is used but
nothing defines it`** — an assertion inside lowering, on a procedure with nothing unusual in it.

The trigger is narrow and entirely ordinary: a local declared before an `if`, a `while` inside it whose
condition **short-circuits**, another `if` after that loop, and an assignment to the outer local. The
short-circuit is what makes the condition span several blocks, which is what gives the loop header a
parameter that is only *intermediate* — collapsed once its operands turn out to agree.

`try_remove_trivial_phi` **cascades**: collapsing one parameter can make another trivial. It repairs uses
by rewriting the MIR and the builder's memo, and it returned the replacement it had chosen **before** the
cascade ran. The cascade could remove that very value. So the operand handed back named a parameter that no
longer existed, and it was written into a `goto`.

The fix is a `replaced` map, consulted at every point an operand leaves the builder: the return of
`try_remove_trivial_phi`, a memo hit in `read_variable`, and each value `add_phi_operands` pushes onto an
edge. The map is kept **flat** — `replace_uses` rewrites existing entries as it goes — so one lookup is
always enough.

**A first attempt was wrong in an instructive way.** `add_phi_operands` holds its operands in a local `Vec`
across reads that can invalidate them, so the obvious repair was to reserve a placeholder argument on each
edge and fill it immediately, putting the operand where `replace_uses` could see it. That **broke a
different procedure**: `try_remove_trivial_phi` deliberately bails out when an edge has not supplied its
argument yet, and a placeholder makes an unfinished parameter look finished, so it collapsed against
operands that were not the real ones. The reserve-first ordering is still documented in the code, because
the *alignment* argument for it is real — a read can create a parameter for a different local in the same
block — but resolving on the way out is what makes it correct.

**This was latent, not introduced.** Nothing in the corpus had the shape. It is the fourth defect in this
project surfaced by writing a library rather than by a compiler test, after ADR-0156's `mk().count`,
ADR-0157's variadic `open`, and ADR-0197's missing `trim`.

### §3. The `getenv` assertion is native-only, and that is ADR-0158's call

A C string returned *by* libc points into the **host's** environment, while a Jairs pointer is an offset
into the VM's own linear region (ADR-0061). So `jr run` traps — `invalid access of 1 bytes at address
0x16b976e4a`, measured — and a native binary reads it fine.

A program whose two engines legitimately differ has no home in `tests/corpus/valid/`, whose whole premise
is that they agree. The round trip lives in a native-only `jr-cli` integration test, which is the same call
ADR-0158 made for `modules/Process`.

### §4. `BuildCpp` and a custom link command are compositions, and this amends ADR-0197 §5

ADR-0197 §5 listed `BuildCpp` and a custom link command among the things "refused with reasons". That is
**withdrawn**: both work today, and neither needed any new code.

- **`BuildCpp`**: the script shells out to `cc -c` and `ar rcs`, then asks for a Jairs program that calls
  into the archive through `#system_library` with `library_paths` set. Verified end to end — the linked
  binary exits 42, so the C function actually ran.
- **A custom link command**: ask for `Output_Kind.OBJECT` so the compiler stops after codegen, then run
  whatever linker invocation the script wants. Jai needs a flag for this because its compiler otherwise
  always links; this needs none, because the object *is* an output kind. Verified — the script's own `cc`
  produced a binary that exits 7.

Calling them refusals was the mistake ADR-0197 itself warns about one section earlier: describing a
composition as a missing feature. Icons and manifests stay owed (they are platform resource formats), and
`Bindings_Generator` stays refused (it is a C parser).

## Consequences

- `modules/String` now covers Jai's API for everything a build script or an FFI caller needs, and the two
  directions of the C-string boundary are named routines rather than open-coded loops.
- One latent compiler defect fixed, with two regression tests — `&&` and `||`, because `short_circuit`'s
  `short_on` swaps which successor is the short one and a repair threading only one arm would pass a test
  written with the other.
- ADR-0197 §5 is corrected: two of the five things it called refusals are compositions, and they are
  pinned by tests that run the artefacts.
- No new diagnostic code. **E0296 is still the first free one.**

## Rejected alternatives

- **Accumulating the float in Jairs.** Twenty lines and wrong in the last place for inputs a build script
  really has, in a way no test written by the same author would catch. `Math` ships no `sqrt` for this
  reason (ADR-0112).
- **`talloc` for `string_to_float`'s scratch.** Tidier, and it would make this module import
  `modules/Basic` for one parse. `context.allocator` is already this module's convention.
- **A recursive `wildcard_match`.** Four lines, and exponential on `"aaaa…"` against `"*a*a*a*a*a"` — a
  pattern a caller writes by accident. One remembered backtrack point is linear and cannot overflow a
  stack.
- **A `nocase` flag on `find` and `region_equal`.** It makes every caller of the exact comparison pass a
  `false` that means nothing to it.
- **Leaving `to_c_string` out and having callers build the buffer.** That is what produced the `getenv`
  bug in the first place, and it produced it in code written by someone who had just read the warning.
