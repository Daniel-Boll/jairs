# Corpus

These `.jr` files are simultaneously **three** things:

1. the worked examples referenced by `docs/spec/`,
2. the Rust compiler's parser tests (`jr-syntax`), and
3. the tree-sitter grammar's tests (`tree-sitter-jairs`).

That triple duty is deliberate. `PLAN.md` §5 lists "the tree-sitter grammar
drifts from the compiler's parser" as a standing risk, and this directory plus
the `corpus-drift` CI job is the mitigation: there is exactly one corpus, and
both parsers are held to it.

## Layout

| Directory | Contract |
|---|---|
| `valid/` | Single-file programs. Must parse with **zero** errors in the compiler, produce **zero** `ERROR` nodes in tree-sitter, type-check with **zero** diagnostics, and round-trip byte-identically through `jr fmt`. |
| `invalid/` | Must parse **with** errors in the compiler and still produce a usable tree. Excluded from the tree-sitter gate, since tree-sitter is not the authority on diagnostics. |
| `type-errors/` | Single-file programs that **parse** cleanly and must be rejected by semantic analysis. Each names the code it expects; the harness asserts that exact set, so an accidentally *different* error fails. |
| `imports/valid/` | Multi-module programs that must check cleanly. Resolved against `modules/` below. |
| `imports/invalid/` | Multi-module programs that must produce a specific *semantic* diagnostic (missing module, ambiguous name, unresolved name). These parse fine — the error is in resolution. |
| `modules/` | Importable fixture modules used by `imports/`. Passed to the compiler as a module search path. These are **libraries, not test cases**: they must parse and check cleanly, but they are never expected to produce diagnostics of their own. |

`modules/` demonstrates both layouts from ADR-0014: `Shapes/module.jr` (directory
form, tried first) and `Colors.jr` (single-file form).

## Why type errors are not in `invalid/`

`invalid/` is the **parser's** recovery corpus, and three things enforce that:
`jr-syntax` asserts every file there produces a parse diagnostic, `jr-fmt` asserts
`jr fmt` refuses it, and the tree-sitter gate excludes the directory because its
files do not parse. A type error parses perfectly well, so filing one under
`invalid/` would have meant weakening all three contracts to make room for it.

`type-errors/` keeps both contracts whole. Its files are well-formed source, so
they *join* the formatter and tree-sitter gates rather than being excluded from
them, and what they assert is narrower and stronger: exactly the diagnostic codes
their `// EXPECT:` header names.

## Rules

- Every grammar change requires a corpus file. No exceptions — this is what
  keeps the two parsers honest.
- Files in `invalid/`, `type-errors/` and `imports/invalid/` open with
  `// EXPECT:` (and, where recovery is the point, `// RECOVER:`) comments stating
  the property under test. A file that merely fails is not a useful test; what
  matters is *what the compiler does next*. In `type-errors/` the `// EXPECT:`
  line must name the diagnostic code, because the harness compares against it.
- Keep each file small and about one thing. `valid/024-hello.jr` is the sole
  exception: it is the Jairs-0 slice exit criterion from `PLAN.md` §1.4 and is
  deliberately end-to-end.
- Numbering is stable. Insert new files with new numbers rather than renumbering.
- Every `.jr` file anywhere under `tests/corpus/` must be canonically formatted;
  the `jairs-fmt` CI job enforces it.

## A note on `imports/invalid/`

Before module loading existed, `jr check` **suppressed** all resolution
diagnostics for any file containing an `#import`, because every imported name
would otherwise be reported unresolved. `imports/invalid/003` exists specifically
to prove that suppression is gone: an unknown name in a file *with* imports must
now be a real error.
