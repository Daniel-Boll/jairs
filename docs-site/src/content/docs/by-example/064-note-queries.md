---
title: Reading & querying notes
description: has_note and note_value read a named declaration's notes; noted_count and noted_name query a file for every declaration tagged with a note.
sidebar:
  order: 64
---

Notes become useful once a metaprogram can *read* them at compile time. There are two levels: reading a
**named** declaration's notes, and **querying** a whole file for every declaration that carries a given
note. All four intrinsics are answered at check time, with no VM at all — the answer is already in the
HIR the checker is holding.

## Reading a named declaration: `has_note` and `note_value`

`has_note(f, "x")` asks whether declaration `f` carries note `x`; `note_value(f, "x")` reads the note's
string payload back (ADR-0099).

```jr
#import "Basic";

/// Two notes, one bare and one with a payload.
tuned :: (x: s64) -> s64 @fast @since "0.3" {
    return x;
}

/// No notes at all, so every question about it answers "no".
plain :: (x: s64) -> s64 {
    return x;
}

/// A note on a macro — the reader does not care that the callee is spliced rather than called.
doubled :: (x: s64) -> s64 #expand @inlined {
    return x + x;
}

/// A note on a polymorphic procedure, read at the *declaration* rather than per instantiation: notes belong
/// to the template, and every instantiation is a clone that keeps them.
identity :: (x: $T) -> T @generic {
    return x;
}

main :: () {
    total := 0;

    // Present, and its payload is irrelevant to presence.
    if has_note(tuned, "fast") {
        total = total + 1;
    }
    if has_note(tuned, "since") {
        total = total + 2;
    }

    // Absent — `false`, not a diagnostic.
    if has_note(tuned, "slow") {
        total = total + 100;
    }
    if has_note(plain, "fast") {
        total = total + 100;
    }

    // Notes on a macro and on a template are read the same way.
    if has_note(doubled, "inlined") {
        total = total + 4;
    }
    if has_note(identity, "generic") {
        total = total + 8;
    }

    // A payload read back. Its **length** is compared rather than its text, because `==` on two strings
    // is refused: comparing contents needs a byte loop, which is the String module's job. `"0.3"` is three bytes.
    version := note_value(tuned, "since");
    if version.count == 3 {
        total = total + 16;
    }

    // `""` for a **bare** note and for an **absent** one — deliberately conflated, since a caller wanting
    // a payload wants the payload or nothing. Both are zero bytes.
    bare := note_value(tuned, "fast");
    if bare.count == 0 {
        total = total + 32;
    }
    absent := note_value(plain, "since");
    if absent.count == 0 {
        total = total + 64;
    }

    exit(total);
}
```

### The important design choices

- **The first argument is the declaration, not its name as text.** `has_note(add, "inline")` misspelt
  as `has_note(addd, …)` is an ordinary unresolved-name error; named by text it would have been a
  silent `false`.
- **A missing note is not an error.** Asking whether a note is present is the whole point, so `has_note`
  answers `false` for an absent note rather than refusing.
- **`note_value` conflates absent and bare.** Both a note that does not exist and a note with no payload
  give `""` — a caller wanting a payload wants the payload or nothing.
- **A folded call is a constant, not storage**, so `note_value(...).count` is read via a local first
  (`version := ...; version.count`). This is an ordinary consequence of folding, not a gap.
- The example compares `.count` rather than string contents because `==` on two strings is refused — the
  fix for which is the `String` module's `equal`.

This program's exit code **depends on its notes**: it exits **127** (1+2+4+8+16+32+64, every question
answered as intended), and deleting `@fast` would make it exit 126 instead. A shared wrong answer would
be invisible to a two-engine differential, so the exit code is made to depend on the notes.

## Querying a file: `noted_count` and `noted_name`

The reader above needs you to *name* each declaration. To ask "every declaration tagged `@X`", use
`noted_count("x")` and `noted_name("x", i)` (ADR-0100). They see **this file only**.

```jr
#import "Basic";

/// The first `@serialise`, so index 0 names this one. A single-letter name, because the exit code compares
/// name *lengths* — comparing text needs a byte loop, which is String's job.
a :: (x: s64) -> s64 @serialise {
    return x;
}

/// Not noted at all, so it is skipped rather than counted — sitting *between* the two that are counted, so
/// declaration order is genuinely tested rather than accidentally satisfied.
b :: (x: s64) -> s64 {
    return x;
}

/// The second `@serialise`. A macro, since a note belongs to the declaration rather than to how it is called.
c :: (x: s64) -> s64 #expand @serialise {
    return x + x;
}

/// A different note, counted on its own — the query is per-note.
d :: (x: s64) -> s64 @internal {
    return x;
}

main :: () {
    // Two `@serialise`s: `a` and `c`, with `b` skipped between them.
    n := noted_count("serialise");

    // Declaration order, so these are `a` then `c` — one byte each.
    first := noted_name("serialise", 0);
    second := noted_name("serialise", 1);

    // Past the end. `""` rather than a refusal, so an unrolled tail is quiet.
    past := noted_name("serialise", 2);

    // A different note, counted separately: one `@internal`.
    internal := noted_count("internal");

    // A note nothing carries. Zero is a real answer, not a sentinel.
    missing := noted_count("nosuchnote");

    exit(n * 100 + first.count * 10 + second.count + past.count + internal - 1 + missing);
}
```

### What the query guarantees, and its honest limit

- **Declaration order.** `noted_name(..., 0)` and `(..., 1)` return `a` then `c`, in source order — the
  one order a reader can predict from the file. Sorting by name would renumber every index when a
  declaration is inserted; a hash order would differ between runs.
- **An out-of-range index answers `""`**, not a refusal, because unrolling to a fixed bound is the
  intended use and its tail must be quiet: a script written for "up to four serialisable types" must
  compile in a file with two.
- **Per-note, not per-file.** `@internal` is counted on its own, and a note nothing carries counts zero.
- **This file only.** A note in an imported module is not counted — the same cross-file boundary a macro
  splice and a cross-file instantiation have. A build script is itself a file, so the boundary is where
  a build script would want it.

The exit code depends on every answer: it is 211 (2 declarations × 100, then a 1-byte first name × 10, a
1-byte second name, and 0 for the out-of-range read; `+ internal - 1 + missing` is `+ 1 - 1 + 0`).

### The boundary of folding

A folding intrinsic is answered at *check* time, so every argument must be readable then — and a `for`
variable is not, because it exists only at run time. So this does **not** work, and no spelling makes it:

```jr
// Does NOT compile: `i` is a run-time value, and a folding intrinsic needs its argument at check time.
for i: 0..noted_count("serialise") { name := noted_name("serialise", i); … }
```

After these intrinsics, notes can be **counted** and **named**, but not **looped over** by ordinary
program code. Genuine loop-driven iteration needs the query to lower to real code reading a
compiler-emitted static table, which is a separate, later mechanism. The next page shows the way around
this that *is* available today: a loop *inside* the fold.
