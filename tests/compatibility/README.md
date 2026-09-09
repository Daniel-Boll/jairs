# Executable compatibility probes

This directory implements ADR-0219. `probes.toml` maps one pinned
`The_Way_to_Jai` example from every example-bearing top-level chapter to a
small repository-owned Jairs probe.

Ordinary tests never read or execute the submodule. The upstream path and
revision are provenance; only files below `probes/` execute.

Each probe runs from a copied temporary directory. It may therefore create
files or build artefacts without changing the worktree or sharing process state
with another probe.

Statuses mean:

- `source-compatible` — the selected pinned `.jai` source also passed
  `jr check` unchanged when this manifest was reviewed;
- `ported` — the same capability is executable with documented Jairs spelling
  or library differences;
- `blocked` — a focused owned source pins the current missing capability and
  exact diagnostic code;
- `divergent` — Jairs deliberately makes a different choice, pinned by an
  executable acceptance or refusal.

The files do not belong in `tests/corpus/`: compatibility evidence is not the
language specification, and some probes are intentionally invalid.
