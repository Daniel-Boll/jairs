# ADR-0035: an `#import` line navigates, and the whole line is the target

- **Status:** Accepted
- **Date:** 2026-07-28
- **Deciders:** dboll

## Context

`#import "Basic";` is the one declaration in Jairs that names another *file*, and it is the
only one goto-definition could not follow. Requested directly, and confirmed by probing the
real server before writing anything: goto-definition and hover both answer **nothing** at
every column of an import line — column 0, on `#import`, on the quotes, and on `Basic`
itself.

The cause is a single field. `jr-hir`'s `lower_import_decl` allocates the item with
`name: None`, because an import declares no name in the file's scope — which is true, and is
what makes ADR-0014's flat merge work. But `locate_declaration` (ADR-0028 §4) skips items
whose `name` is `None`, deliberately, so that hovering a top-level `#run` does not render
whatever item happens to sit at that index. An import is caught by a guard aimed at `#run`.

There is a second, quieter symptom that proves how long this has been broken: `render.rs`'s
`signature()` has an arm

```rust
ItemKind::Import { path, .. } => Some(format!("#import \"{path}\"")),
```

carrying the comment *"Rendered because hovering the path of an `#import` is a reasonable
thing to do."* That arm is **unreachable**. Three lines above it, `signature()` does
`item.name?` and returns early on exactly the items it claims to render. The code, the
comment and the behaviour have disagreed since the hover wave, and nothing noticed because
no test asked.

## Decision

### 1. Goto-definition on an import opens the module file, from anywhere on the line

Not just on the path string. The whole `#import "Basic";` declaration is the target, because
that is what a user points at — a request phrased as "I should be able to goto from anywhere
in the line" is the correct instinct: the line has exactly one meaning and no sub-parts worth
distinguishing.

The destination is the **start of the module file**, not a declaration inside it. There is no
"the definition of a module" to land on; a module is a file (ADR-0014 §1). Line 1, column 0.

**Rejected: only the path string is clickable.** It is what the `path_span` field already
records and what an LSP client's `Location.range` would highlight most precisely. Rejected
because it makes the feature feel broken for the nine columns of `#import ` that precede it,
and because there is nothing else on the line that could mean anything different.

**Rejected: land on the module's `//!` documentation or its first declaration.** Cuter, and
wrong: the first declaration is an arbitrary choice the user did not ask for, and a file with
no `//!` block would then behave differently from one with it.

### 2. Hover on an import shows the module, its resolved path, and its `//!` documentation

The card that `render.rs` has been unable to produce. Three parts, and each is there because
it answers a question the line does not:

- `#import "Basic"` — the declaration, for consistency with every other card.
- the **resolved absolute path**, because `#import "Basic"` does not say *which* `Basic`, and
  ADR-0014's search-path order means the answer depends on configuration. This is the part
  worth hovering for.
- the module's `//!` documentation, which `file_docs` already collects (ADR-0027 §2) and which
  nothing has ever displayed.

A module that does not resolve hovers as the declaration plus a note that it was not found,
rather than as nothing. E0210 already reports the error; the hover's job is to not look
broken next to it.

### 3. An unresolved import is refused, not guessed

Goto-definition on an `#import` whose module `module_file` cannot find returns `None`. The
alternative — pointing at where the file *would* be — invents a location for a file that does
not exist, and a client would open an empty buffer at a path the user never chose.

### 4. `locate_declaration` gains an import arm rather than losing its `name.is_some()` guard

The guard stays exactly as it is, because it is load-bearing for `#run`. Imports are matched
by a separate, explicit arm keyed on `ItemKind::Import`, so the reason each item kind is or is
not matched is written at the site rather than implied by a field being `None`.

**Rejected: give an import a synthetic `name` of the module string.** It would make every
existing lookup work for free — and it would put a name into the file's item scope that the
language says is not there, so `completion` would offer `Basic` as a value and `references`
would treat it as a declaration. A well-typed placeholder standing in for a real
representation is this project's first named failure mode, and this is precisely its shape.

## Consequences

- `render.rs`'s dead `Import` arm becomes live. Its comment stops being aspirational.
- `DeclSite` grows a variant, which is a compile error at every exhaustive match on it — the
  intended consequence of `AGENTS.md`'s exhaustive-match rule, and how this change stays
  honest in `hover`, `goto_definition` and anything later.
- **`references` on an import is deliberately not added.** An import is not a definition
  anything refers to; `Res::Imported` names the *import item*, but those are references to the
  imported declaration, not to the import line. Adding it would answer a different question
  than the one asked.
- Hover on an import is the first card in the project whose body is not derived from a type or
  a signature, which is why the resolved path earns its place rather than being decoration.
