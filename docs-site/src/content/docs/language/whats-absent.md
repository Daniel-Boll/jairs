---
title: What's absent (and why)
description: An honest inventory of what Jairs does not do yet, and the reasoning behind the gaps.
sidebar:
  order: 20
---

Jairs is pre-alpha, and this book has marked features <span class="jairs-status absent">absent</span>
as it went. This closing chapter gathers the larger gaps in one place — not as a roadmap of
promises, but as an honest inventory of where the language stops today and the reasoning behind
each edge.

## The shape of the project

Jairs is built as a **vertical slice** that was driven end to end — lexer through native binary,
plus a language server — for a tiny subset, and is being *thickened* one feature "wave" at a
time. Everything documented as working is implemented across the whole pipeline and asserted
equal in both engines. The absences below are things later waves add, or things deliberately
declined.

## Language features not yet present

- **`#must`.** The compile-error-on-ignored-status half of the error model. The multiple-return
  half exists; `#must` is owed its own decision.
- **Cross-file polymorphic *instantiation*.** A `$T` procedure or `#expand` macro in another
  module can't be instantiated from your file (the workaround is a concrete wrapper the module
  provides — which is why you see `sort_ints` beside the generic `sort`). Polymorphic *structs*
  do now cross a module boundary.
- **Two-way unification and explicit type arguments** for `$T`. Inference is a one-layer
  structural match today.
- **Array literals** (`[1, 2, 3]`), **sub-slicing** (`buf[1..3]`), `==` **on views**, and an
  **array length that needs evaluation** (`[2 + 2]u8`).
- **Iterating by reference** (`for *x`), a range as a first-class value, and `for` over a
  user-defined type.
- **Pointer difference** (`p - q`), `p[n]` index sugar, and pointer ordering.
- **A recursive `variant`** or `List($T)`.
- **Float printing** — `print_int` has no floating-point counterpart.
- **Overloading** unary operators, `[]`, `()`, and compound assignment.
- **Run-time reflection** — a loop reading declarations as values — and `Type_Info`'s
  variable-length field list. Compile-time reflection and note-driven generation exist; the
  run-time table they'd need does not.

## Deliberately declined

Some absences are *decisions*, not gaps:

- **No garbage collector, no RAII, no exceptions.** These are design values, not missing
  features. Cleanup is `defer`; errors are values; memory is explicit.
- **No `Code` value** — a first-class quoted syntax tree. Declined until something can inspect or
  transform one; a value that can only be spliced is what a `string` already is.
- **No VS Code extension.** The language server is editor-agnostic; a packaging target for an
  unused editor would rot.
- **Bitwise precedence is not C's, and int/float never mix implicitly.** Both are choices in
  favour of refusing to guess.

## Back-end and platform status

- **The native back end is Cranelift.** An LLVM back end (for optimised release builds) is a
  later wave; there is no `--release` and essentially one optimisation path, plus the single
  `--no-bounds-check` build setting.
- **macOS arm64 is the only verified target.** An x86-64 Linux target is configured in CI, but
  **no CI run has ever happened** on the repository — so Linux is unverified, and the quality
  gates are green *locally*.
- **No debug info.** A native binary has no DWARF yet, so it is not debuggable in a normal
  debugger. Traps still print a located backtrace, which is the runtime story.
- **Optimisation is real but shallow** — an inliner, store-to-load forwarding, const-propagation
  and dead-code elimination, run to a bounded fixed point. No SROA, no SIMD, no `#soa`.

## Security, stated honestly

An internal audit's security scope is only **partly** covered, and the project says so rather
than implying otherwise. Some narrow dispatches are done (a foreign call's pointer span is
bounded by the VM's own check; `BUILD_OUTPUT` is confined to the working directory; the
compile-time FFI gate holds structurally). Others — forging an `Any` or a procedure pointer, and
language-server path handling — are unexamined, and a second pass is owed.

## The one rule to carry away

Where this book shows a feature without a caveat, it works — end to end, in both engines,
checked. Where it's marked absent, it genuinely isn't there. That honesty is the point of
documenting a language this early: you can build on what's shown, and you won't be surprised by
what isn't.

That's the end of Book I. From here, [Book II — Jairs by Example](/by-example/) is the
feature-by-feature reference, and [Book III — Jairs in Practice](/in-practice/) shows the
language carrying real programs.
