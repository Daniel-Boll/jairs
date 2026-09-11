# ADR-0246: Extraction is a semantic source transformation behind one editor seam

- **Status:** Accepted
- **Date:** 2026-09-11
- **Deciders:** dboll
- **Programme:** editor fidelity: value/type semantic highlighting, highlighted hover signatures,
  and selection-based `refactor.extract` actions.

## Context

Four editor behaviours disagree with the language the compiler already understands:

- tree-sitter captures `true` and `false` as booleans, but the LSP reports every keyword through
  one semantic-token kind and overrides them as keywords;
- tree-sitter knows the operand of `Point.{...}`, `string.{...}`, or `Box(T).{...}` denotes a
  type, while semantic tokens classify the same expression as an ordinary value;
- hover cards use a fenced language named `jr`, although the editor language, parser, and LSP
  language id are `jairs`; the fence also contains a bare container line before an incomplete
  declaration, so the procedure name parses as recovery rather than as a function;
- code actions repair diagnostics and rewrite comments, but expose no selection-based extraction.

Expression extraction is mostly a scheduling question. Procedure extraction is not a text move:
the selected region may read or mutate caller bindings, declare locals used afterward, take
addresses, return from the enclosing procedure, or break/continue an enclosing loop. A refactor
that guesses at any one of those can produce valid source with different behaviour.

The LSP currently owns protocol conversion and thin source edits. Putting capture analysis,
liveness, control translation, type spelling, naming, and edit construction directly in
`jr-lsp::actions` would make that module a second compiler frontend and leave no coherent test seam.

## Decision

### 1. `jr-refactor` is one deep, protocol-neutral module

The workspace gains an in-process `jr-refactor` crate. Its external interface accepts a database
snapshot, source file, module catalog, byte selection, and requested extraction kinds, and returns
zero or more titled source changes:

```rust
pub fn extract(
    db: &dyn Db,
    file: SourceFile,
    catalog: ModuleCatalog,
    request: ExtractRequest,
) -> ExtractionReport;
```

A source change is a sorted set of non-overlapping byte edits against that exact source snapshot.
Ordinary inapplicability is an exhaustive refusal in the report, not a protocol error. `jr-lsp`
alone converts positions and changes to LSP ranges, filters `CodeActionContext.only`, advertises
`refactor.extract`, and attaches the immediate `WorkspaceEdit`.

The implementation keeps a private source-oriented analysis IR and transformation plan. CST/HIR
selection correlation, use/definition analysis, outward control edges, writable type spelling,
generated names, copied source fragments, and final edits remain behind the seam. MIR is rejected
as the analysis source: it has useful CFG facts but has already lost declaration and trivia identity,
and may not exist for the source an editor is changing.

No ports or renderer trait are introduced. Every dependency is in-process and there is one real
consumer adapter, the LSP. A fake renderer maintained only for tests would be a hypothetical seam.

### 2. A selected expression is extracted only under a proved evaluation schedule

The local-variable action requires an exact non-empty CST/HIR expression selection and an insertion
point in the same braced block. The first implementation supports schedules where moving the
expression immediately before its containing statement preserves evaluation count and order,
including a complete initializer, assignment RHS, return value, `if` condition, switch scrutinee,
or explicit discard.

Nested short-circuit operands, loop conditions, and expressions preceded by an effectful sibling
are refused until the language has an expression/block form or the refactor can linearise every
preceding evaluation without changing scope. Refusal is preferable to a plausible edit with a
different execution count.

The generated local name is deterministic and collision-free (`extracted`, then a numeric suffix).
The selected source is copied verbatim; the refactor does not become a second formatter.

### 3. Captures cross the extracted-procedure seam by storage identity

The procedure action requires consecutive complete statements from one block. A selected region is
analysed by resolved binding identity, never by textual name.

Every external local or parameter used by the region is passed as a pointer to its caller storage.
The copied body rewrites reads, writes, and address-taking through that pointer. This preserves:

- mutations without synthetic copy-out assignments;
- aliases between captures;
- the identity observed by an address taken inside the region;
- values observed by caller-owned defers after an outward transfer.

File items and imported declarations are not captures. The implicit `context` keeps travelling
through the language's existing hidden context parameter. A promoted `using` name captures its
base storage and rewrites the promoted field path through that pointer.

Generic `$T` pointer parameters describe external capture types, so ordinary captures do not need
display text turned back into source.

### 4. Escaping declarations and return payloads use caller-owned slots

A local declared inside the region and referenced afterward is allocated in the caller before the
helper call. Its exact writable type is rendered from sema/pool identity and visible imports. The
helper receives its storage by pointer; the original declaration becomes initialization of that
slot at the same point in the copied body.

The same mechanism carries values from an enclosing-procedure `return`. Caller-owned slots use the
outer procedure's declared result types; the copied return writes them, then returns a private flow
code. This avoids a multi-result protocol containing meaningless values on fallthrough paths.

Writable type rendering is stricter than hover rendering: a spelling must resolve in the insertion
file to the original `PoolId`. It handles builtins and structural types, local nominal names,
qualified import aliases, parameterised nominal types, and procedure calling conventions. An
unspellable/private/poisoned type refuses the action.

### 5. Outward control flow is translated explicitly

Control whose target is wholly inside the selected region remains unchanged. An edge targeting the
caller becomes a private integer flow code:

- return from the enclosing procedure, including multiple values;
- break to an enclosing selected-outside loop;
- continue to an enclosing selected-outside loop.

The helper returns the code after committing mutations and payload slots. The call site dispatches
it to the original `return`, `break`, or `continue`, so active caller defers still run in their
original scope and order. Several distinct labelled loop targets receive distinct codes.

`todo` remains terminal inside the helper. Generated or expansion-only source with no stable
editable range is refused.

A defer wholly contained in a moved nested block keeps its lifetime. A defer registered directly
in the selected enclosing block may move only when the selection covers that block's remaining
tail; otherwise the action is refused because the language has no operation that registers a
caller-scope defer from a callee.

### 6. Semantic tokens and hover use the language's editor identity

The semantic-token legend appends a boolean kind; existing wire indices are never reordered.
`true` and `false` use it, while `null` remains a keyword-like literal under its existing policy.

Identifiers in the explicit type operand of a struct literal are reported as types before ordinary
expression resolution. Initializer expressions remain values. This covers local, imported,
qualified, parameterised, builtin, and incomplete explicit type spellings without changing the
grammar.

Hover puts the human container label outside a `jairs` fence and puts only Jairs source inside it.
The tree-sitter query recognises a bodyless procedure-signature fragment produced for hover, so its
function, parameters, punctuation, and types receive their ordinary captures. Completion resolve
continues to share the same card renderer.

## Rejected alternatives

### Keep extraction inside `jr-lsp::actions`

Rejected because the protocol adapter would own compiler analysis, source scheduling, liveness,
control translation, and type spelling. Deleting that module would spread the same rules across
tests and future editor consumers, which is the definition of a shallow interface.

### Expose a public staged analysis/plan/render pipeline

Rejected because callers would need phase ordering, plan invariants, and recovery rules. A private
IR is useful; making it the interface couples every test and caller to implementation structure.

### Build the extraction from MIR

Rejected because source edits need exact declarations, comments, trivia, and selection identity.
MIR has transformed those away, and generated bodies may not map one-to-one back to editable text.

### Copy captures by value and return mutations

Rejected because taking an address inside the helper would point at the copy, aliases could observe
different storage, and every outward control edge would need a complete synthetic copy-out payload.

### Approximate defer or outward control semantics

Rejected because these edits often compile. A silent semantic change is worse than an absent
lightbulb, so unrepresentable lifetime or control cases are explicit refusals.

## Consequences

- Editor highlighting agrees across tree-sitter and LSP semantic tokens.
- Hover code is parseable Jairs and receives the same query captures as a source buffer.
- `refactor.extract` can perform capture-aware procedure extraction rather than only zero-capture
  text movement.
- The LSP remains a protocol adapter over compiler-owned facts.
- Extraction costs one request-local linear body analysis plus selected-source rewriting; it is not
  a salsa query, so arbitrary selections do not grow the incremental cache.
- Interface tests apply edits, parse and type-check the result, and compare executable behaviour
  where the original program is runnable.
- Gate 7 is not required: no MIR variant, pool layout, runtime representation, or back end changes.
