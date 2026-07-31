# ADR-0066: a trap reports the call chain of the frames that still exist

- **Status:** Accepted
- **Date:** 2026-07-31
- **Deciders:** dboll
- **W3's last feature**, and the one PLAN.md §2.1 lists as "panics/traps with backtraces".
- **Amends nothing, but bounds ADR-0020.** ADR-0020 gave a trap one source location. This adds the
  chain of *live procedure frames* beneath it, and §4 records precisely which frames cannot appear and
  why that is structural rather than an omission.

## Context

A trap today names one line and nothing else. With a call chain three deep, both engines print:

```text
error: division by zero
  --> bt.jr:6:12
```

which says *where* the division was but not *how the program got there*. That was checked by running,
along with four constraints that together define what a backtrace can be here. Each of these was
verified, not assumed:

1. **Native embeds a fixed message string per trap site, at compile time.** `report` in
   `jr-codegen-clif/src/body.rs` renders `jr_base::trap_message` into a read-only data object and
   passes its address to the runtime helper. A linked binary has no source map to consult.
2. **There is no runtime object.** `jr-link`'s docs say so in as many words: the trap helper is
   *generated into the object* by codegen, so there is no C runtime, no unwinder, and no symbol table a
   stack walk could read.
3. **The differential harness compares a trapping program's stderr byte for byte** between the VM and
   native (ADR-0020 §2). Any backtrace must therefore be *identical* in both engines, not merely
   similar — which rules out "native uses the platform unwinder, the VM uses its frames".
4. **Inlining erases callee frames, on purpose.** Both engines consume `optimized_file_mir` (`jr-db`'s
   `run.rs:81` for the VM, `build.rs:66` for native), and ADR-0021 §3 rewrites *every* copied span to
   the call site's span — because a callee's `MirSpan` names arenas in the callee's file while
   `resolve_span` is handed the caller's `FileHir`, so a surviving callee span would index the wrong
   file. The guarantee is structural: `Splice::span` takes no argument, so a callee span has no way
   through.

And one more, which is why this is a wave rather than a one-line change: **neither engine records who
called whom.** The VM's `Frame` holds `regs` and `slots` and nothing else; `TrapSite` names only the
procedure that was executing. A four-deep *recursive* chain — which the inliner cannot flatten — still
reports a single line, so the missing piece is real bookkeeping, not an inlining artefact.

## Decision

### 1. Both engines maintain a shadow call stack of `ProcRef`s

Each engine pushes the callee's `ProcRef` on entry and pops it on return. The VM does this in `Vm::call`,
where `depth` is already incremented; native does it around each call, writing to a module-level
mutable data object that codegen declares (`declare_data` already takes a `writable` flag).

**A shadow stack rather than the platform's.** Reading the real stack needs frame pointers, an
unwinder and a symbol table — constraint 2 says none exists, and constraint 3 says whatever native did
would have to match the VM byte for byte anyway. A shadow stack is the only mechanism *both* engines can
implement identically, which makes agreement structural rather than something the harness has to catch.

**`ProcRef`, not a name.** A `ProcRef` is one word and already the identity both engines use for a
procedure. Names are resolved for *rendering*, by the side that has the HIR — the same split ADR-0020
§4 already uses for the trap's own location, where the VM reports an identity and `jr-db` renders it.

### 2. The chain is rendered under the location, innermost first

```text
error: division by zero
  --> bt.jr:5:16
  in countdown
  in countdown
  in countdown
  in main
```

`jr_base::trap_message` grows a `frames: &[&str]` parameter and emits one `  in <name>` line per frame,
innermost first. It stays the single place that decides what a trap says (its module docs argue why),
so the two engines cannot drift in punctuation or order.

**Innermost first, matching rustc and every backtrace a reader has seen.** The trapping frame is the
one they are looking for, and putting it first means they do not count lines to find it.

**Names, not `path:line:col` per frame.** A per-frame *line* would be the call site in each caller,
which needs a return-address-to-span map in the binary — a static table per call site, which is the
subsystem §4 declines. A name answers "how did I get here" and is what the shadow stack can carry
honestly.

### 3. Native embeds the names, so the message is still fixed at compile time

Constraint 1 says native cannot render at trap time. It does not have to: the *reason* and *location*
are already per-site constants, and the frame **names** are a per-procedure constant too. Codegen emits
one read-only string per procedure and a table from `ProcRef` index to that string; the runtime helper
walks the shadow stack and writes `  in <name>` for each entry from the table.

So the helper grows from "write these bytes" to "write these bytes, then walk a stack and write a line
per frame" — still no unwinder, still no symbol table, and no source map. The bytes it produces are
`trap_message`'s, because the *format* is shared even though the assembly is per-engine.

### 4. Inlined frames do not appear, and that is structural

A frame the inliner removed **has no runtime existence** — there is no call, so there is nothing to
push. Both engines agree because both inline identically: the pass is deterministic (a fixed
`MAX_INLINE_STATEMENTS` of 24 against the same `Callees`), so the shadow stacks match.

This is stated rather than hidden, because a reader comparing a backtrace against their source *will*
notice a missing frame. Verified by running, which corrected a wrong guess in this ADR's own draft:

```jr
inner  :: (a: s64, b: s64) -> s64 { return a / b; }
middle :: (x: s64) -> s64 { return inner(x, 0); }
main   :: () { y := middle(10); exit(y); }
```

reports

```text
error: division by zero
  --> bt.jr:6:12
  in middle
  in main
```

**`inner` is absent and `middle` is present**, and the rule that decides it is not size: `is_inlinable`
refuses any callee that *makes a call of its own* (ADR-0021 §4's caller-side exclusion), so only leaves
inline. `inner` is a leaf and was spliced into `middle`; `middle` calls `inner` and so was never
inlined into `main`. The draft of this ADR predicted both would vanish, leaving only `main` — running it
was what showed otherwise.

That is exactly the property worth having: the chain describes the *execution*. At run time there were
two frames, and two frames are what it prints. A backtrace that invented `inner` would be describing
the source instead, and ADR-0020 §4 already held that reporting no location beats reporting a
neighbouring one. The same argument applies to a frame.

**Rejected: recording an inline chain in the MIR so inlined frames can be named.** This is what a
production compiler does (rustc's `SourceScope`, LLVM's `DILocation` inline-at chains), and it is the
right long-term answer. Rejected *here* because it means every `MirSpan` gains an inline-provenance
field that every pass must maintain, and ADR-0021 §3's structural guarantee — a callee span cannot
survive, because `Splice::span` takes no argument — would have to be replaced with a discipline the
verifier cannot check. That is a mid-end change of its own, and it is exactly the kind of "a flag some
passes ignored" that PLAN.md §5's first failure mode is about.

### 5. What is deliberately absent

- **A line number per frame.** §2: it needs a return-address-to-span table in the binary. The
  innermost frame's line — the one that matters — is already there from ADR-0020.
- **Inlined frames** (§4), and **`#c_call` or `#foreign` frames**, which are not Jairs calls and push
  nothing.
- **A depth limit on the printed chain.** A deep recursion prints a long backtrace. Truncation ("… and
  N more") is a presentation decision worth taking when something actually produces an unreadable one;
  guessing a limit now would be the same unmeasured guess `MAX_INLINE_STATEMENTS` already is, and at
  least that one is documented as such.

## Consequences

- **`jr_base::trap_message` changes signature**, so both call sites change together — which is the
  point of it being one function. Its doctest and the two shape tests move with it.
- **The VM gains a `Vec<ProcRef>` beside `depth`**, pushed and popped in `Vm::call`. `TrapSite` grows a
  frames field, and `jr-db`'s `trap_location` gains a sibling that resolves each `ProcRef` to a name.
- **Native gains a mutable data object and a per-procedure name table**, and the generated trap helper
  grows a loop. This is the first *mutable* global the back end emits, which is worth noting because
  every other data object it makes is read-only.
- **Every corpus program that traps changes its expected stderr**, so the differential harness's
  trapping cases all move at once — a large mechanical diff whose *shape* is what to review.
- **No new diagnostic code.** A backtrace is a runtime message, not a diagnostic. **E0258 is still the
  first free code.**
- **W3 closes with this**, and the honest summary of it is: a trap names its line and the frames that
  existed, which is less than a source-level backtrace and exactly what the execution had.
