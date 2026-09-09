# ADR-0224: `assert` is a source-located compiler check

- **Status:** Accepted
- **Date:** 2026-09-09
- **Deciders:** dboll
- **Amends:** ADR-0017 §MIR statements and ADR-0020 §trap source locations.

## Context

Jairs has source-located traps for arithmetic, bounds, invalid variant reads and `todo;`, but no
ordinary assertion. Library code can spell an `if` followed by `todo;` or call a foreign abort
routine, but those substitutes report the wrong reason, cannot carry a stable message across the VM
and native engines, or introduce a call frame that is not part of the asserted program.

Jai exposes assertions as a compiler-known facility. Jairs already has compiler-recognised call
intrinsics such as `type_info`, `any_of` and `atomic_load`: the name is recognised only when ordinary
resolution finds no declaration, so a program may still declare and call its own procedure of the
same name.

The optional message creates a representation decision. A generic trap kind carries one static
sentence, while an assertion message belongs to one call site. Reconstructing it in each engine would
give three implementations of the wording the differential harness compares.

The decider chose:

- `assert(condition)` and `assert(condition, "static message")` as a compiler-recognised call;
- a boolean condition and, in this wave, a literal static string message;
- a dedicated MIR assertion statement whose failure is an observable effect;
- identical source-located failure text in the VM, Cranelift and LLVM;
- ordinary trap behaviour: failure aborts immediately and does not run `defer`;
- E0230 when a false assertion is reached during compile-time evaluation.

## Decision

### 1. `assert` is an unresolved-name intrinsic and may be shadowed

`assert(condition)` and `assert(condition, "message")` are recognised only when the callee is the
unresolved unqualified name `assert`. A declaration, import, local or parameter named `assert` keeps
ordinary resolution and call behaviour.

The intrinsic returns `void`. Its first argument must be `bool`; ordinary E0214 reports a type
mismatch. It takes exactly one or two positional arguments; ordinary E0216 reports any other arity.
Named arguments are not accepted because the intrinsic has no declaration whose parameter names could
be part of the language contract.

Rejected alternatives:

- **Reserve `assert` as a keyword.** It would unnecessarily break an existing declaration and make a
  call-shaped operation a special parser production.
- **Declare `assert` in `Basic`.** A library procedure adds a frame, cannot force identical native and
  VM trap rendering, and makes compile-time execution depend on the library implementation.
- **Add `#assert`.** A compiler directive suggests a compile-time-only operation; this assertion is an
  ordinary runtime check that also works when reached by the compile-time interpreter.

### 2. The optional message is a static string literal in this wave

The second argument, when present, must be a string literal after normal escape decoding. E0297 reports
a non-literal message and explains that formatted or computed assertion messages are not part of this
form.

Literal-only is deliberate. Semantic checking can read a literal without running const evaluation,
while accepting an arbitrary compile-time expression would add a new downstream evaluation target and
make MIR availability depend on that result. That is useful only together with a broader formatted
assertion design, so it remains a later decision rather than being partially approximated here.

The failure reason is:

- `assertion failed` without a message;
- `assertion failed: <message>` with one.

One shared formatter constructs that reason. Engines pass it to `jr_base::trap_message`, which continues
to own the `error:` prefix, location, backtrace formatting and trailing newline.

Rejected alternatives:

- **Accept a runtime `string`.** Native code would need a new dynamic trap ABI and the compile-time VM
  would no longer be comparing the same static bytes.
- **Store only a generic `TrapKind::Assertion`.** It cannot represent the call-site message and would
  force the engines to discard it or find it through a second side channel.
- **Use the literal as the complete reason.** The stable `assertion failed` prefix identifies the
  operation even when the supplied text is empty or terse.

### 3. MIR carries an explicit assertion statement

MIR gains:

```text
assert <condition> ["message"] @ <span>
```

The statement stores the boolean operand, the optional decoded message and the call expression's
`MirSpan`. It produces no value and may terminate execution, so dead-code elimination must never remove
it merely because nothing consumes a result. Operand substitution and inlining may rename its condition;
inlining keeps the caller-side span policy used by the other statements.

The dedicated statement is preferred to lowering the assertion into HIR branches. HIR should retain the
call the writer wrote, and a synthesized branch would lose the fact that the false edge is specifically
an assertion with a message. It is also preferred to translating the operation to `todo`: the reasons,
condition and message are different.

Rejected alternative:

- **Expand directly to a MIR branch and an assertion-flavoured unreachable terminator.** It reuses CFG
  machinery, but it turns one source check into extra blocks before optimisation, makes the message a
  property of an otherwise generic terminator, and weakens the invariant that an assertion itself is an
  undeletable effect. An explicit check follows the existing `BoundsCheck` and `TagCheck` model.

### 4. Failure is an immediate, source-located trap in every engine

The VM bytecode gains an assertion instruction carrying the condition and optional message. Cranelift
and LLVM emit a cold failure branch that calls the existing `jr_trap` helper with the shared rendered
message. A true condition falls through with no effect.

The statement's span is the active source location while the failure is emitted or interpreted. Failure
therefore names the assertion call's line and carries the ordinary live backtrace. It aborts immediately
and does not execute pending `defer` statements, matching every other trap rather than structured exits
such as `return`.

During `#run` or another constant evaluation, the VM executes the same instruction. A false condition
becomes the existing E0230 with the assertion reason; an assertion in an untaken path is inert.

### 5. Tooling preserves the call and offers the intrinsic

No parser or tree-sitter grammar change is required: both forms are ordinary calls. The formatter must
preserve and idempotently format both arities. LSP completion offers `assert` as a compiler intrinsic
function with an insertion snippet, not as a keyword. Hover, definition, references and rename continue
to follow ordinary resolution, so a user declaration named `assert` remains navigable.

## Consequences

- Assertions have one meaning at runtime and compile time and one byte-for-byte failure message across
  all three engines.
- A message is static and source-owned; no dynamic trap ABI or formatting language is implied.
- MIR optimisers must handle the new effect explicitly, and exhaustive matches make omissions compile
  errors.
- User code may shadow the intrinsic, consistent with every existing unresolved-name intrinsic.
- Variadic formatting, arbitrary compile-time message expressions, a test declaration model and
  `#assert` remain separate future decisions.
- All six ordinary gates and gate 7 are required because MIR, the VM and both native backends change.
