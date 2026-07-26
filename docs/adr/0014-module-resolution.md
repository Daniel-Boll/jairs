# ADR-0014: Module resolution — search paths, flat imports, and cycles are legal

- **Status:** Accepted
- **Date:** 2026-07-25
- **Deciders:** dboll

## Context

`#import "Basic";` records a dependency (ADR-0012 makes it an item; spec ch. 03
describes the scoping). Nothing yet maps that string to a file, so imported names
cannot resolve — and `jr check` currently *suppresses* resolution diagnostics for
any file containing an import, because otherwise every name from the module would
be reported as unknown. That suppression is the gap this ADR closes.

Four questions have to be answered together, because the answers constrain each
other: where do we look for a module, what does importing bring into scope, what
happens on a collision, and what happens on a cycle.

## Decision

### 1. `#import` searches module paths; it is not relative to the importing file

`#import "Basic"` names a *module*, not a path. Search order:

1. each `--module-path` given on the command line, in the order given
2. the compiler's bundled module directory (the repository's `modules/`)

The importing file's own directory is deliberately **not** searched. Relative
file inclusion is a different operation and will be `#load` (a later wave).
Conflating the two makes a module's meaning depend on who imported it.

Within a search path, two layouts are tried in order:

1. `<Name>/module.jr` — a directory module, which can grow to several files
2. `<Name>.jr` — a single-file module

The directory form is tried first so that a module can be promoted from one file
to many without its importers changing.

### 2. Importing merges the module's exported names in flat

As in Jai, `#import "Basic";` makes `print` directly callable — there is no
`Basic.print` qualification and no namespace object. This is the ergonomics Jai
is built around, and departing from it would change the feel of the language for
no gain at this stage.

**Everything at file scope is currently exported.** `#scope_file`,
`#scope_module`, and `#scope_export` are lexed but not implemented (wave W2), so
there is no way to mark something private yet. This is a known and temporary
over-sharing, recorded here so it is not mistaken for a design choice.

### 3. Collisions: local declarations win; ambiguous imports are an error

- A file-level declaration **shadows** an imported name of the same name,
  silently. This is consistent with block shadowing being permitted (spec ch. 03)
  and it means adding a name to a module can never break an importer that already
  defines that name itself.
- If **two different modules** export the same name and that name is used, the
  use is an error (E0211). Silently picking the first import order would make
  behaviour depend on the order of `#import` lines, which is exactly the kind of
  action-at-a-distance we are avoiding.
- The error is at the **use site**, not the import. Importing two modules that
  happen to overlap is harmless as long as you never name the ambiguous symbol.

### 4. Import cycles are legal

Two modules may import each other. This falls directly out of file-scope
declaration order not mattering (spec ch. 03): a file scope is a *set*, so there
is no ordering constraint for a cycle to violate.

The loader must therefore be written to *tolerate* cycles rather than reject
them — memoise in-progress loads so recursion terminates. Reporting a cycle as
an error would be inventing a restriction the semantics do not need.

Note this is the opposite of the decision most languages with ordered
initialisation make, and the reason is specifically that Jairs has no
file-level initialisation order to protect.

### 5. Missing modules list the paths that were searched

E0210 must name every location tried. "Module not found" without the search paths
is the single most annoying diagnostic class in any build system.

### 6. Duplicate and self imports

- Importing the same module twice is idempotent, not an error. It is common when
  imports are generated or when a file is refactored.
- A file importing itself is a no-op rather than an error, which follows from
  cycles being legal.

## Consequences

### Positive

- `jr check` can stop suppressing resolution diagnostics, so unresolved names
  become real errors for the first time.
- The slice's `print` finally means something, which is what makes
  "stdlib in Jairs" (PLAN.md decision #5) a demonstrated claim rather than an
  intention.
- Cycle tolerance costs one memoisation table and removes a whole class of
  ordering complaints.

### Negative

- Flat imports mean a module can inject a name into an importer's scope, so
  adding an export to a module is potentially a breaking change for importers
  that use that name from a *different* module (E0211). This is Jai's tradeoff
  and we accept it knowingly.
- Everything being exported until W2 means modules currently have no
  encapsulation at all.
- Two file layouts per module is slightly more filesystem probing per import.

### Follow-on work this forces

- Wave W2 must implement `#scope_*`, at which point "everything is exported"
  becomes "everything not marked otherwise".
- The module loader must be a salsa query (ADR-0007), so that editing a module
  invalidates exactly its importers and no more.
- `#load` needs specifying as the relative-inclusion counterpart, or users will
  reach for `#import` expecting path semantics.

## Alternatives considered

**Namespaced imports (`Basic.print`).** Rejected for now: it is a different
language ergonomics, and Jai's flat model is what "Jai-inspired" commits us to.
Worth revisiting only alongside `#scope_*` in W2, since namespacing without
visibility control solves half a problem.

**Search the importing file's directory too.** Rejected: it makes a module name
resolve differently depending on the importer, and it collides with the future
`#load`.

**Reject import cycles.** Rejected as an invented restriction — see §4. Cycles
are only a problem for languages with ordered file initialisation.

**First import wins on collision.** Rejected: it makes semantics depend on the
textual order of `#import` lines, and the failure is silent.
