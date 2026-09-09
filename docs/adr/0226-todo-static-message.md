# ADR-0226: `todo` accepts empty parentheses or one static description

- **Status:** Accepted
- **Date:** 2026-09-09
- **Deciders:** dboll
- **Amends:** ADR-0223 §1, §2 and §5.

## Context

ADR-0223 deliberately shipped only `todo;` and deferred custom messages. That made an unfinished path
terminal and source-located, but it left no place to say *what* remains unfinished. The decider wants both
call-shaped forms:

```text
todo();
todo("Not implemented");
```

The existing statement form must remain source-compatible. This is still a control-flow statement, not an
ordinary procedure call or expression: it has no result type, cannot be shadowed, terminates its path and
does not run pending defers.

A runtime-computed message would be a different feature. Native traps currently embed a compile-time
reason and call one fixed helper; passing an arbitrary `string` would add a dynamic trap ABI to MIR, the VM
and both native back ends merely to describe unfinished code.

## Decision

### 1. There are three statement spellings

All of these are legal wherever `todo;` is legal:

```text
todo;
todo();
todo("Not implemented");
```

`todo;` and `todo();` are the same no-message operation. Parentheses contain either nothing or exactly one
string literal. A computed value, a non-string literal or several arguments is a syntax error rather than a
partially supported call.

The literal-only rule belongs in the grammar. It makes the static contract visible to tree-sitter and keeps
HIR from carrying an expression that sema would later have to reject. This follows the shape of other
syntax-level static strings such as an import path or note payload.

Rejected alternatives:

- **Remove `todo;` in favour of `todo()`.** Existing source and ADR-0223's explicit spelling remain valid.
- **Accept any expression and reject it in sema.** The language would parse a runtime message shape it has
  no intention of evaluating.
- **Accept a computed string.** That requires a runtime operand and a new trap ABI in three engines.

### 2. The message is decoded once and travels with the terminal reason

HIR stores:

```text
Todo { message: Option<String>, span: Span }
```

The string is escape-decoded during lowering through the same decoder ordinary string literals use.
`todo;` and `todo();` carry `None`; `todo("")` carries `Some("")`, preserving the fact that the caller
supplied a description even when it is empty.

MIR changes `Unreachable::Todo` to `Unreachable::Todo(Option<String>)`. The payload belongs on the enum
variant rather than as a generic field on every unreachable terminator: a message attached to
`FellOffEnd` or `StrayJump` would be an invalid state the type needlessly admitted.

Inlining clones the message while retaining ADR-0223's source rule: an inlined terminal path is attributed
to the call site, because the callee's source location has no runtime frame after inlining.

### 3. One shared function decides the reason text

`jr_base::todo_reason` is the only formatter for the reason:

```text
reached todo
reached todo: Not implemented
```

An explicitly empty message renders `reached todo: `, matching `assert(false, "")`'s preservation of an
empty supplied description. The VM, Cranelift and LLVM consume the same formatted text rather than
reconstructing punctuation independently.

The exit status remains 4. The source location, live backtrace, terminal control flow and skipped defers
are unchanged. Compile-time execution uses the same VM path, so a reached described todo becomes E0230
with the complete reason; an untaken one remains inert.

### 4. Tooling owns the complete statement

The formatter preserves all three forms and canonicalises the call-shaped variants without spaces before
the parentheses. Tree-sitter's `todo_stmt` accepts both optional-parenthesis forms, and the generated parser
is committed. Both editors continue to highlight `todo` as a keyword and now classify the description as a
string. The LSP parser, formatter and semantic-token paths therefore accept the same source the compiler
does; completion may continue to offer the `todo` keyword without inventing a placeholder description.

## Consequences

- Unfinished paths can explain themselves without becoming expressions or ordinary calls.
- Existing `todo;` source remains valid.
- The message is static, deterministic and byte-identical in all three engines.
- `Unreachable` is no longer `Copy`; exhaustive compile errors identify every place that must preserve or
  inspect the description.
- Parser, formatter, HIR, MIR, VM, Cranelift, LLVM, tree-sitter and LSP coverage all change.
- All six ordinary gates and gate 7 are required.
