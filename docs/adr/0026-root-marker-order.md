# ADR-0026: `.git` before `modules` as the workspace root marker

- **Status:** Accepted
- **Date:** 2026-07-27
- **Deciders:** dboll

**This ADR amends [ADR-0025](0025-editor-integration.md) §1**, in the sense ADR-0018 §5
amended ADR-0017: that decision stands except for its choice of root marker order, which
was wrong.

## Context

ADR-0025 §1 shipped `editors/nvim/lsp/jairs.lua` with

```lua
root_markers = { "modules", ".git" },
```

and justified it in a doc comment: *"`modules/` is the marker rather than a `.git`
directory: a Jairs project is defined by having modules to import, and vendoring one
inside a git repository should not attach it to the outer project."*

That reasoning is fine as far as it goes. It is also **falsified by the repository it was
written in.** Opening `tests/corpus/valid/024-hello.jr` rooted the server at
`tests/corpus`, because `tests/corpus/modules/` exists — it is the fixture directory the
import tests use.

Two facts make this a decision to revisit rather than a typo to patch.

**`root_markers` order is priority, not proximity.** `:h vim.fs.root` is explicit: the
first pattern in the list that matches anywhere up the tree wins, even when a later one
matches closer to the file. So listing `modules` first did not mean "prefer the nearest
marker"; it meant "`modules` always beats `.git`", which is a much stronger claim than the
doc comment made. Verified both list forms against a real buffer rather than reasoned
about.

**A directory named `modules` is not distinctive.** Node projects, Terraform trees,
Python packages and this repository's own test fixtures all have one. ADR-0025's argument
assumed such a directory implies a Jairs project; it does not.

It is worth naming how this was found: not by review, but by running the integration
against the repository it ships with. ADR-0025 §6 argued that instructions nobody has
executed are the same shape as the plan claims ADR-0024 had to correct. This is that
argument collecting on its own decision.

## Decision

```lua
root_markers = { ".git", "modules" },
```

`.git` first. In the common case a Jairs checkout is a git repository and this gives the
repository root. `modules` stays as a fallback for a non-git tree, where it is the only
signal available — and its failure mode there is bounded, because a tree that is neither
a git repository nor a Jairs project has no reason to be open in this filetype.

`editors/nvim/verify.lua` now asserts the resolved `root_dir` equals the repository root,
so the ordering is pinned rather than merely corrected.

**Rejected: keep `modules` first and exclude fixture paths.** There is no way to say "a
`modules/` directory, but not a test fixture" that is not a list of this repository's
directory names embedded in an editor config.

**Rejected: drop `modules` entirely and use only `.git`.** Tidier, and it would have made
this ADR a deletion. Rejected because a Jairs source tree extracted from a tarball has no
`.git`, and rooting at the file's own directory then means a cross-file `#import` resolves
against the wrong tree — the failure ADR-0025 §5 had just finished fixing from the other
direction.

**Rejected: the nested form `{ { ".git" }, { "modules" } }`.** Neovim supports it for
explicit priority groups, and it resolves identically here — both forms were tested. The
flat list already expresses priority, so the nesting adds punctuation and no meaning.

## Consequences

### Positive

- The server roots at the project rather than at whichever ancestor happens to contain a
  `modules` directory.
- `verify.lua`'s check count rises to 23 and the ordering cannot regress silently.
- One more instance of the same lesson, on the record: a rationale that sounds sound is
  not evidence. Five consecutive ADRs have now had a claim corrected by running something.

### Negative

- A Jairs tree with no `.git` and a stray `modules/` above it still roots too high. Bounded
  and unlikely, and stated rather than engineered around.
- ADR-0025's doc comment now reads as the wrong argument in the git history. That is what
  immutable ADRs cost, and it is cheaper than a record that quietly rewrites itself.

### Follow-on work this forces

- **Into the VS Code extension:** it will need the same root logic, and it should read it
  from one place rather than re-deriving it. There is no shared configuration between the
  two editors yet; when there is, this is one of the two things in it.
