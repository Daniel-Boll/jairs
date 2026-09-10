# ADR-0230: Cross-file type specialisation is root-scoped and owner-local

- **Status:** Accepted
- **Date:** 2026-09-10
- **Decider:** dboll

## Context

ADR-0229 lets a local procedure infer `$K` and `$V` from an imported nominal instance such as
`*Table(string, string)`. The remaining generic-container blocker is a procedure declared in
another module:

```text
#import "Hash_Table";

table: Table(string, string);
set(*table, "name", "Jairs");
```

`set` is visible through the imported signatures, but the compiler still refuses the call with
E0268. Same-file specialisation appends a clone to the caller's `FileHir`; doing that to an imported
template would copy a body whose item, type and import indices belong to another file. Merely
removing the refusal would therefore recreate the old `no routine for file N proc M` failure or,
worse, resolve the body against the wrong declarations.

This cannot be solved by one file independently. A root may demand a clone in module A, whose body
then demands a clone in module B. Run, build, optimisation, diagnostics and the VM must all see the
same expanded HIR/signature/MIR tuple for every reachable file.

## Decision

### 1. A template demand carries full declaration identity

`jr-sema` records a type-specialisation demand as the template's `(FileId, ProcId)` plus its ordered
bound types. A caller-local `ProcId` is insufficient because two modules routinely have procedure
zero, and because the clone belongs to the declaration file.

Pure `$T` templates may cross a module boundary. An imported `$N` template, including a mixed
`$T`+`$N` template, remains E0268: its values are available only through the current file-local
const-evaluation pass, and adding that dependency is a separate decision.

`#expand` remains a macro before it is a template. An imported polymorphic macro is still E0272:
owner-file specialisation must not silently turn a cross-file splice into an ordinary procedure.

### 2. The clone is appended only to its owner file

The existing `jr-hir::expand_instantiations` remains owner-local. The program planner groups
specialisation keys by the template's file and invokes the appender on that file's prepared HIR.
Private names, imports, source spans and arena indices therefore retain their original meaning.

Calls redirect to the resulting full `ProcRef`; no clone is copied into an importer.

### 3. One root-scoped planner owns the global fixed point

`jr-db` gains one deep module whose interface answers:

> For this root program and catalog, what expanded HIR, resolve map, signatures, check result and
> redirects belong to each reachable file?

Its implementation:

1. prepares every reachable file, including computed `#insert` expansion;
2. harvests type-specialisation demands from every check;
3. deduplicates by `(owner ProcRef, bound types)`;
4. rebuilds each affected owner from its prepared HIR;
5. rechecks all rebuilt owners and harvests demands from clone bodies; and
6. repeats to the existing bounded fixed point.

Ordering is deterministic: reachable-file order, then call-site order, then full template identity.
The existing E0280 reports failure to settle.

`file_mir_for_root(root, file)` and `optimized_file_mir_for_root(root, file)` consume that same
plan. Existing per-file queries remain compatibility adapters with `root == file`. Run, build and
root diagnostics use the root-aware interface, and optimisation must fetch imported MIR through
the same root or clone ids could name absent bodies.

Root-aware diagnostics are part of the interface, not an afterthought. A concrete clone may reveal
an error the unbound template correctly withheld; E0280 and `#modify` rejection also exist only
after planning. `jr check`, `jr run` and `jr build` therefore gather diagnostics from the same
root-scoped MIR tuple they will execute or compile.

### 4. Expanded facts travel as one tuple

An expanded HIR is never paired with base signatures, a base check or MIR built for another root.
The planner returns those facts together per file; callers do not reconstruct the tuple.

This is the module's depth: callers learn one root-program interface while owner grouping,
fixed-point expansion, deterministic clone assignment and redirect construction stay local to its
implementation.

### 5. Instantiation backtraces are materialised, not walked across files later

An instantiation site stores its complete frame list, innermost first. When the planner creates a
clone it prepends the new demanding-call frame to the caller clone's existing frames.

This replaces the old same-file `ExprScope` walk. A frame's `Span` already carries its file id, so a
diagnostic in an owner-file clone can point back to an importing call without a reverse cross-file
HIR lookup during checking.

### 6. A mixed local type/value specialisation remains one clone

A call recorded in both the type-instantiation map and the comptime-value map is keyed once as
`(template, bound types, baked values)`. The clone receives both `proc_bindings` and
`comptime_values`.

Keeping the two old key lists independent creates a type-only clone and a value-only clone for one
call; the later redirect wins and loses the type bindings. ADR-0137 already chose one combined
`$$T` instantiation, and a root-scoped planner must preserve that local capability rather than
regress it whenever another call activates the cross-file path.

## Rejected alternatives

- **Clone into the caller.** The body indexes the owner's arenas and imports; caller ownership makes
  those indices lie.
- **Make each module specialise itself independently.** A clone in A may demand one in B, and no
  per-file query can discover the reverse demand without a cycle or an incomplete program.
- **Key by template name or `ProcId`.** Names may collide and `ProcId` is file-local.
- **Let run/build assemble different expanded files on demand.** That allows optimisation, VM and
  native codegen to observe different clone sets.
- **Include imported `$N` now.** It adds cross-file const-evaluation and cycle questions unrelated
  to the type-only ownership seam.

## Consequences

- Imported generic procedures become ordinary runnable calls in the VM, Cranelift and LLVM.
- Generic container modules can expose native `$K`/`$V` operations instead of concrete wrappers.
- The planner is root-dependent by design: a library module may have different private clones in
  two programs while its source declarations remain identical.
- Same-file `$T`, local `$N`, `$$T`, `#modify`, computed inserts and nested template calls continue
  through the same owner-local appender.
- Imported polymorphic `#expand` and imported `$N` keep their existing explicit refusals.
