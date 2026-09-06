# ADR-0213: A project-owned module catalog replaces path probing

- **Status:** Accepted
- **Date:** 2026-09-06
- **Deciders:** dboll
- **Amends:** ADR-0014 §1 (ordered search paths), ADR-0029 §2 (filesystem discovery outside
  salsa), ADR-0199 §3 (the importable-module index), ADR-0202 §2 (project configuration)

## Context

`#import "Foo"` currently means “probe every `-I` directory for `Foo/module.jr`, then
`Foo.jr`.” That made the compiler easy to bootstrap, but it makes an ordinary project depend on a
separate `modules/` directory and an extra path flag. It also leaves three consumers answering one
project question independently:

- the CLI combines `-I`, `[build].module_paths`, and the bundled library;
- `jr-db` probes the filesystem one name at a time;
- the LSP walks workspace files and round-trips each path through that probe.

The resulting interface is shallow: every consumer knows search order, path spellings, and which
filesystem reads make a module exist. It also conflates two facts that have different invalidation
rules: **what may be imported** and **which files a rename may edit**.

The decider chose the full project-catalog option rather than only adding another conventional search
root or only adding exact dependencies.

## Decision

### §1. `jr-project` owns discovery and exposes one immutable context

A new leafward crate, `jr-project`, turns project configuration and filesystem state into a
`ProjectContext`. Its public surface is deliberately smaller than the work behind it:

```rust
ProjectContext::discover(DiscoverRequest) -> Result<ProjectContext, Error>
ProjectContext::derive(ModuleAdditions) -> Result<ProjectContext, Error>

context.catalog()
context.entry()
context.root()
context.owned_roots()
```

The context owns an immutable, sorted `ModuleCatalog`: one flat import name, one source path and
text, and one origin. Callers do not perform their own walks or reconstruct precedence. Build targets
derive a context with generated or provided exact modules rather than appending directories and
thereby exposing unrelated siblings.

`jr-project` depends on `jr-manifest` and `jr-stdlib`, never on `jr-db`. Filesystem discovery and
reads happen before salsa inputs are installed. This preserves ADR-0029 §2's rule: a tracked query
does not perform I/O salsa cannot observe.

### §2. Manifest-backed projects get an implicit `src` module namespace

When a governing `jairs.toml` exists, these two forms define a local module without `-I`:

```text
src/Foo.jr
src/Foo/module.jr
```

Only direct children of `src/` are implicit. Imports stay flat: both are imported as
`#import "Foo"`, and neither creates a hierarchical namespace. If both spellings exist for the same
name, discovery fails rather than selecting one.

There is no implicit `src` rule for a scratch file with no manifest. Optional manifests remain
optional; a bare `jr check /tmp/x.jr` still sees explicit `-I` roots and the bundled library only.

### §3. Exact external dependencies are named in `jairs.toml`

The manifest gains:

```toml
[dependencies]
Geometry = { path = "../geometry" }
Noise = { path = "../vendor/noise.jr" }
```

Names are the exact flat import names. A file path names that file. A directory path names exactly
`<directory>/module.jr`; it does **not** expose the directory's siblings as more importable modules.
Relative paths are resolved against the manifest directory.

An exact dependency and a local module with the same name are an error. This is not search-path
precedence: the project has made two declarations of one identity.

### §4. Compatibility paths are adapters, not the model

Sources are considered in these tiers:

1. command-line `-I`, in operator order;
2. manifest-local modules and exact `[dependencies]`;
3. legacy `[build].module_paths`, in declared order;
4. the bundled standard library.

`-I` preserves first-hit behaviour and is an explicit operator override. Exact dependencies are also
explicit and may replace a bundled module of the same name. An implicit local module or a legacy
module path may not silently shadow the bundled library; discovery reports the collision and directs
the project to use an exact dependency or `-I`. Local/exact duplicates and generated/provided
duplicates are errors.

This keeps old invocations working while making their path search an input adapter to the catalog.
The adapter may be removed later without changing `jr-db` or any semantic query.

### §5. Availability and ownership are separate salsa inputs

`jr-db` installs catalog sources as `SourceFile`s, then records a `ModuleCatalog` salsa input.
`module_file(name)` becomes an in-memory lookup and performs no filesystem probe. `module_index`
enumerates that same catalog directly, so completion and code actions no longer need a workspace walk
merely to discover importable names.

`WorkspaceFiles` remains the independent ownership input for references, rename, and workspace
symbols. Changing edit scope therefore does not invalidate import resolution, and an external exact
dependency can be available for navigation without being silently treated as project-owned and
editable.

### §6. The LSP discovers projects after protocol initialization

The server must use the client's workspace folders/root URI, not the process working directory, to
find governing manifests. It maintains contexts by governing manifest (or scratch root) and chooses
the longest owned root for an opened file. Create/delete/rename notifications and manifest changes
refresh discovery; ordinary text edits update the existing `SourceFile` and do not rebuild the
catalog.

A malformed manifest is a project-configuration diagnostic surfaced by the server, not a silent
fallback and not merely a line on the server's stderr. This is the editor form of
`jr-manifest`'s existing “unknown keys are errors” rule.

### §7. Build-script modules are exact additions

The compiler-generated `Build` module and `provide_import` results are installed by name as exact
catalog additions for that build target. They do not append a directory to a global search list and
cannot make unrelated files beside them importable. A duplicate name fails the build with both
origins named.

## Rejected alternatives

- **Only make `src/` a conventional search root.** It removes one `-I`, but leaves query-time path
  probing, exact dependencies, build-script leakage, and the LSP's separate index unchanged.
- **Only add an exact dependency map.** Better dependency identity, but ordinary local modules still
  require a path convention every caller must know.
- **Keep discovery in `jr-cli`.** The LSP and build driver are independent consumers; putting project
  truth in a binary crate guarantees another copy.
- **Let `jr-project` install directly into `jr-db`.** That reverses the dependency direction and
  turns a filesystem/configuration module into a salsa adapter. The caller performs the small
  installation step instead.
- **Combine catalog and ownership in one salsa input.** A rename-scope change would invalidate every
  import and semantic result despite no module becoming available or unavailable.
- **Continue probing the filesystem inside `module_file`.** Salsa cannot observe directory changes,
  so the cached answer can be stale. Discovery belongs on the input side.
- **Add `override_bundled = true`.** An exact dependency named `Basic` or an operator `-I` already
  states the override at the point that supplies its replacement. A second switch can disagree with
  that declaration and adds no information.

## Consequences

Manifest-backed projects import `src/Foo.jr` and `src/Foo/module.jr` with no `modules/` directory and
no path flag. External dependencies have exact names and paths. The standard library, generated
modules, CLI, driver, database, and editor all consume one catalog and therefore cannot disagree
about which file `#import "Foo"` means.

Compatibility flags remain available, but they terminate at the catalog boundary rather than
infecting semantic APIs with path-search policy.
