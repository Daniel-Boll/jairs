# ADR-0054: `#scope_module` hides what follows it from importers; export is the default

- **Status:** Accepted
- **Date:** 2026-07-30
- **Deciders:** dboll
- **Completes W2.** The last of the wave's seven features.
- **Fulfils ADR-0014 §2**, which said `#scope_file`, `#scope_module` and `#scope_export` are "lexed
  but not implemented (wave W2), so everything at file scope is currently exported", and listed
  implementing them as follow-on work. §1 keeps two of the three and argues the third down.

## Context

`PLAN.md` §2.1 lists `#scope_*` in W2 and §7 called it "a filter on `ItemScope`". That description
was checked before this ADR was written rather than trusted, because the previous handoff's
*reasoning* turned out to be invented (ADR-0053 §5). It holds: `jr-db`'s `file_exports` is one
function whose whole body clones `hir.scope`, and its doc comment already says "W2 will add
`#scope_*` filtering".

Five facts were established by reading the code, and three shaped the decisions.

- **`file_exports` depends only on `file_hir`**, and its doc comment names that as "the key
  invariant that prevents salsa cycles when modules import each other". **This is the fact that
  decides §3**: the filter must be computable from one file's own HIR, with no resolution and no
  other file consulted — which a position-based marker is and a name-based export list would not
  necessarily be.
- **`#scope_module` lexes as an ordinary `DIRECTIVE` token and the parser rejects it** with E0101,
  "unexpected token at top level". So ADR-0014 §2's "lexed but not implemented" is accurate, and the
  work is a parser arm plus a filter rather than anything lexical.
- **A Jairs module is one file** (ADR-0014 §1: "one module = one file"). **This decides §1's
  rejection of `#scope_file`**: with one file per module, file scope and module scope are the same
  set, so the two directives could not be told apart by any program.
- **`ItemScope` is `names: FxHashMap<Symbol, ItemId>`** and nothing else, so filtering is removing
  entries. The `ItemId`s it maps to stay valid, because the *file's own* scope is untouched.
- **`resolve.rs` reports an unresolved name as E0201 with a near-name suggestion.** A filtered name
  would land there, which is what §2 is about.

## Decision

### 1. Two directives, `#scope_module` and `#scope_export`, positional, exporting by default

```jr
// Exported: no directive has appeared yet.
area :: (r: float64) -> float64 { … }

#scope_module
// Everything from here down is hidden from importers.
scale :: 1.5;
helper :: (x: float64) -> float64 { … }

#scope_export
// Visible again.
perimeter :: (r: float64) -> float64 { … }
```

A bare directive on its own line, taking no argument, that changes the visibility of every
declaration **after** it until the next such directive. Jai spells it exactly this way.

**Export is the default**, which is the compatibility half of the decision and is load-bearing:
ADR-0014 §2 promised "everything at file scope is currently exported", 126 corpus files rely on it,
and `modules/Basic` exports `print` and `print_int` with no directive at all. Defaulting to *hidden*
would be the safer language design and it would change the meaning of every existing file, so it is
rejected on those grounds rather than on merit — a future ADR may flip it with a migration.

**Rejected: `#scope_file` as a third level.** Jai has it, and it would be a level hidden even from
other files of the same module. **A Jairs module is one file** (ADR-0014 §1), so file scope and module
scope are the same set and no program could distinguish the two directives. Implementing both would
be a promise about a multi-file module system that has not been designed — and when it is, the
`#scope_file` decision belongs to *that* ADR, where "what is a module" has an answer.

**Rejected: a per-declaration marker** (`helper :: () #scope_module { }`). More local, harder to
misread at a glance, and it diverges from Jai for no gain in expressiveness. The concrete objection
is friction: a forty-line block of private helpers needs forty markers, and that is the cost that
makes people not bother — a visibility system nobody uses is worse than one with a coarse grain.

**A directive with no declaration after it is legal**, and means nothing. Refusing a trailing
`#scope_module` would need a rule about what "after" means at end of file, for no benefit.

### 2. Using a hidden name is E0253, which says it is hidden

```text
error[E0253]: `helper` is not exported by `Shapes`
  --> user.jr:4:9
   |
 4 |     x := helper(1.0);
   |          ^^^^^^
   |
   = it is declared behind `#scope_module`
   = help: remove the `#scope_module`, or move the declaration above it
```

**Not the existing E0201.** A filtered name is genuinely absent from the imported scope, so E0201
"unresolved name" would be *true* — and it would offer a near-name suggestion and send the reader
looking for a typo that is not there. The difference between "you misspelled this" and "the module's
author hid this" is the whole value of the diagnostic, and only the specific one can express it.

The cost is a second lookup: `resolve.rs` must consult the module's **unfiltered** scope to discover
that the name exists but is hidden. That is one extra query per *failed* lookup, on the error path
only, and it buys a message that names the module.

**Rejected: reporting at the `#import`.** "This module hides a name you use" would point at the wrong
line — the mistake is at the use site, and a module hiding names is not itself an error.

### 3. The filter lives in `file_exports`, and nowhere else

`jr-hir` records a `exported: bool` per item, computed during lowering by walking items in source
order and tracking the current visibility. `jr-db`'s `file_exports` then omits the unexported ones.

**Computable from one file's own HIR**, which preserves the invariant `file_exports`' doc comment
names: it depends only on `file_hir`, so `resolved(A)` calling `file_exports(B)` never calls back
into `resolved(A)`. A visibility rule that needed resolution — say, an export *list* naming
identifiers — would have to resolve those names and could reach into another file. Positional
markers cannot.

**The declaring file is unaffected.** Within its own file a hidden name resolves, type-checks, and
answers hover, goto-definition, rename and completion exactly as before, because the file's own
`hir.scope` is never filtered. That is the meaning of "module-private" rather than an implementation
shortcut.

**Rejected: also filtering `workspaceSymbol` and cross-file completion.** Arguably correct — a hidden
name should not appear in a workspace-wide symbol search. Rejected for this wave because those
handlers read HIR directly rather than through `ItemScope`, so each needs teaching separately, and
because a symbol search that silently omits declarations is its own surprise. Recorded as owed, with
the note that it is an *LSP* decision rather than a language one.

### 4. What is deliberately absent

- **No `#scope_file`** (§1), and no visibility on anything but a file-level declaration. A struct
  field, a parameter and a local are all unaffected; field privacy is a different feature.
- **No re-export.** A module cannot export a name it imported; `#import` brings names in for its own
  use. Jai is the same, and re-export needs a decision about whose name the re-exported one is.
- **No visibility on an `#import` itself.** Every `#import` is private to its file, which is what
  "no re-export" means from the other side.

## Consequences

- **`Item` gains `exported: bool`**, so every construction site must supply it — a compile error at
  each, which is the mechanism. Lowering is the only place that computes it.
- **`file_exports` stops being a clone.** Its doc comment's "everything at file scope is exported
  (W2 will add `#scope_*` filtering)" is now wrong and must be rewritten, and ADR-0014 §2's
  "everything is exported" clause is superseded by this ADR.
- **One new diagnostic code, E0253**, for a use of a name a module does not export. **E0254 is the
  first free code**; the parser needs no new code, because a `#scope_*` directive either parses or is
  the existing E0101 stray-token error.
- **`jr-fmt` must emit the directive on its own line with a blank line around it.** The formatter has
  lost or mangled a construct in **six consecutive waves** — most recently deleting every parameter
  default. A test must assert survival *and* canonicalisation.
- **A corpus program must import a module that hides something and use only its exports**, and a
  refusal file must use the hidden name. Both are needed: the first proves the filter does not break
  a legal program, the second that it filters at all. A single file cannot do both, because one must
  check cleanly.
- **`modules/` gains its first module with a private section.** That is the dogfooding test ADR-0014
  §2 implies: if `#scope_module` is right, `Basic`'s internal helpers should use it — and
  `print_digits`, which `PLAN.md` has recorded as "still recurses" for several waves, is exactly such
  a helper.
- **Both engines and `jr-mir` are unchanged.** Visibility is resolved before MIR exists, so nothing
  downstream of resolution learns what a scope directive is — the same property ADR-0053 §1 arranged
  for argument names, and worth stating as evidence rather than assuming.
