---
title: Jairs in Practice
description: Complete Jairs programs that do real work, walked through end to end.
sidebar:
  order: 0
  label: Overview
---

The first two books teach the language a piece at a time. This one puts the pieces together.
Each page here is a **complete program** that does something recognisable, walked through
from top to bottom: what it computes, which language features it leans on, and why it is
written the way it is.

These programs are built to run in today's Jairs — every one uses only features that exist
now, and draws on the in-language standard library (`String`, `Sort`, `List`, `Map`, `Math`,
`Random`) the way a real program would. Where a program has to work around a current
limitation, the walkthrough says so rather than hiding it, because part of what these
examples demonstrate is *how far the language already reaches*.

## The programs

Every program on these pages has been compiled and run with the real `jr` driver; the output
shown is the output it produces.

- **[Counting words and lines](/in-practice/word-count/)** — a miniature `wc` that scans a
  string once, counting bytes, words and lines with the `String` module.
- **[A bracket checker with a stack](/in-practice/balanced-brackets/)** — use a
  fixed-capacity `Array` as a stack to check nested brackets, and see how two-value returns
  replace exceptions for the empty-pop case.
- **[Memoising with a hash map](/in-practice/memoized-fib/)** — cache recursive Fibonacci in
  a `Map`, and manage the heap it owns in a language with no destructors.
- **[A deterministic dice simulation](/in-practice/dice-simulation/)** — roll dice with the
  seeded `Random` generator and tally the totals, relying on both engines producing the
  identical sequence.
- **[3D vector math](/in-practice/vector-math/)** — compute a triangle's normal with `Math`'s
  `Vector3`, with operator overloading crossing the module boundary.
- **[A generated task runner](/in-practice/note-serialiser/)** — tag procedures with a
  `@note` and have the compiler generate a call to each one, the metaprogram loop that Jairs
  calls "the Jai superpower".

Each walkthrough links back to the relevant chapters of Book I and the isolated examples in
Book II, so you can drop down to the details whenever you want them.
