# ADR-0200: An imported type's name, and navigation from a type position

- **Status:** Accepted
- **Date:** 2026-09-04
- **Deciders:** dboll
- **Amends:** ADR-0171 §"honest gaps" (the anonymous struct DIE), ADR-0028 §4 (hover's source list)

## Context

An inlay hint on `window := create_window(...)` read:

```
window: structDeclId(1:1)
```

`structDeclId(1:1)` is an internal identifier. It reached a place a person reads a type name, which is
the eleventh instance of that shape in this project, and it reads as a type actually *called*
`structDeclId` rather than as a missing answer.

Two further symptoms came from the same report and turned out to be a **second, unrelated** defect:
goto-definition and hover on a type annotation both answered nothing at all — not an error, not a
fallback, simply `null` at every column.

## Decision

### §1. The pool records a nominal type's declared name

`FileSignatures::type_name` is keyed by `PoolId` and populated from **this file's own items**, so an
importing file's map has no entry for `modules/Window`'s struct and the renderer fell through to
`format!("struct{decl:?}")`.

The name belongs in the pool, and the argument is one the codebase already makes. `Pool::soa_counts`
carries this comment:

> Here rather than in `jr-hir` because the question "is this an `#soa` struct" is asked by `jr-sema`
> about a type that may have been declared in **another file** (ADR-0117), and the pool is the one
> place every file's declarations already meet. Two lookups — one per file's HIR — would be two chances
> to disagree about a type's identity.

That transfers word for word to "what is this type called". So `decl_names: FxHashMap<DeclId, String>`
sits beside it, written by `FileSignatures::record_in` — which already runs "for the file being checked
*and* for every file it imports", which is exactly the coverage the name needs.

The `DeclId` is derived in `record_in` through a new `Pool::nominal_decl`, rather than threaded through
`insert_type_name`: one derivation beats five extra arguments that could each pass the wrong one.
`nominal_decl` is an **exhaustive match**, so a fifth nominal kind is a compile error rather than a
silent `None` that renders as an internal identifier — and `ProcValue`, which carries a `DeclId` and is
not a type, is listed and refused rather than swept up by a wildcard.

### §2. The last resort names the kind, not the declaration

When neither the file's signatures nor the pool knows, the answer is `<struct>`.

This is the part worth stating separately, because the old code was not merely missing information —
it was **presenting** it wrongly. A reader can tell `<struct>` means "the name is unavailable";
`structDeclId(1:1)` cannot be told apart from a real type name. It is the same choice `<unknown>`
already makes for an error type two arms above.

One helper for all four nominal kinds, so they cannot disagree about the order the two sources are
consulted in.

### §3. Hover and goto-definition read a type position from the CST

A `TypeRef` carries **no span** (ADR-0013) — where one is needed it is added to the variant that needs
it, as `TypeRef::Array` does for `len_span` — and no resolution for a type name reaches `ResolveMap`,
which covers expressions only. `resolve.rs` says so outright, calling it "the asymmetry ADR-0031 §2 had
to work around for unused imports". So `locate` cannot see a type annotation, and both features
answered `null`.

`locate::type_name_at` reads the **CST** instead: the identifier at the offset whose parent is a
`NAME_TYPE`. That is the same choice completion's `context_at` makes, and the tree knows precisely that
an identifier is in *type* position, which is what stops a value of the same name answering instead.

**Rejected: give `TypeRef::Name` a span.** 19 sites across nine crates, to record something the CST
already holds exactly.

Resolution is one function, `resolve_type_name`, shared by hover and goto-definition — so a cursor
cannot describe one declaration and jump to another. That is ADR-0028 §1's one-renderer rule applied to
the *resolution* rather than to the rendering. It looks in three places, in this order:

1. **This file's own declarations**, through `FileSignatures::lookup` plus a type-kind check. The
   nearer answer, and the one a reader means.
2. **Every module imported bare**, through `file_exports`. An aliased import contributes nothing to an
   unqualified name, exactly as it contributes nothing to completion (ADR-0199 §6).
3. For a **qualified** `W.Rect` (ADR-0179 §1), only the module the alias names. A cursor on the alias
   answers `None`, because an alias names a module and not a type — and `import_target` already answers
   for a cursor on the import line.

`file_exports` rather than the module's raw items, so a `#scope_module` name is not reachable:
describing or jumping into a declaration the importer cannot name would answer a question the program
cannot ask (ADR-0054 §3).

**Rejected: sema's own `type_name_imports`.** It looks like the answer and is not. It is populated from
four specific resolution sites, and a type used only in a **body-local annotation** reaches none of
them — measured, it holds nothing for `w: Window` inside `main`. Building on it would have worked for a
parameter and silently failed for a local, which is the shape of bug reported as "it works sometimes".

### §4. ADR-0171's anonymous struct DIE is named

That ADR's §"two honest gaps" said:

> The struct DIE is **anonymous**, because the pool records no *declared* name — it carries a `DeclId`
> and the name lives on the HIR item, which a back end cannot see. Faking one from the `DeclId` would
> print a number no reader recognises.

§1 removed the reason. `DW_AT_name` is now the declared name, and the existing DWARF test asserts it
alongside the member offsets it already checked.

**This is the argument for putting the name in the pool rather than in the LSP.** Two consumers wanted
the same fact for unrelated reasons — a hover and a debugger — and had each worked around its absence
differently. One answer means they cannot disagree about what a type is called.

## Consequences

- An imported type's name reaches every renderer: inlay hints, hover, signature help, completion detail.
- Hover and goto-definition work from a type annotation, in this file, from an import, and through a
  qualified alias.
- `lldb` shows a struct under its own name.
- An existing test that asserted the *absence* of type-annotation hover now asserts its presence. Its
  own failure message said "if this now works, update the note", which is what happened — the fourth
  instance in this project of a test naming an unimplemented thing having a one-wave shelf life.
- No new diagnostic code. **E0296 is still the first free one.**

## Rejected alternatives

- **Fixing it only in `jr-lsp`**, by consulting the imported modules' `FileSignatures` in the renderer.
  It would work, and it would leave ADR-0171's DWARF gap open with the same cause — two consumers, two
  workarounds, and nothing forcing them to agree.
- **A `Symbol` rather than a `String` in the pool.** The pool has no interner, and a `Symbol` from one
  file's interner cannot be resolved against another's.
- **Falling back to the `DeclId`'s number**, which is what the old code did. It is not a smaller loss of
  information than `<struct>` — it is a *wrong* answer instead of an absent one.
- **Answering the alias of a qualified type with its module's first type.** A cursor on `W` would jump
  somewhere plausible and wrong; `None` is the honest answer, and the import line above it already has
  its own.
