# ADR-0221: manifest-free formatting defaults to two spaces

- **Status:** Accepted
- **Date:** 2026-09-09
- **Deciders:** dboll
- **Amends:** ADR-0220 §3.

## Context

ADR-0220 made newly generated manifests choose `indent_width = 2` while preserving the formatter's
historical four-space default for files with no manifest. The result was two defaults for one
setting:

- a project created by `jr new` used two spaces;
- the same source outside a manifest-backed project used four.

The decider explicitly chose consistency over that compatibility split.

## Decision

The project-facing formatter fallback is `jr_manifest::default_fmt_config()`, with an
`indent_width` of two. An empty manifest, no manifest, and a newly generated manifest therefore all
use the same indentation through the CLI and LSP.

An explicit `[fmt] indent_width` still overrides the default, and `indent_style = "tab"` still
ignores the width.

`jr_fmt::Config::default()` remains four spaces. Its direct callers include the repository's
canonical corpus and formatter unit tests; changing that lower-level library default would turn
this request into a whole-tree reformat. The user-facing tools no longer use it as their
manifest-free fallback.

**Rejected: retain four spaces only for manifest-free files.** That makes moving a file into or out
of a generated project change its canonical formatting even though no formatter setting was edited.
One default is easier to explain and test.

## Consequences

- `jr fmt` and LSP formatting use two spaces when no manifest supplies another width.
- New-project output is unchanged because generated manifests already specify two.
- Existing unconfigured files will reindent on their next formatting pass.
- Direct `jr_fmt::format_default` callers remain four-space and the canonical corpus does not move.
- No tests are added; the existing default-behaviour assertions are updated.
- No corpus, syntax, MIR, module or backend change.
