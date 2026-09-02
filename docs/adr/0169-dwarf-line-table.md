# ADR-0169: The DWARF line table — W12's first item, from zero

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **W12's first item**, and the first debug information this compiler has ever produced. PLAN §8.4 claimed
  "line tables exist"; ADR-0159 probed and found **none**.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

`dwarfdump --debug-line` on a built binary printed an empty section and `size -m` found no debug section at
all. `gimli` had been a workspace dependency for eleven waves and was never used. So this starts from zero.

## Decision

### 1. A Cranelift `SourceLoc` is an index into a `(path, line)` vocabulary this crate owns

Cranelift attaches an opaque `u32` to each instruction and hands back `(code offset, SourceLoc)` pairs after
compiling a function. Nothing in Cranelift knows what the `u32` means, which is the whole point: it is this
crate's number.

So it indexes a deduplicated list of `(path, line)`. One entry per distinct source line in the whole module, and
a statement on a line already seen costs no row and no allocation.

**Rejected: encoding the span's byte offset into the `SourceLoc`.** It needs no vocabulary — and it needs the
*file* recoverable some other way, since a `u32` cannot hold both a `FileId` and an offset without a bit-packing
scheme that breaks silently on a large file. An index into a table this crate owns has no such ceiling.

**`u32::MAX` cannot be an index**, because Cranelift spells "no location" as `SourceLoc::default()`, which *is*
`u32::MAX`. `intern` refuses to hand it out; a vocabulary would need four billion distinct source lines to reach
it, so the refusal is a statement rather than a limit anyone meets.

### 2. The position comes from `TrapLocations`, so a line row and a trap message cannot disagree

`TrapLocations` already resolved a `MirSpan` to a location for trap messages — as a **formatted string**, which
DWARF cannot use.

So the trait now defines `position() -> Option<SourcePosition>` with a path, line and column, and **`location()`
became a provided method that formats it**. An implementor cannot supply one without the other, and the
rendering exists once.

That is ADR-0020 §2's argument applied one level down: that ADR made *one* place decide what a trap says,
because two engines rendering at different times had drifted in punctuation. This is two *consumers* — a trap
message and a `.debug_line` row — and the same fix prevents the same class of divergence. A `.debug_line` saying
line 41 while the trap says line 40 is a bug nobody would find quickly.

### 3. A row per statement and per terminator, and no row for a synthetic instruction

That is the granularity a debugger steps at. A row per *instruction* would inflate the section without telling a
reader anything new.

**A synthetic span produces no row**, leaving Cranelift's own "unknown" in place. **Rejected: inheriting the
previous statement's line**, which is what a naive implementation does: a stepping debugger would then show the
cursor on a line whose code has already run, confidently.

### 4. No column, deliberately

A DWARF row's column is optional. This compiler's spans are per-*statement*, so a column would always be the
statement's first byte — and a consumer would render a caret under the wrong token with full confidence. Absent
is more honest than wrong.

`TrapLocations::position` still carries the column, and the trap message still prints it, because there it is
text a reader can judge rather than a coordinate a tool trusts.

### 5. DWARF 4, not 5

Both `dwarfdump` and `lldb` read 4 everywhere. DWARF 5 moves the line-program header's file table in a way older
consumers reject, and a wave whose deliverable is *"a debugger can read this"* should not also be a bet on
consumer versions.

### 6. Two wrong results before the right one, both recorded in the code

**The section name.** Mach-O spells it `__debug_line`, not `.debug_line`. The wrong name produced a section
`dwarfdump` silently ignores — indistinguishable from emitting nothing, which is exactly the failure this wave
existed to fix.

**The segment.** A Mach-O debug section outside `__DWARF` links with an alignment warning and then *fails*:
`ld: pointer not aligned in 'anon-45'`. `ld` treated it as ordinary data to be laid out among the pointers. So
`object`'s own `StandardSegment::Debug` is asked rather than the name hard-coded.

Both are in the code as comments, because each looked like "the feature does not work" and was one string.

### 7. A relocation-aware writer, not `EndianVec`

A line program's sequence starts at a function's **address**, which does not exist in an object file. gimli's
`EndianVec` *errors* on `Address::Symbol` for exactly that reason, so this crate has a `Writer` that records
`(offset, symbol, addend, size)` and applies them as `object` relocations after the sections are added.

**The symbol is an index into a `Vec<SymbolId>` this crate owns.** gimli's `Address::Symbol` carries a `usize`
and `object`'s `SymbolId` is opaque with no accessor, so the two cannot be the same number. **The first draft
instead recovered the id by parsing `SymbolId`'s `Debug` output** — it worked, and it depended on another crate's
formatting. Replaced, and recorded as the rejected alternative rather than left in.

### 8. Verified by parsing, not by grepping a tool

The test parses the section with `gimli`, the way `lldb` does. **Rejected: shelling out to `dwarfdump`** — it is
a macOS tool, its output is not a contract, and a grep would pass on a section no debugger can read.

**What it asserts beyond "the section exists"**, because that check is the one that would have passed on this
wave's two wrong results:

- Rows name lines that **are** statements — a `return`, a `while` and an `if`, spread through the file so one
  wrong constant cannot satisfy all three. A table whose every row said line 1 would parse perfectly.
- Not every row is the same line.
- The file table holds **both** files, since the program imports `modules/Basic`. One entry would mean every row
  was attributed to whichever file came first.

Verified by hand too: `dwarfdump` on the object shows rows at lines 21, 29, 31, 35, 40 and 41 of
`valid/024-hello.jr`, and each of those is a statement.

### 9. macOS keeps DWARF in the object, and that is not a defect

`dwarfdump` on the *linked binary* shows nothing, because `ld` on macOS writes a debug map referencing the object
files rather than copying DWARF in; `dsymutil` then produces a `.dSYM` bundle. PLAN's W12 row already named
"`__DWARF` versus a `dsymutil` bundle" as a decision.

**It is deferred rather than decided**, because it is a *driver* question — `jr build` deletes the object after a
successful link, so a `dsymutil` step needs the object kept and a flag saying so. `--emit-object` already
produces an object with full DWARF, which is what this wave's test uses and what a debugger can be pointed at
today.

## Consequences

- **A native binary's object carries a valid DWARF line table.** The first debug information this project has
  emitted.
- **`TrapLocations` gained `position()` and `location()` became provided**, so the trap path and the debug path
  share one resolution. Every implementor changed; there were two.
- **1064 tests** — five new: four on the vocabulary, one parsing real DWARF.
- **`gimli` is pinned to 0.33** rather than the 0.34 the workspace declared-but-never-used, so there is exactly
  one gimli in the tree: `cranelift-codegen` and `cranelift-object` already pull 0.33, and two DWARF libraries in
  one binary is a duplicate nobody wants to debug.
- **The LLVM back end has no line table**, and shares nothing here — W12's remaining work, along with type DIEs
  and locals through Cranelift's value labels. `value_labels_ranges` exists on `CompiledCode` and the back end
  emits **no `ValueLabel`s**, so locals need the emission *and* the consumption.
- **A `dsymutil` step is owed** for a linked binary on macOS, and it is a driver decision.
