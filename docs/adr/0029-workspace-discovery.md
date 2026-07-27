# ADR-0029: workspace file discovery

- **Status:** Accepted
- **Date:** 2026-07-27
- **Deciders:** dboll

## Context

Two requested features need something this compiler has never had: a list of the files
that exist.

- **Whole-workspace rename** must edit every file that mentions a name. A rename that
  misses one produces a build that no longer compiles — worse than refusing, because the
  user has already accepted the edit and moved on.
- **An auto-import quick fix** must offer `#import "Basic";`, which means knowing that
  `Basic` is available. `jr_db::module_file` only ever *probes*: given a name it tries
  `<dir>/<Name>/module.jr` then `<dir>/<Name>.jr`. Nothing enumerates.

What exists today: `ModuleSearchPaths`, a salsa input holding the directories the CLI was
told about; `JairsDatabase::file_inputs`, a map of the files something has explicitly
loaded; and `jr-driver`, a one-line stub whose eventual job this is. `PLAN.md` §7 has
listed "a workspace notion" as owed work for several waves without saying what one is.

## Decision

### 1. The workspace is the search paths plus the root's tree, walked for `*.jr`

No new file format and no new concept in the language. The set is:

- every directory in `ModuleSearchPaths`, and
- the tree under the server's root directory, which the client supplies at `initialize`
  and which ADR-0026 already resolves by `.git` then `modules`.

Walked for `*.jr`, skipping `target/`, `.git/`, `node_modules/` and any dot-directory, and
**not following symlinks** — a symlinked parent is how a walk becomes infinite, and the
alternative (tracking visited inodes) is more machinery than the case deserves.

**Rejected: a `jairs.toml` manifest declaring source roots.** Explicit, fast, and
something `jr-driver` will plausibly want later. Rejected for now because it is a new
language-adjacent artifact that this repository, every corpus directory and every user's
scratch file would need — or would need a fallback to exactly the rule above, at which
point the rule is doing the work and the manifest is an optional override. It stays
available as a later addition rather than a prerequisite.

**Rejected: only what the client has opened, plus transitive imports.** Zero discovery
code and zero staleness. Rejected because it makes "whole-workspace rename" a promise the
server cannot keep, and a rename that silently covers three of five files is the outcome
§4 of ADR-0030 exists to prevent.

### 2. The file list is a salsa input, refreshed by a client file watcher

`WorkspaceFiles` is an input holding the discovered paths. The server registers
`workspace/didChangeWatchedFiles` for `**/*.jr` via `client/registerCapability` and
re-scans on notification.

**A directory walk is untracked I/O.** Salsa cannot know the filesystem changed, so
without a watcher the list is stale from the moment it is taken, and stale in the
direction that matters: a module created after startup is unimportable, and a file created
after startup is invisible to rename. Making the list an *input* rather than a query is
what keeps salsa honest — the staleness lives in one place, and refreshing it invalidates
exactly what depended on it.

Clients that do not advertise `dynamicRegistration` for watched files fall back to
re-scanning on `didOpen` and `didSave`. Neovim's support is **verified in
`editors/nvim/verify.lua`, not assumed** — that is the rule ADR-0025 §6 established, and
the last two waves each found a bug in the thing that was going to be assumed.

**Rejected: polling on a timer.** It makes the answer depend on when you asked, which is
the hardest kind of bug to reproduce.

### 3. Discovery yields paths, not loaded files

A path list is cheap; reading and parsing every file is not. Discovery therefore does
**not** populate the database. A consumer that needs a file's HIR loads it on demand, and
salsa caches the result.

The consequence is stated rather than hidden: **the first whole-workspace rename parses
the whole workspace.** On this repository that is trivial; on a large one it is a
noticeable pause on a keystroke, and it is the same cost `workspaceSymbol` pays. ADR-0013's
latency trigger now has a third reason to be measured, and the honest position is that no
number exists yet.

**Rejected: eagerly load everything at `initialize`.** Simpler consumers, and it converts
a per-request pause into one startup pause. Rejected because it makes opening a single
file in a large tree pay for files the user may never touch, and because a parse error in
an unrelated file would then be computed for nothing.

### 4. A bound, and a refusal when it is hit

The walk stops at 10 000 files, and a workspace larger than that makes
`WorkspaceFiles::truncated` true. Consumers that must be exhaustive to be correct —
rename — **refuse** rather than proceed on a partial list. Consumers that are merely
better with more — `workspaceSymbol` — proceed.

Unbounded is the wrong default for a walk rooted at whatever directory an editor happened
to open, and a silent cap is how a rename quietly misses a file.

## Consequences

### Positive

- Rename and auto-import become expressible, and both can tell the difference between
  "no other file mentions this" and "I do not know".
- The workspace is defined in one place, so `jr-driver` inherits a definition rather than
  inventing a second one.
- The staleness window is a named property of a named input.

### Negative

- The first request that needs the workspace parses it, and there is no number for what
  that costs.
- A file created by another process between watcher events is invisible. With the
  `didOpen`/`didSave` fallback the window is much larger.
- The ignore list is a heuristic, and a project that keeps Jairs sources in a dot-directory
  is wrong by fiat.

### Follow-on work this forces

- **A latency measurement** covering discovery, the offset scan and completion together.
  Three features now want the same number.
- **`jr-driver`** should consume this rather than growing its own notion when it stops
  being a stub.
- **An optional manifest**, if and when the walk's heuristics stop being enough.
