# ADR-0170: LLVM's line table — the same spans, a completely different route

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **Completes W12's first item**, which asks for a line table in *both* back ends. ADR-0169 did Cranelift's.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

ADR-0169 built Cranelift's line table by hand: a `SourceLoc` indexing a vocabulary, `gimli` writing the
section, a relocation writer for the sequence addresses.

**None of that is reusable here**, and that is the interesting part. LLVM writes DWARF itself from `!dbg`
metadata, so this back end's job is to *attach* metadata and let LLVM emit. The two paths share exactly one
thing: the `TrapLocations::position` lookup ADR-0169 §2 introduced.

## Decision

### 1. A `DILocation` per statement, hung from a per-body subprogram

`DebugScope` carries the module's `DebugInfoBuilder` and **this function's** `DISubprogram`, and `mark_line`
mints a `DILocation` at each statement and terminator — the same two places Cranelift sets a `SourceLoc`
(ADR-0169 §3). The two engines must attribute code to the same construct, or a debugger tells a different story
about one program depending on which back end built it.

**The subprogram is per body, not per module**, because LLVM *rejects* a location whose scope is not the
enclosing function's. The verifier says `!dbg attachment points at wrong subprogram for function`, which is at
least honest.

**A span with no position clears the location** rather than leaving the previous statement's, matching
ADR-0169 §3's argument: a stepping debugger would otherwise park on code that has already run.

### 2. The column *is* set here, and that is not an inconsistency with ADR-0169 §4

That ADR left DWARF's optional column unset, because a per-statement span would always give the statement's
first byte and a consumer would draw a caret under the wrong token confidently.

LLVM **requires** a `DILocation` to carry a column and writes whatever it is given. So the choice is not
"column or no column" but "the span's column or a lie". The span's column is passed.

The distinction ADR-0169 §4 actually drew was against *inventing* precision. Passing through the column a span
genuinely has is not inventing anything — and a consumer reading a DWARF column knows how much to trust it,
which is exactly why the field is optional in the first place.

### 3. A `DIFile` per source path — this wave's first wrong result

Every subprogram initially hung off the compilation unit's file. The file table then had **one entry**, and
`modules/Basic`'s statements were attributed to `024-hello.jr`.

A line table that names the wrong file is worse than none: it sends a reader to a line in a file that has
different code on it. So a `DIFile` is created per distinct path and each body's subprogram names its own.

**A check on the root file alone would have passed the bug**, which is why the test asserts the *imported*
module has an entry too. Same reasoning as ADR-0169 §8's "not every row is the same line".

### 4. `DWARFSourceLanguage::C`, and inkwell's builder rather than the raw API

**`C` because there is no `DW_LANG_Jairs`**, and inventing a number makes every consumer fall back to a default
anyway. C is the closest honest answer for a language with C's pointers, C's integers and C's calling
convention.

**`create_debug_info_builder` rather than the raw C API** for one good reason: LLVM strips every `!dbg` from a
module whose `llvm.module.flags` lacks `"Debug Info Version"`, **silently** — a module that verifies, emits, and
carries no line table. inkwell's constructor sets the flag.

**`finalize()` before `verify()`.** An unfinalised builder leaves temporary metadata nodes and the verifier
rejects them, with a message about a malformed node rather than about a missing call.

### 5. The entry shim deliberately carries no location

It is emitted after every body, has no source of its own, and giving it one would attribute the program's exit
to whichever line happened to be last.

### 6. `is_optimized: false` on both the unit and the subprogram

ADR-0142's `-O` selects how much the **mid-end** rewrites MIR and never reaches LLVM — this back end asks for
`OptimizationLevel::None` for the reason that ADR gives. Claiming otherwise here would make a debugger warn
about variables it can in fact see.

### 7. A separate test, not a parameter of the Cranelift one

The two routes share only the span lookup, so a shared test would assert the intersection and miss what
matters: **two unrelated emitters, reading one span source, agree about which lines exist.** The LLVM test
names the same three statements as ADR-0169 §8's, and that agreement is the point.

Two differences from the Cranelift assertions, neither a concession:

- LLVM writes a file entry as a bare **name** plus a directory, where this project's own emitter writes the
  path it was given. That is DWARF's own split.
- LLVM emits rows at **line 0** for instructions with no location, exactly as `clang` does — DWARF's spelling of
  "no line". Those are filtered rather than asserted against, because demanding none would be asserting an LLVM
  implementation detail.

## Consequences

- **W12's first item is complete**: both back ends emit a line table, from one span source.
- **1064 tests**; the new one is `llvm`-gated, so gate 7 is 1065.
- **`Shared` gained a `debug` field** and `body::statement_span` became `pub(crate)` in both back-end crates —
  the second because the *unit's* file is learned from the first body that reports a position, which is a
  question about statements the back end must ask before translating one.
- **The `dsymutil` gap is unchanged and now applies to both engines**: `ld` on macOS leaves DWARF in the object,
  `jr build` deletes the object after a successful link, so a linked binary carries none while `--emit-object`
  carries all of it. Still a driver decision.
- **Type DIEs and locals remain**, and they will *not* share an implementation either: LLVM wants
  `create_basic_type`/`create_struct_type` metadata, Cranelift wants `.debug_info` DIEs written by hand. This
  wave is the evidence for expecting that shape rather than hoping for reuse.
