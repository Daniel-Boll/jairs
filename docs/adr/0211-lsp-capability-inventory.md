# ADR-0211: the language server's public inventory follows its advertised surface

- **Status:** Accepted
- **Date:** 2026-09-06
- **Deciders:** dboll
- **Amends:** stale capability inventories across crate docs and comments, `jr lsp --help`,
  `PLAN.md`, the dashboard, capability/docs-site pages, and the Neovim and Zed guides.

## Context

The optimisation audit began by asking whether Jairs' language server had the features expected of
a serious language tool. The answer in the code and the answer in the documentation disagreed.

`ServerCapabilities` advertises hover, definition, completion with resolve, references, document
highlights, document and workspace symbols, prepare-rename and rename, code actions, signature
help, inlay hints, full-document semantic tokens and whole-document formatting. The server also
publishes compiler diagnostics and implements full-document synchronization. Direct handler tests,
stdio tests and the real Neovim verifier each exercise a different part of that surface; no one
test claims transport and real-client coverage for every family.

Six reader-facing inventories instead described older servers:

- `jr-lsp`'s crate docs said semantic tokens did not exist and that a type annotation could not be
  hovered, after ADR-0159 and ADR-0200 had implemented both.
- `jr lsp --help` still promised only diagnostics, hover and goto-definition.
- `docs/capabilities.md` said there were twelve capabilities and no semantic tokens.
- the documentation site's tooling page carried another fixed total and stale editor claims.
- the Neovim guide said code actions, signature help, inlay hints and formatting were absent.
- several comments repeated a capability count whose meaning had changed as notifications,
  request methods and resolve methods were added.

This is not only presentation drift. An optimisation programme needs a trustworthy baseline:
otherwise an implemented feature is scheduled again while a correctness defect in an existing one
is mistaken for a missing feature.

## Decision

### 1. The public inventory names capability families and does not publish a total

The server's advertised `ServerCapabilities` value and its request dispatcher are the source of
truth. Public prose lists capability families rather than saying "twelve", "fourteen" or another
total.

A total is ambiguous: diagnostics are notifications rather than a provider field, completion
resolve is a second method in one user-facing feature, and prepare-rename may be counted with or
apart from rename. A list stays useful when one family gains another protocol method.

**Rejected: update every occurrence to a new number.** It would make the current documents agree
for one commit while preserving the mechanism that made them disagree.

### 2. Integration guides describe implemented behavior, not the wave that introduced them

The Neovim guide now includes code actions, signature help, inlay hints, semantic tokens and
formatting. Type-position hover and definition are described as working. The CLI help groups the
surface into diagnostics, navigation, completion, refactors, hints, semantic tokens and formatting
instead of referring to the original slice exit criterion.

Historical ADRs remain historical. A statement that was true when an ADR was accepted is not
rewritten merely because a later ADR changed the product; current inventories and crate-level
documentation are.

### 3. This inventory is a baseline, not a claim that the LSP audit is complete

The audit also found correctness and depth gaps:

- importer diagnostics can remain stale after a dependency changes;
- secondary diagnostic locations and instantiation frames lose precision over LSP;
- completion's keyword/directive vocabulary and lexical scope are stale;
- client capability negotiation, `didClose`, protocol errors and cancellation need hardening;
- type-definition, implementation/call hierarchy and richer assists remain absent.

Those are subsequent behavioural waves with their own decisions and tests. This ADR prevents the
programme from starting from a false inventory; it does not declare the existing server finished.

## Consequences

- A reader sees the same feature families in the CLI, crate docs, capability table and editor
  guides that a client receives during `initialize`.
- The capability inventory no longer needs a synchronized integer.
- ADR-0159 and ADR-0200 are reflected in current documentation without rewriting their history.
- The next LSP waves can be prioritised around trustworthiness and depth rather than rediscovering
  already implemented methods.
- No runtime behavior, test count, corpus file or module changes in this wave.
