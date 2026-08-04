# ADR-0091: A `#expand` macro call splices the macro's body into the caller's scope

- **Status:** Accepted
- **Date:** 2026-08-04
- **Deciders:** dboll
- **W5 sub-wave 7b.** ADR-0090 delivered the `#expand` surface and refused a call (E0272). This makes a call
  *run*: the macro's body is spliced into the caller, so it sees the caller's locals. E0272 is repurposed for
  the one case still refused — a **cross-file** macro call.

## Context

ADR-0090 §2 settled the mechanism ahead of the build: reuse the `Stmt::Insert` splice, deliberately
unhygienic. What the build had to settle is *where* the splice is driven from and what a macro's `return`
means.

## Decision

### 1. The macro's text reaches the call through a pre-scanned map, and the body is not lowered standalone

`lower_file_with_inserts` collects every `#expand` macro's `(parameter names, body inner text, has return
type)` before any body is lowered, and threads it to each `BodyLowerCtx` exactly as `InsertOperands` is —
the same proven shape rather than a new one. A call may precede the declaration in source order, which is
why it is a pre-scan.

**A macro's own body is deliberately not lowered.** It exists only to be spliced, and lowering it standalone
resolves its names against the macro's own scope — so a macro that reads the caller's locals, which is the
entire point, reported them as unresolved. It therefore emits no MIR, and **`declarations()` skips it** the
way it skips a template: leaving it declared gave the linker `function "jr$0$0" with linkage Local must be
defined but is not`, caught by the corpus differential on this wave's own file.

### 2. A call generates text: a prelude, then the body, then (for a value) a result local

A call is lowered by building source text and handing it to `expand_insert_text`:

```
x := <argument text>;        // the prelude, one line per parameter
<body, with `return <e>;` rewritten>
```

**The prelude is why each argument is evaluated once.** Substituting the argument's text at every use of the
parameter would re-evaluate a side-effecting argument per use — a wrong answer, not a slow one. The MIR
snapshot shows it: `double(1 + 2)` lowers to `1 + 2` once, then `* 2`.

For **expression** position a result local is generated, so one mechanism serves both:

```
__macro_0 := 0;  x := 21;  __macro_0 = x * 2;  exit(__macro_0);
```

A `return <e>;` in **tail position** becomes `__macro_N = <e>;`. That is the weaker, well-defined meaning a
spliced `return` can have, since a macro has no frame to leave.

### 3. What is refused, each by design rather than left to misbehave

- **An early `return`** — one that is not the macro's last statement — is **E0273**, raised in *lowering*
  because that is where the splice is built and where "the rewrite would fall through" is knowable. Returning
  from the *caller* is Jai's real semantics and strictly more powerful, but it changes what `return` means by
  provenance and interacts with `defer` (ADR-0049 §3). Rewriting it to an assignment would silently fall
  through to the statements after it, so it is refused.
- **A void macro in expression position** — no return type, so no value — is E0273 with its own message.
- **A cross-file macro call** is **E0272**, repurposed. The macro-text map is per file, so an imported macro
  is not spliced; before this refusal the call reached the VM as `internal compiler error: no routine for
  file 1 proc 0`, the **fifth** time compiler internals have leaked for a reasonable program. The fact that a
  name is a macro is carried on `FileSignatures` (`is_macro`), because an importer has another file's
  signatures and not its HIR.

### 4. The token-set trap, for the fifth time

`looks_like_proc_signature` decides whether `(…)` begins a procedure by looking at the token after the
matching `)`. A **void** macro — `f :: (x: s64) #expand { … }`, with no `->` — reached neither `ARROW` nor
`L_BRACE`, so it was read as a parenthesised-expression constant and produced fourteen cascading errors.
`#expand` joined that list, whose own comment already warned this had happened for `#c_call` and for
`TYPE_START`.

## Consequences

- **Macros work in both positions**, verified by value: `valid/075` exits 96, and the MIR snapshot shows
  **no calls at all** — every body inlined into `main`. The void macro modifying the caller's `total` is the
  part no ordinary call could reproduce.
- **One macro call per statement** this wave. A second in the same statement would need its own result local
  threaded through the same rewrite; it falls to the ordinary path, which refuses it, rather than expanding
  only half.
- **E0273 is a `jr-hir` code**, continuing that crate's block (E0262–E0264 are `#insert`'s), because it is
  raised in lowering.
- Still owed: the caller-return semantics for an early `return`, a cross-file splice, and `#modify` /
  `#bake_arguments` — each its own decision.
