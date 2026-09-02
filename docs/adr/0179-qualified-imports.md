# ADR-0179: Qualified imports — `Simp :: #import "Simp";` and `Simp.name`

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **Group A of the Simp-shaped-graphics plan.** The graphics restructure needs three modules to coexist
  in one file, and it cannot have them: `#import` is flat, so two modules exporting one name make that
  name unusable. This is the language change that removes the wall, delivered before any of the modules
  it exists for.

## Context

### The wall, measured rather than argued

`modules/Window` exports eleven unprefixed names — `open`, `close`, `start`, `stop`, `fill`, `destroy`,
`line`, `clear`, `present`, `rect`, `delay`, `push`. `modules/File` exports `open` and `close`. A file
that imports both and calls `open` gets:

```
error[E0211]: ambiguous name `open`: provided by multiple imported modules: `Window`, `File`
```

So **a graphics program that loads a file was unwritable**. That is not a hypothetical: it is the
program Group E of the plan has to produce.

The workarounds in the tree show the same pressure from three directions. `modules/UI` renamed its own
id sentinel to `NONE` purely to dodge `Window`'s flat names, and said so in a comment.
`modules/Image` is fully prefixed — after four E0211 collisions in one wave (ADR-0166 §7 recorded
`fill`, `destroy`, `free` and `layout_is_sdl2` firing at once). And ADR-0167 §7 wrote the rule down:
*"in a flat namespace a module must prefix as though the namespace were its own"* — a convention with
nothing enforcing it, which two of the three graphics modules were violating.

### What the language actually did, before this

Four probes, all run before a line was written.

**`X :: #import "Y";` was refused**, with a message that is misleading rather than wrong:

```
error[E0208]: `#import` is only allowed at file scope
```

The directive *is* at file scope. It is in *expression* position, and
`FILE_SCOPE_ONLY_DIRECTIVES` refuses the whole class there.

**`String.length(...)` in value position parsed** and then failed to resolve — `error[E0201]:
unresolved name String`. So the parser already produced a field access; only the resolution was
missing.

**`Window.Event` in type position did not parse at all**: `error[E0100]: expected ')', found '.'`.

**`Res::Imported(ItemId, Symbol)` already existed** and is documented as *"the `ItemId` is the
`#import` item in the current file; the `Symbol` is the name in the imported scope"* — which is exactly
what a qualified member resolves to, and is already consumed by sema, MIR and the LSP.

## Decision

### §1 — An import gains an optional alias, recognised in lowering rather than in the grammar

`ItemKind::Import` gains `alias: Option<Symbol>`. `None` is a bare `#import "M";`, whose names merge
into file scope exactly as ADR-0014 §2 promises; `Some` is `Alias :: #import "M";`.

**The parser needs no change.** The aliased form arrives as a *constant declaration whose value is a
directive expression*, which already parses — so recognition happens in
`lower_const_decl_with_inherited`, by the directive's **name**, exactly the way `#bake_arguments`
(ADR-0097) and `#insert` (ADR-0072) are recognised. A grammar rule was rejected for that reason: the
shape is already legal, and a second rule for it would be a second way to parse one thing.

The E0208 refusal **stays** for every other position. An `#import` inside a procedure body is still an
error, and the check is reached because the aliased branch is gated on `insert_in_scope` — false for a
hoisted nested constant (ADR-0134).

**The alias is removed from `hir.scope`.** A bare `Simp` is not a value, and leaving it bound would
resolve it to `Res::Item` of an import, which every consumer downstream would then have to learn to
refuse. `Res::Error` — reported as `unresolved name Simp` — is the honest answer, and `Simp.thing`
never asks the question, because §4 makes the whole spelling one name.

`Item::name` stays `Some`, and that is load-bearing: it is what makes `check_duplicates` report an
alias colliding with a declaration. See §6.

### §2 — An aliased import merges nothing

`ImportIndex::build`'s merge loop skips an aliased import entirely — neither its exported names nor its
`hidden` set enter the flat index. **That single `continue` is what makes the collision go away**: the
ambiguity E0211 reports cannot arise, because only one spelling reaches an aliased module's scope.

The `hidden` set is skipped too, deliberately. A hidden name of an aliased module is not a name this
file could have meant *bare*, so recording it would make an unrelated unresolved name report "not
exported by `Simp`". The qualified path checks `hidden` itself (§4), where the reader did name the
module.

**The deduplication key becomes `(path, alias)`**, not the path alone. ADR-0014 §6 makes a repeated
`#import` idempotent, and that is still true per spelling — but a bare and an aliased import of one
module are **two different requests**, and both stand. The unused-import warning is keyed the same way
for the same reason.

### §3 — A named struct at the resolve boundary, not a widened tuple

`jr_hir::resolve` took `&[(&str, &ItemScope)]`. It now takes `&[ImportedModule<'_>]`, carrying `path`,
`alias` and `scope`. A three-tuple of two `&str`-ish things invites a swap, and the two mean opposite
things: `path` is what was imported, `alias` is the name it answers to.

**There were three call sites, not one.** `jr-db`'s `resolved`, and two in `jr-db`'s `sema` that
re-resolve over an expanded tree. All three built the same `Vec` from a path-keyed scope list, so the
construction is now one helper — `imported_modules_for_resolve` — which walks the **`#import` items**
rather than the path list, because the alias exists only on the item. A module imported both bare and
aliased therefore contributes two entries, which is §2's rule arriving by construction.

### §4 — A qualified value is a *name*, produced by lowering

**This is the decision the whole group turns on.** `Simp.foo` lowers to
`Expr::Name { name: foo, module: Some(Simp), … }` — one name carrying its module — and resolves to
`Res::Imported`.

The alternative on the table was the plan's: keep the `Expr::Field`, and have resolution record
`Res::Imported` **on the field expression**. It was rejected after counting what it costs. Sema reads a
callee through `let Expr::Name { res, .. } = self.expr_of(scope, callee) else { … }` at a dozen sites —
the foreign-value refusal, the `#c_variadic` refusal, intrinsic recognition, `type_of_callee` — and MIR
reads it at seven more. Every one would have to learn that a `Field` can carry an import resolution.
**A construct with no representation on the lowering path, filled in at some sites and not others, is
this project's first named failure mode** (AGENTS.md), and MIR is where its instances have lived.

Carried on the name instead, **nothing downstream learns anything**. Adding a field to `Expr::Name` made
its four construction sites compile errors — which is the point — while every `..` match site was
untouched. Seven MIR patterns needed `module: _` and no MIR logic changed at all.

The alias set is collected by a **pre-scan of the syntax tree** before any item is lowered, for exactly
the reason `collect_macro_bodies` needs one (ADR-0090 §2): `Simp.foo` may be written above the
`Simp :: #import "Simp";` that binds `Simp`. It is purely syntactic — a file-scope constant whose value
is an `#import` directive — so it cannot disagree with what lowering then produces.

**A local wins, silently.** In body position the rewrite is gated on `lookup_local(alias).is_none()`,
so a local or parameter named `Simp` makes `Simp.foo` an ordinary field of a value. That is ADR-0014
§3's shadowing rule, enforced by *where* the check sits rather than by a rule of its own.

**One new code, E0292**: `Alias.member` where the module exports nothing by that name. E0253 — "not
exported by `M`" — is reused when the module declares it and hides it, reached through the same
`lookup_qualified` helper the two positions share, so they cannot disagree about what a module exports.
The two stay separate codes because they send a reader to different places: one to a `#scope_module`
line, the other to a spelling.

**A second code was drafted and refused.** The plan called for E0293, "the alias names something that
is not an import — e.g. a local shadowing it". It has **no reachable condition**: a local of the
alias's name makes the access a field access (above), and an alias colliding with a file-scope
declaration is already E0200. A code with no condition is worse than no code — it reads as a promise
that something is checked.

### §5 — A qualified type is a new `TypeRef` variant, resolved in sema

Required, not optional: `Texture`, `Event` and `Bitmap` are type names in signatures, so an import that
hid its procedures and leaked its types would not solve the collision it exists for.

- **Parser**: the `IDENT` arm of `parse_type_inner` accepts `. IDENT`, and both identifiers land in the
  **same `NAME_TYPE` node**. No new node kind, so no consumer meets one. The AST reads the *last*
  `IDENT` as the type's name and the first as the module, which means every existing consumer of
  `NameType::name_token` keeps working. Guarded on `nth(1) == IDENT`, so a lone `.` stays unconsumed and
  is reported by the enclosing construct, where it means something.
- **HIR**: `TypeRef::Qualified { module, name }`. A *variant* rather than a field on `Name`, because the
  workspace's exhaustive-match ban then makes every consumer that resolves a type name a compile error
  until it decides what a qualified one means. There were **six** real sites, not the seventeen the plan
  estimated — the rest of the matches were comments.
- **Sema**: `resolve_qualified_type_name` is deliberately **not** a path through `resolve_type_name`.
  None of that function's earlier steps apply: a qualified name is never a builtin, never a bound `$T`,
  and never a file-level declaration. The alias says which scope to look in, and that is the whole
  lookup. It reaches the same `FileSignatures` the bare form does, so **`Window.Event` and a bare
  `Event` from the same module intern to one `PoolId`** — verified by a program that declares a local
  one way and passes it to a procedure that declares its parameter the other, which exits 7.
- **The type-position refusal is E0212**, sema's unknown-type code, not resolution's E0292. The two
  positions are answered by different crates — a type annotation is invisible to `ResolveMap`, which is
  the asymmetry `jr-db`'s unused-import query already documents and lives with (ADR-0031 §2). Raising
  E0292 from sema would also break `codes.rs`'s no-two-crates rule, and correctly: it would be two
  declarations of one code.

**The formatter dropped it, on the first attempt, in the unsound direction.** `f :: (e: W.Event)`
reformatted to `f :: (e: W)` — a file that no longer type-checks. That is the **thirteenth consecutive
wave** in which `jr fmt` has had to learn a construct, and the fix is the same one every time: emit
every token the node carries, not the first.

### §6 — The library proves it, and three modules stop leaking

`modules/UI` is converted to `Window :: #import "Window";` and qualified uses. It is the smallest
consumer and the one whose docs recorded the flat namespace as a problem.

`Window`'s exports are deliberately **not** renamed. With an aliased import the collision is gone,
which is the whole point of doing this before the graphics restructure — a rename would have been the
workaround this ADR exists to remove.

`Window` and `Image` gain `#scope_module`, which they never had, so their raw `#foreign` bindings stop
escaping: seventeen names from `Window` alone. Each file's bindings and its `#system_library` handle
moved below the marker, which is mechanical — file-scope declarations are order-independent — and
brings the three graphics modules in line with `Basic`, `File`, `File_Utilities` and `List`.

## Consequences

- A module may now be imported under a name, and a program may import two modules that export the same
  name and use both. `tests/corpus/imports/valid/019-qualified-imports.jr` is that program.
- Every existing file is unchanged in meaning. A bare `#import` still merges flat.
- **A bare alias is `unresolved name`**, which is a slightly indirect message for `x := Simp;`. Accepted
  rather than given a code: the construct is meaningless, and inventing a diagnostic for it would mean
  keeping the alias in scope, which is what §1 rejected.
- **Qualified imports are still not `Window.Event` everywhere.** A qualified name is a name and a
  qualified type is a type; a qualified *module* member that is itself a namespace does not exist,
  because there are no nested modules to reach.
- `using p: Window.Point` promotes nothing, and returns `None` rather than falling back to the member
  name — which would find a same-named local struct and promote the wrong fields. Recorded as a boundary
  rather than left implicit.

## Verification

- **The collision that was impossible is possible.** `tests/corpus/imports/valid/019-qualified-imports.jr`
  imports `Colors` and `Palette`, which both export `blend`, and calls both. It checks and resolves with
  zero diagnostics; the bare-import equivalent is
  `tests/corpus/imports/invalid/002-ambiguous-imported-name.jr` and reports E0211.
- **It runs.** `tests/corpus/valid/133-qualified-imports.jr` exits **31** — five independent bits: a
  qualified value, a second module's value, a qualified constant, a qualified type as a local's
  annotation read through a procedure spelled the same way, and the same pointer handed to the module's
  own procedure whose parameter is spelled bare. Both engines agree.
- **The refusal is refused.** `tests/corpus/imports/invalid/019-qualified-name-absent.jr` reports E0292
  at the use, and the file also asserts recovery: `Colors.BLACK` beside it still resolves.
- **Verified by hand where the corpus cannot reach**: `Window` + `File` imported together with
  `Window.start()` called, checking with zero errors — the E0211 quoted at the top of this ADR, gone. It
  is not a corpus file because a program importing `Window` links `-lSDL2` and this directory's
  harnesses build with no `-L`.
- All six gates green; `jr fmt --check` over every corpus directory and `modules/`; tree-sitter
  regenerated, the whole corpus parsed with no `ERROR` node, all four queries loaded, and the checked-in
  Neovim parser rebuilt.
