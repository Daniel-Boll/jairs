# ADR-0154: W6 closes — a second build option, and two items declined with their reason

- **Status:** Accepted
- **Date:** 2026-09-01
- **Deciders:** dboll
- **Closes W6 — Metaprogram.** The last of PLAN §8.2's four items, after ADR-0152 (the static-data
  mechanism) and ADR-0153 (the message loop).
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

W6's remaining list was four items. Two are delivered here; two are **declined**, and declining them is a
decision with a stated reason rather than a gap left open — which is the difference this ADR exists to
record.

## Decision

### 1. `BUILD_OPT_LEVEL`, and the bootstrap rule an option that affects compilation needs

`BUILD_OPT_LEVEL :: 0;` declares the optimisation level, beside ADR-0102's `BUILD_OUTPUT`. Two options is
what makes PLAN §2.1's "a build script replaces the makefile" true of more than a filename: naming the
artefact and choosing the optimisation are the two things every makefile does.

Precedence is ADR-0102 §2's asymmetry, unchanged: **`-O` on the command line wins**, then a declared
`BUILD_OPT_LEVEL`, then the default. A declared name is a value the *artefact under compilation* chose; a
flag is an instruction from the *operator* compiling it, and the operator outranks the artefact.

**The problem this hit, which `BUILD_OUTPUT` did not have.** Reading a declared constant means
*compiling* — `file_consts` runs const-eval, which lowers MIR. So an option that affects compilation
cannot be read without already having chosen one. ADR-0102 avoided this entirely because naming an
artefact does not affect how it is built.

The answer is a **bootstrap configuration**: read the declaration under a fixed level, then set the real
one. That is sound *because of ADR-0142's check*, not by assumption — every corpus program behaves
identically at both optimisation levels, which is precisely the property ADR-0142 exists to assert. A
constant read at one level has the same value at the other. Without that check this would be a guess, and
it is worth noting that the check was built two waves earlier for a different reason and is what makes
this legal.

**A wrong declared value is `None`, not an error**, so the default applies. Same asymmetry: a bad
declaration is the artefact's mistake and must not stop a build the operator asked for.

**`Option<OptLevelArg>` on `BuildArgs`**, so "absent" is distinguishable from an explicit `-O1`. `jr run`
keeps a plain default, because there is no artefact declaring anything for it to lose to.

### 2. A `Build_Options` struct is blocked on a language feature, not on a threshold

ADR-0102 §3 deferred a `Build_Options` struct "once there are enough options to justify one". Probing
while writing this wave found a harder reason to keep waiting: **this language has no struct literals.**

```
BUILD :: Build_Options.{ output = "app", opt_level = 1 };
                       ^ error[E0117]: expected a field name after `.`
```

So the struct form is blocked on a *language* feature rather than on a judgement about how many options is
enough. That is a better place for the deferral to sit, because it is checkable: the day struct literals
land, this becomes possible, and until then no number of options makes it available.

### 3. Plugin hooks are **declined**, and the reason is ADR-0153 §1's

A plugin hook in Jai's sense is a callback the compiler invokes at a phase — which is the *poll* model:
the metaprogram is running concurrently with compilation and is handed control. ADR-0153 §1 rejected the
poll, for two reasons that apply unchanged here:

- it needs an execution model this compiler does not have, and PLAN §8.3 has since split that out as W11;
- a poll's observable behaviour depends on compilation *order*, which salsa's re-execution makes unstable
  by design — the same program would answer differently between runs depending on what was memoised.

So there is nothing to hook *into*. What a plugin would want to do, this language already offers in two
reproducible pieces: `noted_insert` generates code at compile time, and `noted_declarations` inspects at
run time. Both are values; neither depends on when the compiler happened to reach them.

**Declined rather than deferred**, because a deferral implies the design is agreed and only the work is
missing. It is not: adding hooks means adopting the poll, and that is a decision this project has now
made twice in the other direction.

### 4. Jai-style workspaces are **declined** for the same reason, and the useful half already exists

A Jai workspace is a compilation the build script *creates*, adds files to, and then polls for messages.
The poll is the whole point of it, so §3's reasoning applies to the entire construct.

What survives is the part that is not about polling — the *set of files being compiled* — and that already
exists: `jr build` computes it transitively from `#import`, `jr-db` carries a `WorkspaceFiles` notion the
LSP already uses, and `reachable_files` is what both the diagnostics gate and the MIR assembly walk. A
build script that wants to change the file set changes its imports.

**What would be needed if this is ever revisited:** a compilation unit that is a *value* — created,
configured and built by a `#run` — which is a very different thing from a poll and would need its own ADR.
Named here so that a later wave starts from the right question.

## Consequences

- **W6 — Metaprogram is complete.** `@note`, a reader, a query, note-driven generation, a build script
  naming its artefact, a build script choosing the optimisation, and a message loop that iterates. Two
  items declined with reasons rather than left ambiguous.
- **PLAN §2.1's W6 row needs two annotations**, marking plugin hooks and workspaces NOT DELIVERED with
  ADR-0154 §3–§4 as the reason — the same honesty the W1 row applies to `[..]T`.
- **A second named blocker for a struct-shaped future**: struct literals. `Build_Options` waits on them,
  and so would anything else that wants a configuration record written as a constant.
- **One integration test**, asserted through the *backtrace* — the one observable difference between the
  levels (ADR-0142 §3) — because a test that read a driver message would pass whether or not the level
  reached the mid-end.
