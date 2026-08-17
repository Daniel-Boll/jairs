---
title: Jairs by Example
description: Every feature of Jairs as a small, annotated, runnable program.
sidebar:
  order: 0
  label: Overview
---

This book is a reference by example. Each page takes one feature of Jairs and shows it as a
small, complete program you can run, with the interesting lines annotated. There is no
narrative thread — every page stands on its own — so use the sidebar to jump to whatever you
need, or read straight through to survey the language.

The programs here mirror the compiler's own test corpus. That is deliberate: those files are
simultaneously the examples, the parser's tests, and the tree-sitter grammar's tests, so the
syntax you see is exactly what the compiler accepts. Where a program ends in `exit(n)`, the
value of `n` encodes which checks passed — a technique the corpus uses so that a computation
is observable through the process exit status, and so the two engines (`jr run` and
`jr build`) can be asserted to agree.

## The examples

The sidebar groups the examples roughly in the order the language was built:

1. **Fundamentals** — declarations, procedures, arithmetic, control flow, pointers.
2. **Types** — the numeric tower, `cast`, structs, enums, flags, unions, variants, arrays
   and views.
3. **Procedures & operators** — multiple returns, named and default arguments, procedure
   values, operator overloading.
4. **Memory** — the context, allocators, temporary storage, pointer arithmetic.
5. **Compile-time** — `#run`, `#insert`, `#code`, type values, reflection, `Any`.
6. **Polymorphism** — `$T` procedures, polymorphic structs, comptime-value parameters.
7. **Metaprogramming** — `#expand` macros, `#modify`, `#bake_arguments`, `@note`s, and
   note-driven code generation.
8. **The standard library** — `String`, `Sort`, `Array`, `List`, `Map`, `Math`, `Random`.

If you prefer a guided path through the same material, read [Book I — The Jairs
Language](/language/introduction/). If you want to see it all composed into working programs,
see [Book III — Jairs in Practice](/in-practice/).
