---
title: What's absent (and why)
description: An honest inventory of what Jairs does not do yet, and the reasoning behind the gaps.
sidebar:
  order: 21
---

Jairs is pre-alpha, and this book has marked features <span class="jairs-status absent">absent</span>
as it went. This closing chapter gathers the larger gaps in one place — not as a roadmap of
promises, but as an honest inventory of where the language stops today and the reasoning behind
each edge.

## The shape of the project

Jairs was built as a **vertical slice** driven end to end — lexer through native binary, plus a
language server — and then thickened across twelve development waves. The twelfth and last of
them is closed, with one item still open: a register-resident local's location is not yet
described in DWARF (see below). Everything documented as working is implemented across the whole
pipeline and asserted equal in both engines — three, when a native release build is asked for. The
absences below are either things the design has never asked for, or things the compiler actively
refuses rather than merely lacks.

For the games-facing version of this inventory — what a 2D game specifically wants and does not
yet have — see [What a game cannot do yet](/games/not-implemented/) in Book IV.

## Language features not yet present

- **Two-way unification and explicit type arguments** for `$T`. Inference is a one-layer
  structural match today.
- **Sub-slicing** (`buf[1..3]`), `==` **on views**, and an **array length that needs evaluation**
  (`[2 + 2]u8`).
- **Iterating by reference** (`for *x`), a range as a first-class value, and `for` over a
  user-defined type.
- **Pointer ordering** (`<`, `>`).
- **A recursive `variant`.**
- **Overloading** unary operators, `[]`, `()`, and compound assignment.

## Deliberately declined

Some absences are *decisions*, not gaps — the compiler actively refuses each of these rather than
simply lacking them:

- **Cross-file polymorphic *instantiation*.** A `$T` procedure or `#expand` macro in another
  module is <span class="jairs-status refused">refused</span> (E0268) rather than merely missing —
  the workaround is a concrete wrapper the module provides, which is why you see `sort_ints`
  beside the generic `sort`. Polymorphic *structs* do now cross a module boundary; only calling a
  template across one is refused.
- **Building an `enum_flags` value from a computed integer** — `cast(Perm, 3)` is
  <span class="jairs-status refused">refused</span>: most integers are valid flag sets, so a wrong
  one would look right, and members are combined with `|` instead.
- **A struct literal shorthand**, `Point.{1, 2}`, is
  <span class="jairs-status refused">refused</span> — it needs field-order decisions that an array
  literal's element count doesn't supply.
- **A `Code` value** — a first-class quoted syntax tree — is
  <span class="jairs-status refused">refused</span> until something can inspect or transform one;
  a value that can only be spliced is what a `string` already is.
- **Item-level `#if`** — a declaration that exists on one platform only — is
  <span class="jairs-status refused">refused</span>; `os()` answers the same question as an
  ordinary value instead, folded at compile time.
- **No garbage collector, no RAII, no exceptions.** These are design values, not missing
  features. Cleanup is `defer`; errors are values; memory is explicit.
- **No VS Code extension.** The language server is editor-agnostic; a packaging target for an
  unused editor would rot.
- **Bitwise precedence is not C's, and int/float never mix implicitly.** Both are choices in
  favour of refusing to guess.

## Back-end and platform status

- **Two native back ends: Cranelift and LLVM.** `--backend llvm` compiles through LLVM as well as
  the default Cranelift, and `--opt-level 0|1` chooses how hard either one optimises. There is
  still no `--release` shorthand.
- **Both macOS arm64 and x86-64 Linux are verified.** macOS locally, gate by gate, and Linux in
  CI — both fully green.
- **A native binary carries real DWARF** — line tables, struct layouts, stack-resident locals — in
  both back ends, so it is debuggable in a normal debugger. One item is still open: a
  register-resident local's location is not yet described.
- **Optimisation is real but shallow** — an inliner, store-to-load forwarding, const-propagation
  and dead-code elimination, run to a bounded fixed point, plus `#simd [N]T` and `#soa(N)` for
  explicit layout control. No SROA.

## Security, stated honestly

An internal audit's security scope is only **partly** covered, and the project says so rather
than implying otherwise. Some narrow dispatches are done (a foreign call's pointer span is
bounded by the VM's own check; `BUILD_OUTPUT` is confined to the working directory; the
compile-time FFI gate holds structurally). Others — forging an `Any` or a procedure pointer, and
language-server path handling — are unexamined, and a second pass is owed.

## The one rule to carry away

Where this book shows a feature without a caveat, it works — end to end, in both engines,
checked. Where it's marked absent or refused, it genuinely isn't there. That honesty is the point
of documenting a language this early: you can build on what's shown, and you won't be surprised by
what isn't.

That's the end of Book I. From here, [Book II — Jairs by Example](/by-example/) is the
feature-by-feature reference, and [Book III — Jairs in Practice](/in-practice/) shows the
language carrying real programs.
