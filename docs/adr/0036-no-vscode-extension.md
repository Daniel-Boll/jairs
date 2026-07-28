# ADR-0036: Jairs ships no VS Code extension; Neovim is the supported editor

- **Status:** Accepted
- **Date:** 2026-07-28
- **Deciders:** dboll
- **Amends:** `PLAN.md` §1.4's first exit criterion, which named VS Code specifically.

## Context

§1.4's editor criterion has read "VS Code: diagnostics + hover + goto-def" since the plan was
written, and §7 has carried "a VS Code extension" as open work for five waves. The server side
has been done the whole time: `jr lsp` speaks LSP 3.17 and `crates/jr-cli/tests/lsp_stdio.rs`
proves it against the real binary. What was missing was an extension to launch it.

The decider does not use VS Code and does not want one. That is the whole reason, and it is a
sufficient one — a packaging target for an editor nobody involved runs is unverifiable in
practice and rots the way §1.4's Neovim box rotted before ADR-0025.

Before descoping, the empirical facts were established against **VS Code 1.120.0** so that a
future reversal starts from evidence rather than from scratch:

- **A bare extension activates from a directory**, no packaging step, via
  `code --extensionDevelopmentPath=<dir>`. Verified by watching an `activate()` write a file.
- **`vscode-languageclient@10` is required.** VS Code has no builtin generic LSP host: 96
  bundled extensions, 52 of them contributing TextMate grammars, and not one exposing a
  server-launching mechanism an extension could reuse.
- **No bundler and no TypeScript are needed.** The client is plain CommonJS resolved through
  subpath exports (`require("vscode-languageclient/node")`), 9 packages and 3.4 MB installed.
  It resolves correctly under plain `node`; the only failure is `require("vscode")`, which the
  host injects at runtime.
- **VS Code has no tree-sitter API for extensions.** `vscode.d.ts` is 21 235 lines and mentions
  tree-sitter **zero** times, while `DocumentSemanticTokensProvider` appears 11 times. So
  highlighting there could not reuse `tree-sitter-jairs` at all.

## Decision

### 1. No VS Code extension. §1.4's editor criterion is met by Neovim

The criterion's *intent* was "an editor gives diagnostics, hover and goto-definition over the
real protocol". `editors/nvim/` does that and eleven capabilities more, verified by 67 checks
against the real editor and the real server (ADR-0025). Naming VS Code in the criterion was a
choice of example, and the example is now wrong for this project.

§1.4's box is therefore **closed**, not abandoned. The distinction matters: an unmet criterion
is debt, and a criterion that named the wrong target is a plan error — this project's second
named failure mode, and one §7 has been reproducing every wave by listing VS Code as owed.

**Rejected: keep it open but unprioritised.** That is what the last five handoffs did, and the
result is a list whose top items are things nobody intends to build — which makes the whole
list less trustworthy, including the parts that are real.

**Rejected: build it anyway, since the facts above show it is a day's work.** It is. It is also
a JS toolchain, an npm dependency tree and a **third** grammar in a repository whose ADR-0010
already accepted a second one only because a corpus-drift gate keeps it honest. Nothing would
gate a TextMate grammar, so it would be the one syntax description in the project that can
silently disagree with the language — which is exactly the failure ADR-0025 §4 exists to
prevent for the tree-sitter queries.

### 2. If it is ever wanted, semantic tokens are the highlighting answer, not TextMate

Recorded because it is the non-obvious half. The instinct is to write a TextMate grammar; the
facts say a *minimal* one for what the lexer alone can decide (comments, strings, numbers,
keywords) plus `textDocument/semanticTokens` from the server, because the server already has a
real parse and VS Code supports the API. That keeps one source of syntactic truth instead of
adding a third.

Semantic tokens remain unimplemented (§7 lists them) and would be the prerequisite, which is
the other reason not to start with the extension.

### 3. Zed, Helix and the rest are the same answer

Any editor speaking LSP can use `jr lsp` today; the repository ships configuration for one
editor and documents the command for the others. `editors/` is not a promise to package for
every host.

## Consequences

- **§1.4's first box closes**, and the slice's remaining criterion is a verified Linux x86-64
  CI run — which needs a push and is therefore the decider's call, not a technical gap.
- **§7 loses its longest-standing open item**, for the right reason. The handoff gets shorter
  because a question was answered, not because it was forgotten.
- **`jr lsp` is unaffected.** It is editor-agnostic by construction (ADR-0024 §4) and nothing
  in it knows which client is connected.
- **The verified facts above are the starting point for a reversal**, and they have a shelf
  life: they were true of VS Code 1.120.0 and `vscode-languageclient@10`.
- **README stops implying VS Code support is coming.** Its capability table said "no VS Code
  extension ships yet, so you wire it up yourself", which read as a promise.
