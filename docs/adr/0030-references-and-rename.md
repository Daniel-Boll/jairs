# ADR-0030: references, rename, and symbols

- **Status:** Accepted
- **Date:** 2026-07-27
- **Deciders:** dboll

## Context

The language server has five capabilities and none of them answers "where else is this
used". Rename was assigned to wave W9 by `PLAN.md` §2.1 and is pulled forward for the same
reason completion was: it needs reference-finding, reference-finding needs the workspace
list ADR-0029 adds, and building them apart means building the traversal twice.

What the compiler provides: `resolved`'s `ResolveMap` maps every `Expr::Name` to a `Res` of
`Local`, `Param`, `Item`, `Imported(ItemId, Symbol)` or `Error`. There is **no reverse
index** — nothing maps a declaration to its uses — so every answer here is a scan.

One property of Jairs shapes all of this: **imports are a flat merge** (ADR-0014). There
are no qualified paths, so an imported `print` is spelled `print`, and a reference to it is
`Res::Imported(import_item, symbol)` where the symbol is all that identifies the target.

## Decision

### 1. A definition is identified by where it is declared, not by name

```rust
enum DefId {
    Item { file: PathBuf, item: ItemId },
    Param { file: PathBuf, proc: ProcId, param: ParamId },
    Local { file: PathBuf, body: BodyId, local: LocalId },
}
```

`Res::Imported(_, symbol)` is resolved to the declaring file's `DefId` before matching, by
the same route `goto_definition` already takes. So a reference search for `print` finds
uses in every importer *and* uses inside `Basic` itself, and does not find an unrelated
local also called `print`.

**Rejected: match by name.** One line, and wrong the first time two files declare the same
name — which the corpus already does, since every module declares `main`-like names freely.

### 2. References scan every workspace file; a local's search is one file

For an `Item`, every file in `WorkspaceFiles` is loaded and its `ResolveMap` walked. For a
`Param` or `Local`, only the declaring file is, because `jr-hir` cannot express a reference
to another file's local.

`documentHighlight` is the same search restricted to the current file, and is therefore
free. It is advertised because an editor uses it on cursor idle, where a workspace scan
would be wasteful — the restriction is the point, not a limitation.

### 3. Rename refuses rather than half-renaming

`prepareRename` refuses before the user types: a keyword, a builtin type name, or a
position with no declaration.

`rename` then computes a `WorkspaceEdit` and **refuses** — an error response, no edit —
when any of:

- the new name is already declared in a scope the rename would reach (a collision that
  would silently change meaning rather than break the build, which is the one outcome a
  refactor must never produce);
- any file that must be edited has parse errors, because an edit computed from a recovered
  parse can corrupt a file the editor then saves;
- `WorkspaceFiles::truncated` is set, because the search cannot have been exhaustive
  (ADR-0029 §4);
- the new name is not a valid identifier.

**Rejected: rename anyway and let diagnostics report the collision.** Simpler, and
arguably respects intent. Rejected because a shadowing collision produces code that
compiles and means something else.

**Rejected: apply with a warning attached.** An unread warning is no warning.

### 4. Symbols come straight from HIR, hierarchically

`documentSymbol` returns `FileHir::items` with struct fields nested under their struct and
parameters *not* nested under procedures — a parameter list is already visible in the
signature `detail`, and nesting them makes an outline unusable. `workspaceSymbol` is the
same over discovered files, capped, and proceeds on a truncated list because a partial
outline is still useful.

Both reuse ADR-0028's `render.rs` for the `detail` string, so an outline entry and a hover
card cannot disagree about a signature.

### 5. What is not in this wave

Code actions, `signatureHelp` and inlay hints are deliberately deferred to the next one.
Rename is what proves discovery is correct, and shipping the risky foundation with the
weaker test would have been the wrong order.

## Consequences

### Positive

- Rename either does the whole job or explains why it cannot.
- `documentSymbol` makes a file navigable for the cost of reshaping data already in hand.
- The scan is written once and reused four times.

### Negative

- Every workspace-wide answer parses the workspace, and there is still no latency number.
- A rename refused for a parse error elsewhere is a confusing failure to receive, and the
  message has to name the file — otherwise it reads as a bug.
- `DefId` holds a `PathBuf`, so it is not a salsa key and these searches are not cached
  queries. Deliberate for now: caching a reverse index means invalidating it, and no
  measurement yet says it is needed.

### Follow-on work this forces

- **A reverse index**, if measurement says the scan is too slow. That is `AstIdMap`'s
  neighbourhood and ADR-0013's decision to revisit.
- **Code actions**, next wave, keyed on codes that are now unambiguous thanks to
  `jr-syntax`'s renumbering.
- **Renaming a module** — its file and every `#import` naming it — which is a different and
  larger operation, since it moves a file.
