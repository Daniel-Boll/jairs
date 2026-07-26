# The Jairs Language Specification

This directory is the reference specification for the Jairs language. Chapters
are written **just ahead of the milestone that implements them** — the
specification is not a promise of a finished language, it is a precise
description of what exists now plus an honest marker of what does not.

## How to read this spec

- Every feature is shown with a **runnable example** taken from
  [`tests/corpus/valid/`](../../tests/corpus/valid/). The corpus files are the
  ground truth for syntax: they are simultaneously the spec's examples, the
  compiler's parser tests, and the tree-sitter grammar's tests
  (`tests/corpus/README.md`). If the spec and a corpus file disagree, the corpus
  file is right and the spec has a bug.
- Where a feature is **not yet implemented**, the spec says so and names the wave
  (`PLAN.md` §2.1) that will add it. It never describes syntax that does not
  exist.
- Load-bearing decisions are cross-referenced to their
  [ADR](../adr/README.md).

## The relationship between spec, corpus, and tests

```
docs/spec/*.md   ──cites──▶   tests/corpus/valid/*.jr
                                   │            │
                        parser tests      tree-sitter tests
                       (jr-syntax)      (tree-sitter-jairs)
                                   └── corpus-drift CI gate ──┘
```

One corpus, two parsers, one CI gate. This is the mitigation for the
"tree-sitter drifts from the compiler" risk (`PLAN.md` §5) and the reason a
grammar change without a corpus file is rejected.

## Scope: Jairs-0

These chapters cover **exactly** the Jairs-0 vertical-slice subset (`PLAN.md`
§1.1) — the tiny language that is driven end-to-end through every compiler
component before any feature is thickened. Everything outside that subset is a
later wave and is called out as such.

## Chapters

| Chapter | Covers | Status |
|---|---|---|
| [00 — Overview](00-overview.md) | What Jairs is, its design values, the Jairs-0 subset boundary | Jairs-0 |
| [01 — Lexical structure](01-lexical.md) | Encoding, trivia, comments, identifiers, keywords, literals, operators, directives, diagnostics | Jairs-0 |
| [02 — Declarations](02-declarations.md) | The three declaration forms, procedures, structs, pointers, uninitialisation | Jairs-0 |

Later chapters (statements and expressions in full, the type system, the module
system, comptime, polymorphs, …) are written as their waves land.
