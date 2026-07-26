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

Codes E0200 and above belong to semantic analysis. (The lexer owns E0001–E0006
and the parser E0100–E0199.) Most are emitted by `jr-hir`; E0210 comes from the
module loader in `jr-db`.

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
| E0210 | module not found (lists every path searched) — emitted by `jr-db` |
| E0211 | ambiguous name provided by two or more imported modules |

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

`#import "Basic";` brings a module's names into the importing file. It is only
valid at file scope (E0208). The full rules are in
[ADR-0014](../adr/0014-module-resolution.md); the summary:

### Finding a module

`#import` names a *module*, not a path. The importing file's own directory is
**not** searched — relative inclusion will be a separate `#load` in a later wave.
Search order:

1. each `--module-path` given on the command line, in order
2. the compiler's bundled `modules/` directory

Within each directory, two layouts are tried:

| Order | Layout | Why |
|---|---|---|
| 1 | `<Name>/module.jr` | a module can grow from one file to many without its importers changing |
| 2 | `<Name>.jr` | a small module needs no directory |

A module that cannot be found is **E0210**, and the diagnostic lists every path
that was probed.

### What importing does

Imported names merge in **flat**. After `#import "Basic";`, `print` is called
directly — there is no `Basic.print` qualification:

```jr
#import "Basic";

main :: () {
    print("hello\n");
}
```

**Everything at file scope is currently exported.** `#scope_file`,
`#scope_module` and `#scope_export` are lexed but unimplemented (wave W2), so
there is no way to mark a declaration private yet. Modules therefore have no
encapsulation at present.

### Collisions

- A file-level declaration **shadows** an imported name of the same name,
  silently (`imports/valid/004`). Adding an export to a module can therefore
  never break an importer that already defines that name itself.
- If two *different* modules provide the same name and it is **used**, the use is
  **E0211**, and the diagnostic names every module providing it. The error is at
  the use site, so importing two overlapping modules is fine as long as the
  ambiguous name is never mentioned (`imports/valid/007`).
- Importing the same module twice is idempotent, not an error
  (`imports/valid/006`).

### Cycles are legal

Two modules may import each other (`imports/valid/005`). This follows directly
from file-scope declaration order not mattering: a file scope is a *set*, and
Jairs has no file-level initialisation order for a cycle to violate. Rejecting
cycles would be inventing a restriction the semantics do not need.

Note this differs from most languages with ordered module initialisation, and the
reason is specifically that Jairs has no such ordering to protect.
