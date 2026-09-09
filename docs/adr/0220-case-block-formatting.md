# ADR-0220: switch-arm block placement is configurable

- **Status:** Accepted
- **Date:** 2026-09-09
- **Deciders:** dboll
- **Amends:** ADR-0202 §2's formatter manifest surface.

## Context

Jairs permits a block as a statement in a `switch` or `if #complete` arm:

```jairs
case .TEXT;
    {
        state = .TAG;
    }
```

The formatter always put that block below the arm header. A requested style instead reads:

```jairs
case .TEXT; {
    state = .TAG;
}
case .TAG; {}
```

The existing `[fmt]` table is strict: an unknown key is an error, and both `jr fmt` and LSP
formatting consume the same `jr_manifest::Located::fmt_config()` result. The option therefore
belongs at that one seam rather than in either consumer.

The project scaffold raised a separate compatibility question. A bare file with no manifest has
used four-space indentation since the formatter shipped, while newly generated projects write
their formatter choices explicitly. The requested two-space default applies to those new
manifests, not retroactively to every unconfigured file.

## Decision

### 1. `[fmt] case_block_style` has two exact values

The formatter configuration gains `CaseBlockStyle::{NextLine, SameLine}`. The manifest exposes the
exact strings:

```toml
case_block_style = "next_line"
case_block_style = "same_line"
```

`next_line` is the formatter default, preserving every project that omits the key.

**Rejected: `case_brace_same_line = true`.** A boolean hides which construct it governs at use
sites and leaves no exhaustive place for another supported placement. The enum makes both choices
readable in the formatter and gives the manifest exact compatibility spellings.

**Rejected: make same-line placement unconditional.** This is a style preference, not a syntax or
safety correction. Existing formatted repositories must not churn without opting in.

### 2. The style applies only to an arm whose entire body is one direct block

With `same_line`, a non-empty block begins after the arm's semicolon and otherwise uses the ordinary
block formatter. A comment-free empty block becomes `{}`. A block containing a comment remains
multiline so the comment cannot be lost or folded onto the header.

An arm containing ordinary statements, or more than one statement, keeps its existing layout. The
setting does not invent braces, move nested blocks, or reinterpret a mixed arm as a block body.

**Rejected: move any first block inline even when more statements follow.** That produces a hybrid
arm whose first statement looks like the arm's body while later statements sit outside it visually.
The one-block condition makes the interface precise and idempotent.

### 3. New project manifests use two spaces; the global default remains four

`jr new` and `jr init` now emit:

```toml
indent_width = 2
case_block_style = "next_line"
```

The generated file is an explicit project policy. `jr_fmt::Config::default().indent_width` remains
four, so formatting a scratch file or an older project with no manifest does not change.

**Rejected: change the formatter's global default to two.** That would silently reindent existing
unconfigured code and exceeds the request for defaults in new configurations.

## Consequences

- CLI and LSP formatting share the option through the existing manifest adapter; neither has a
  second interpretation.
- New projects start with two-space indentation, while old or manifest-free projects retain their
  current output.
- Seven Rust tests are added: three formatter behaviours, three manifest-format guarantees and one
  CLI project test. The LSP parity test is strengthened in place.
- No language corpus file, module, MIR or code generator changes.
