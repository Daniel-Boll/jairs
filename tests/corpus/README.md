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
| `valid/` | Must parse with **zero** errors in the compiler, and produce **zero** `ERROR` nodes in tree-sitter. Must round-trip byte-identically through `jr fmt`. |
| `invalid/` | Must parse **with** errors in the compiler and still produce a usable tree. Excluded from the tree-sitter gate, since tree-sitter is not the authority on diagnostics. |

## Rules

- Every grammar change requires a corpus file. No exceptions — this is what
  keeps the two parsers honest.
- Files in `invalid/` open with `// EXPECT:` and `// RECOVER:` comments stating
  the property under test. An invalid file that merely fails to parse is not a
  useful test; what matters is *what the parser does next*.
- Keep each file small and about one thing. `valid/024-hello.jr` is the sole
  exception: it is the Jairs-0 slice exit criterion from `PLAN.md` §1.4 and is
  deliberately end-to-end.
- Numbering is stable. Insert new files with new numbers rather than renumbering.
