# 03 — Scoping and name resolution

> Implemented by `jr-hir`. This chapter describes what the compiler does today;
> the gaps at the end are real and named.

Jairs resolves names in two stages: **lowering** builds the item tree and
resolves anything it can see locally, then **resolution** fills in the rest. The
split exists because file-level declaration order does not matter but local
declaration order does, and those two rules cannot be applied in one pass.

## Declaration order at file scope

Order does not matter. Every file-level declaration is collected before any name
is resolved, so a declaration may refer to one that appears later in the file.

```jr
LIMIT :: MAX_ENTITIES;      // legal: forward reference
MAX_ENTITIES :: 4096;
```

This follows from analysis being lazy and on-demand (ADR-0007) — the compiler
never depends on having read the file top to bottom. See
[`02-declarations.md`](02-declarations.md) and `007-constants.jr`.

## Declaration order inside a body

Order *does* matter. A local is visible only after its declaration:

```jr
main :: () {
    a := 1;
    b := a + 1;   // legal: `a` is already declared
}
```

The asymmetry is deliberate. At file scope, declarations are a set; inside a
body, they are a sequence with observable initialisation order, and letting a
statement refer forward to a local that has not been initialised yet would be
meaningless at best.

## Scopes

| Scope | Introduced by | Contains |
|---|---|---|
| File | the file itself | every file-level declaration |
| Parameter | a procedure's parameter list | its parameters |
| Block | every `{ ... }` | locals and nested blocks |

Lookup proceeds innermost-first: block scopes from the inside out, then
parameters, then file scope, then imported scopes.

### Shadowing

Shadowing is **permitted and not warned about**. An inner declaration may hide an
outer one of the same name, and re-declaring a name in the same block is also
accepted:

```jr
main :: () {
    outer := 1;
    {
        inner := 2;
        {
            inner := 3;   // shadows the `inner` above; legal
        }
    }
}
```

See `023-block-scope.jr`. Whether Jairs should warn here is deferred to wave W2;
until that is decided, the compiler stays silent rather than guessing.

## What a name resolves to

Resolution assigns every name reference one of:

| Resolution | Meaning |
|---|---|
| Local | a variable declared in an enclosing block |
| Param | a parameter of the enclosing procedure |
| Item | a file-level declaration |
| Imported | a name provided by an imported module |
| Error | resolution failed; a diagnostic was emitted |

A failed resolution is recorded rather than aborting, so that later stages still
see a complete tree and one unknown name does not suppress every other
diagnostic in the file.

## Diagnostics

`jr-hir` uses codes E0200 and above. (The lexer owns E0001–E0006 and the parser
E0100–E0199.)

| Code | Meaning |
|---|---|
| E0200 | duplicate declaration of a name at file scope |
| E0201 | unresolved name |
| E0203 | a procedure has neither a body nor `#foreign` |
| E0204 | integer literal does not fit in `s64` |
| E0205 | unknown string escape |
| E0206 | invalid unicode escape |
| E0207 | a declaration, or `#run`, inside a procedure body |
| E0208 | `#import` outside file scope |
| E0209 | a directive used where it is not valid |

A duplicate declaration reports both places — the redefinition is the primary
span and the original is a secondary label:

```
error[E0200]: duplicate declaration of `dup`
 --> dup.jr:2:1
  |
1 | dup :: 1;
  | --- `dup` first declared here
2 | dup :: 2;
  | ^^^
```

### Why E0207 and E0209 exist

The lexer treats `#` followed by an identifier as a single token, and the parser
accepts a generic `#name "arg"` as an expression. That is deliberate: adding a
directive should never require a lexer or grammar change
([`01-lexical.md`](01-lexical.md)).

The cost is that the grammar accepts directives in places where they mean
nothing, so lowering has to reject them. Without E0209, `main :: () { #import
"Basic"; }` would lower quietly and `jr check` would report success on a program
that makes no sense.

E0207 is the same principle applied to declarations: a block may syntactically
contain a declaration, but nested procedures and local constants are not part of
the Jairs-0 subset. Rather than dropping them silently — which would remove code
from the program with no warning — they are reported, naming the wave that will
implement them.

## Modules

`#import "Basic";` records a dependency. It is only valid at file scope (E0208).

**Not yet implemented:** mapping a module name to a file on disk. Resolution
takes imported scopes as a parameter and performs no filesystem access, so until
module loading exists there is nothing to pass it.

The consequence is visible today: for a file containing any `#import`, `jr check`
**suppresses** resolution diagnostics entirely, because every name coming from
the imported module would otherwise be reported as unresolved. So `024-hello.jr`,
which calls `print` from `Basic`, checks clean — not because `print` resolved,
but because we decline to guess. A file with no imports gets full resolution
diagnostics.

This is a temporary and deliberately loud gap: it is the next thing to build, and
it is what makes the Jairs-0 slice's `print` actually mean something.
