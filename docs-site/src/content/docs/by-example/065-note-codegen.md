---
title: Note-driven code generation
description: noted_insert emits a code template once per noted declaration, and a note's payload can itself be spliced as code.
sidebar:
  order: 65
---

`noted_insert("x", template)` generates code for **every** declaration tagged `@x` (ADR-0101). It is the
point at which a metaprogram can do the whole build-script job: find declarations by note, and emit code
for each one. The template contains `#` as a placeholder, and each occurrence is replaced by a matching
declaration's name.

```jr
#import "Basic";

/// The first `@counted`, so the generated code calls this one first — declaration order.
alpha :: () -> s64 @counted {
    return 1;
}

/// No note, so the generated code skips it. It sits *between* the two that are counted, so order is genuinely
/// tested rather than accidentally satisfied.
skipped :: () -> s64 {
    return 100;
}

/// A note whose **payload is code**, spliced directly by `#insert note_value(…)`. This is the capability that
/// already worked and was undocumented.
configured :: () -> s64 @gen "n = n + 8;" {
    return 0;
}

main :: () {
    n := 0;

    // One line generating a call to every `@counted` procedure. The template has **two** `#`s, so each name
    // is substituted at both occurrences: `n = n + alpha() * alpha();` and the same for `beta`.
    #insert noted_insert("counted", "n = n + #() * #();");

    // A note nothing carries: folds to `""`, which splices nothing. A build script's generated section is
    // empty in a file with nothing to generate for, rather than a diagnostic.
    #insert noted_insert("nosuchnote", "n = n + 1000;");

    // The payload of a note, spliced as code.
    #insert note_value(configured, "gen");

    exit(n + 65);
}

/// The second `@counted`, declared **after** the splice that calls it — the query walks the file's items, not
/// what happens to be in scope at the splice point, so generation is not order-dependent in that way.
beta :: () -> s64 @counted {
    return 2;
}
```

## A loop inside the fold

The previous page's honest limit was that a `for` variable, a run-time value, can never be a folding
intrinsic's argument — so a program-level loop cannot iterate over noted declarations. But that forbids
a loop **in the program** and says nothing about a loop **inside the fold**, which is exactly what
`noted_insert` is. Generated code has to exist *before* checking, so a run-time loop could not declare a
procedure or emit a statement under any circumstances — generation is inherently a compile-time fold, and
a loop inside the fold is the *right* shape for it, not a workaround.

## What the example generates

- `#insert noted_insert("counted", "n = n + #() * #();")` walks the file for every `@counted`
  declaration — `alpha` and `beta` — and emits the template once for each, substituting the name at
  **both** `#` occurrences. It generates `n = n + alpha() * alpha();` and `n = n + beta() * beta();`.
  Note that `beta` is declared *after* the splice: the query walks the file's items, not what happens to
  be in scope so far.
- `#insert noted_insert("nosuchnote", …)` folds to `""` — a note nothing carries — and `#insert` accepts
  the empty string as "splice nothing", so a generated section is simply empty rather than a diagnostic
  about a correct program.
- `#insert note_value(configured, "gen")` splices a note's **payload as code**: `@gen "n = n + 8;"`
  becomes the statement `n = n + 8;`. This worked before `noted_insert` existed, by composition — the
  reader folds to a string and `#insert` of a computed string was already supported — and was simply
  undocumented until it got a corpus file.

## Why `#` is the placeholder

`#` is a single character that is **not** valid in a Jairs identifier and is **not** already an operator,
so a template containing one is unambiguous. `$` is taken by polymorphism, `{}` reads as a block, and a
word-shaped placeholder could collide with a real name in the generated text.

The exit code is **78** and depends on every generated line: `1×1 + 2×2 = 5` from the two `@counted`
calls under the two-`#` template, plus 8 from the payload splice, plus the base 65. Emptying the
generated text would move it to 73, which is what makes the generation load-bearing rather than
decorative.
