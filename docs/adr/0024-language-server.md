# ADR-0024: The language server — a worker snapshot, a span scan, and negotiated positions

- **Status:** Accepted
- **Date:** 2026-07-27
- **Deciders:** dboll

## Context

`jr-lsp` has been a one-line doc comment and an empty `[dependencies]` list since the
workspace was scaffolded, while `lsp-server 0.10.0` and `lsp-types 0.97.0` have sat
pinned and unused in `PLAN.md` §5's dependency table. This ADR is that crate finally
existing.

**Two corrections to the plan come first, because they are why this wave happens now.**

`PLAN.md` §7 has claimed for three waves that "every compiler criterion in §1.4 is met"
and that "the three boxes still open are editor packaging and a Linux CI run, neither of
which is compiler work". The first half is right; the second is wrong. §1.4's open box
reads *"VS Code: diagnostics + hover + goto-def"*, and that is not packaging — it needs a
crate that does not exist. §1.3 scopes `jr-lsp` into the slice in as many words, with the
justification **"Proves the salsa boundary is real"**, and §2.1 gives wave W9 only the
*depth* — completion, rename, inlay hints. A basic server belongs here.

The second correction is about the performance number, and it is mine. ADR-0022 §1 says
"§1.3's estimate has been waiting for" a performance number, and §7 repeated it. **§1.3
contains no performance estimate.** Its only figure is §1.4's "Estimated: 10–14 weeks
solo", which is a *schedule*. §2.1 assigns "published compile-throughput number" to wave
**W8**, and ADR-0019 §6's actual wording is a *trigger* — the inliner must exist before
"the first compile-throughput or runtime number `PLAN.md` proposes to publish" — not an
obligation to produce one. The inliner exists, so the trigger is discharged and nothing
is owed. Per this project's own rule, ADR-0022 stays as written; this ADR is the record
that one of its Context sentences was false.

Four facts about the existing code decide the shape, and all four were read rather than
assumed.

**Every query the three capabilities need already exists.** `file_diagnostics` for
diagnostics; `resolved` for goto-definition, whose `ResolveMap` maps
`(ExprScope, ExprId)` to a `Res`; `checked` for hover, whose `TypeMap` answers
`expr_type(scope, id)`. Nothing new is needed in `jr-db` except a way to share it.

**Nothing maps a source offset to a HIR node.** That is what ADR-0013 deferred along
with `AstIdMap`. It is the first thing hover asks for.

**The database is already shareable, by accident of earlier decisions.** `Interner` is an
`Arc<ThreadedRodeo>`; `source_map`, `file_inputs`, `module_search_paths` and `pool` are
all `Arc<Mutex<_>>`; and salsa's `Storage` is `Clone`. So a snapshot is a field-wise clone
and costs nothing.

**Cancellation is salsa's, not ours.** salsa 0.28.1 ships
`Cancelled::{Local, PendingWrite, PropagatedPanic}` and `Cancelled::catch`, and
`StorageHandle` carries a `Coordinate { clones, cvar }`. A writer that wants to bump the
revision signals cancellation and *blocks until the clone count drops*. In-flight readers
unwind. We do not implement cancellation; we use it, and it imposes one obligation on us
in return (§2).

## Decision

### 1. A cursor position finds its HIR node by scanning spans, innermost first

Each request walks the file's HIR arenas comparing spans to the offset and keeps the
smallest containing node. ADR-0013 put spans directly on HIR nodes, so this needs no new
data structure and cannot go stale.

**The arena must be part of the answer, not inferred.** `FileHir::exprs` and every
`Body::exprs` both start at index 0, which is the collision `ExprScope` exists to prevent
and which has already caused one real bug in `jr-hir`'s `ResolveMap`. So a located
expression is an `(ExprScope, ExprId)` pair everywhere, and the scan visits each body's
arena under its own scope.

This is O(nodes) per request. That is precisely the cost ADR-0013 named its own revisit
trigger — "measure keystroke-to-diagnostic latency and decide then whether `AstIdMap` is
worth building" — so this wave is what makes the measurement possible rather than
hypothetical, and the number is not owed by it.

**Rejected: build `AstIdMap` now.** It would make node identity survive unrelated edits
and turn lookup into a map hit. ADR-0013 deferred it *pending evidence*, and building it
in the wave that first produces the evidence is backwards. ADR-0019 §3 already records a
wave lost to believing `AstIdMap` was a blocker when it was not; building it on the same
hunch is that mistake with the sign flipped.

**Rejected: find the CST token by offset, then map to HIR.** rowan's `token_at_offset` is
cheap and precise, and the CST-to-HIR map it then needs is exactly `AstIdMap`. It arrives
at the same wall having added a traversal.

### 2. Requests run on a worker against a database snapshot, and salsa cancels them

The main thread owns the writer database and does nothing slow: it reads messages, applies
`didOpen`/`didChange` as writes, and hands read requests to a worker. The worker takes a
`JairsDatabase::snapshot()` per request, wraps the handler in `Cancelled::catch`, and
drops the snapshot when it finishes or unwinds.

`didChange` replaces the whole document. salsa's invalidation grain is the file, so
patching ranges would buy nothing analytically and would add arithmetic that is wrong in
interesting ways around line endings.

**The obligation salsa's mechanism imposes, stated so it is not discovered later:** a
writer blocks until the snapshot count drops to one. So a worker that holds a snapshot
across requests, or ignores `Cancelled`, does not merely waste work — it **stalls the
next keystroke**. Taking the snapshot per request and dropping it on unwind is not
tidiness; it is what makes the write side non-blocking.

**Rejected: a single-threaded blocking loop.** `PLAN.md` §5 chose `lsp-server` partly on
that reasoning — "sync, you own threading" — and it is simpler, with no snapshot and no
`catch`. Rejected because the whole point of §1.3's "proves the salsa boundary is real" is
that the LSP exercises what salsa is *for*, and a loop that cannot be interrupted never
touches cancellation, which is the half of incrementality that only an editor reveals. It
would also have to be replaced rather than extended the first time a file is slow.

**Rejected: incremental text sync.** See above; the grain is the file.

### 3. Position encoding is negotiated, with UTF-16 implemented as the fallback

The initialize result advertises `positionEncoding: ["utf-8", "utf-16"]` (LSP 3.17, which
`lsp-types` 0.97 supports). When the client takes UTF-8, a position's character is a byte
offset within its line and conversion is arithmetic on what `Span` already is. When it
does not, columns are converted through UTF-16 code units.

Both paths exist and both are tested, because the wrong one is silently wrong rather than
broken: it is correct for ASCII and off by one per non-ASCII character. This repository's
own sources are full of em dashes.

**Rejected: UTF-16 only.** Universally supported and one code path. Rejected because the
conversion then runs on every position in every request and response, including the ones
that did not need it, and the negotiated path is strictly less code on the hot path.

**Rejected: assume bytes and do not negotiate.** It would pass every test written against
an ASCII corpus and be wrong the moment a comment contains a dash — working in the tests
and wrong in use, which is the worst combination available.

### 4. Handlers are pure functions; one test speaks the real protocol

Every capability is a function of `(&db, params) -> response` with no I/O, tested
directly. Separately, **one** end-to-end test spawns the binary and speaks JSON-RPC over
stdio for `initialize` plus one `hover`.

The smoke test is not belt-and-braces. Handler tests alone would pass with a completely
broken transport, and this project has already been bitten by exactly that: the first
native run of `024-hello.jr` printed both its lines perfectly and exited **1**, and no
in-process assertion noticed. A language server's transport is the same kind of surface —
correct-looking output, wrong framing, nothing to see.

**Rejected: only end-to-end over stdio.** Every assertion would pay process startup and
protocol framing, and a failure would say the response was wrong without saying which
layer produced it.

**Rejected: manual verification in an editor.** Nothing regression-tested, on the first
wave whose deliverable `cargo test` cannot see — which argues for testing it more
carefully than usual, not less.

### 5. The server ships as `jr lsp`, not a second binary

`jr-lsp` is a library; `jr-cli` gains an `lsp` subcommand that runs the stdio loop. One
binary to build, install and point an editor at, and it matches how `jr` already carries
`check`, `fmt`, `run`, `build` and `parse`.

**Rejected: a separate `jr-lsp` binary.** `PLAN.md`'s tree comment ("language server")
reads either way. A second binary means a second thing to install and a second place for
version skew between the server and the compiler that produced its diagnostics.

No new dependency enters the workspace: `lsp-server`, `lsp-types`, `serde`, `serde_json`
and `crossbeam-channel` are all already pinned in §5's table under ADR-0009's discipline.

## Consequences

### Positive

- ADR-0007's central claim — that the LSP is a consumer of the same queries and not a
  second front end — stops being an assertion. The handlers call `file_diagnostics`,
  `resolved` and `checked` and add no analysis of their own.
- §1.4's first open box becomes reachable, and the README's honest table loses its
  largest "Not started".
- ADR-0013's revisit trigger becomes measurable, in the wave that also explains what
  would be measured.
- Cancellation is exercised, which is the half of salsa's design a batch compiler never
  touches.

### Negative

- A snapshot held too long blocks the next edit. That is a real footgun and the mitigation
  is a discipline (§2) rather than a type.
- The span scan is linear per request, so a large file pays for every hover. Known, and
  the trigger for fixing it is now instrumented rather than guessed.
- Two position-encoding paths.
- Hover reports a type and nothing else — no documentation, no signature rendering. §2.1
  gives W9 the depth, but a user will notice.
- The LSP is the first component whose correctness the six gates only partly see.

### Follow-on work this forces

- **Into this wave:** the two plan corrections in §7 and §1.5, so the next reader is not
  told the slice is done when a scoped crate is empty.
- **Into wave W8:** the compile-throughput number, where §2.1 puts it — and the
  keystroke-to-diagnostic latency ADR-0013 wants, which this wave makes obtainable.
- **Into wave W9:** completion, rename, references, inlay hints, semantic tokens, and
  hover that renders more than a type.
- **Into whichever wave measures it:** whether `AstIdMap` earns its keep. The evidence is
  now collectable; ADR-0013's trigger stands.

## Alternatives considered

Each fork's rejected alternatives are argued at its own point of decision. One
alternative spans the whole ADR.

**Take the performance number instead, as §7 said.** Rejected on the facts in the
Context: §2.1 assigns it to W8, no §1.3 estimate is waiting for it, ADR-0019 §6's
condition is a trigger rather than a debt, and — per ADR-0023's follow-on work — Jairs-0
cannot express a runtime workload anyway, so the figure would describe the front end
after three consecutive mid-end waves. Meanwhile a crate §1.3 calls slice scope was
empty and §7 was describing it as packaging. One of those is a real gap and the other is
a number nobody asked for.
