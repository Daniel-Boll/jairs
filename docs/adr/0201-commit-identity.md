# ADR-0201: One commit identity, signed, across the whole history

- **Status:** Accepted
- **Date:** 2026-09-05
- **Deciders:** dboll

## Context

Every one of the project's 427 commits was authored and committed as
`Daniel Boll <dboll@amazon.com>`, and exactly one was signed. This is a personal project; the work
address was the machine's default and nothing ever corrected it.

The decider asked for the whole history to carry the right identity — `Daniel Boll
<danielboll.academico@gmail.com>` — GPG-signed with the key registered to that address, and
sign-off-ed. And for one commit to be squashed: `b357dec shouldn't be here tho. SQUASH THIS`, which
removed a `main.jr` that should never have been committed.

## Decision

### §1. The identity is repo-local configuration, not a habit

```
user.name       Daniel Boll
user.email      danielboll.academico@gmail.com
user.signingkey BC362D94E7ACAC77
commit.gpgsign  true
tag.gpgSign     true
```

Repo-local rather than global, because the machine's global identity is the work one and is correct
for work repositories. `commit.gpgsign true` means signing is the default rather than something to
remember per commit; only `-s` still has to be passed.

### §2. The history was rewritten, not amended forward

427 commits including 132 merges. `git filter-branch` over `-- --branches --tags`, which preserves the
DAG exactly by mapping parents — 149 refs rewritten, every local branch and tag.

Four filters, and each is load-bearing:

- **`--env-filter`** sets `GIT_AUTHOR_*` **and** `GIT_COMMITTER_*`. The committer pair is the one an
  amend-forward approach forgets: setting only the author leaves `%cn`/`%ce` on the old identity, and
  forge platforms attribute by committer as well.
- **`--msg-filter`** strips any existing `Signed-off-by:` and adds exactly one. Appending
  unconditionally would have doubled it on the one commit that already had a sign-off — with the old
  address.
- **`--commit-filter`** is `git commit-tree -S "$@"`, because **nothing else in `filter-branch`
  signs**. It is wrapped in `git_commit_non_empty_tree`'s logic, so a commit whose tree matches its
  single parent is dropped.
- **`--index-filter`** removes `main.jr`. An index filter rather than a tree filter, because the latter
  checks out every commit and would have taken hours instead of three minutes.

### §3. The squash is the file never existing

`main.jr` was a scratch file of the decider's, sitting untracked. It was committed by a `git add -A`
in `88a7e49` — after that same session had explicitly noted it was the decider's work and should be
left alone. `b357dec` then removed it.

Folding the removal into the commit that added it would have been one reading of "squash". Removing
the file from **every** tree is the better one, and it is what "shouldn't be here tho" asks for: the
file is then in no commit at all, and `b357dec` — which now changes nothing — is dropped as empty by
§2's commit filter. 427 commits became 426.

### §4. Verified by measurement, not by inspection

The rewrite was **rehearsed on a scratch clone first** and checked there before the real repository was
touched. On the result:

| Property | Evidence |
|---|---|
| One identity | `git log --format='%an <%ae>\|%cn <%ce>' \| sort -u` → one line |
| Every commit signed | `%G?` → `G` for all of them |
| The right key | `%GK` → `BC362D94E7ACAC77` for all of them |
| One sign-off each | counted per commit; every count is 1 |
| `main.jr` gone | absent from every tree, checked by walking `git ls-tree` per commit |
| **Content unchanged** | `git diff <original-tip> <new-tip>` is empty |

The last row is the one that matters most: a history rewrite that changes what the code *is* would be
a catastrophe that no amount of signature checking would catch.

## Consequences

- Every commit on every local branch and tag carries the right identity and a good signature.
- The pre-rewrite commits survive under `refs/original/refs/heads/*` until a `gc`. That is the backup,
  and it is the reason the next point exists.
- **`main.jr` is in no commit.** The `SQUASH THIS` commit is gone with it.
- `refs/remotes/origin/*` still holds old-identity commits, necessarily: those are the remote's view.
  A push will need `--force-with-lease`, and that is the decider's call.
- All seven gates green after the rewrite: 1129 tests (1135 under gate 7), 170 Neovim checks, 19 Zed
  checks, zero grammar drift.

### A false green this exposed

`editors/zed/verify.sh` checks that the pinned grammar revision exists. It **passed immediately after
the rewrite**, on a revision that no branch contained any more — because the old commit was still an
object, reachable from `refs/original/`. `cat-file -e` cannot tell "present" from "reachable", and the
pin would have broken silently at the next `gc`.

The check now requires the revision to be an ancestor of some local branch. Its first version was
itself wrong in a way worth recording: a `while` on the right of a pipe runs in a **subshell**, so its
`exit 0` left only that subshell and the `return 1` after the pipeline always won — it reported a
revision on `main` as unreachable. A `for` over command substitution instead, and both directions are
verified: it passes on the live pin and fails on a zeroed one.

## Rejected alternatives

- **`git rebase --root --exec 'git commit --amend --no-edit -S -s'`.** It signs, but it flattens
  merges unless `--rebase-merges`, and replaying 132 merges risks conflicts on a history that is
  already correct. `filter-branch` maps parents and cannot conflict.
- **`git filter-repo`.** Faster and not deprecated, and it **cannot sign** — it has no commit-signing
  hook, so it would need a second pass anyway.
- **Rewriting `--all`.** It would include `refs/remotes/*`, which is the remote's view and meaningless
  to rewrite locally. `-- --branches --tags` is the scope that has meaning.
- **Squashing `b357dec` into its parent.** It keeps `main.jr` in the middle of history for one commit,
  for no benefit — the file should be in none of them.
- **Leaving the 200-odd `feat/*` branches on the old identity.** They are ancestors and duplicates of
  `main`'s history; leaving them would mean the repository still contained the old identity, and any
  future merge from one would reintroduce it.
