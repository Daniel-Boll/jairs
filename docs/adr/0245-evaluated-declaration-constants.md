# ADR-0245: Declaration-shaping constants use a VM-backed pre-signature pass

- **Status:** Accepted
- **Date:** 2026-09-11
- **Deciders:** dboll
- **Amends:** ADR-0070 §2, ADR-0129 §4, and ADR-0144 §2. Their literal-or-one-name
  restriction remains the fallback when no evaluated value exists, but it is no longer the
  language boundary.
- **Programme:** first of seven language-priority waves: evaluated declaration constants,
  sequence-place ergonomics, pure templates in `#run`, general procedure overloading,
  cross-file procedure values, first-class `Code`, and multi-result forwarding.

## Context

Five declaration-time consumers ask the same question and currently receive the same deliberately
narrow answer:

- `[N]T` and `#simd [N]T`;
- an enum member's explicit value;
- `#align` and `#place`;
- `#soa(N)`.

They accept an integer literal or a name whose initializer is immediately one. Arithmetic, alias
chains, calls, `#run`, and imported constants are refused because signatures run before
`jr-db::file_consts`, while that evaluator needs signatures before it can lower MIR. ADR-0070
correctly rejected putting a second folder in `jr-sema`; it did not provide the missing phase seam.

The HIR also discards an array or vector count once it has decided that the syntax is neither a
literal nor a bare name. The parser accepts `[WIDTH * HEIGHT]T`, but no later phase retains the
expression that would have to be evaluated.

Real programs need this for grids, lookup tables, SIMD aliases, and generated layouts. The game
source audit ranks evaluated lengths P0, and the same phase-order limit is duplicated across all
five consumers. Fixing only array syntax would leave four definitions of “usable declaration
constant” behind.

## Decision

### 1. A pre-signature query evaluates declaration constants through MIR and the bytecode VM

`jr-db` gains a private provisional-signature query and a declaration-value query:

1. provisional signatures type declaration operands but use marked placeholder shapes when an
   evaluated value is not available;
2. declaration-value evaluation lowers the retained expressions through `jr-mir::lower_const` and
   executes them in the compile-time VM;
3. final signatures run the same `jr-sema` entry point again with the evaluated integer map.

The evaluator is therefore not a syntax fold. Overflow, wrapping arithmetic, shifts, division, and
every future integer operation keep the same MIR and VM semantics as ordinary compile-time code.
`jr-sema` receives only evaluated integers through a small `DeclarationValues` interface; it never
depends on `jr-db`, `jr-mir`, or `jr-vm`.

The provisional and final passes are two configurations of the same signature machine. There is no
second typer and no second implementation of declaration rules.

### 2. This wave establishes the seam with local expression graphs

The first slice evaluates:

- integer literals and unary/binary integer expressions;
- chains of local file constants;
- direct declaration operands and names that resolve through those constants.

It applies uniformly to arrays, vectors, enum members, `#align`, `#place`, and `#soa`.
Expressions whose value depends on a per-instantiation `$N`, or declarations synthesized by a
computed `#insert`, remain outside this source-file prepass; the existing bare `$N` path is
unchanged.

Calls and imported values are deliberately not approximated. A call needs reachable procedure
routines in the provisional program, while an imported value needs a root- or component-scoped
fixpoint so legal import cycles do not become salsa query cycles. Both extend the same
declaration-value query in follow-up sub-waves:

1. imported constant graphs over the reachable module component;
2. callable `#run`/procedure expressions with cycle reporting and one-time output.

This staging changes capability, not semantics: an expression either reaches the ordinary MIR/VM
evaluator or remains the existing diagnostic.

### 3. HIR retains the count expression

Array and vector type references carry the lowered expression id in addition to its source span.
The expression is allocated in the same top-level or body arena as the type reference, and
`ExprScope` remains the authority that distinguishes those arenas.

Literal and bare-name fast paths remain useful:

- they preserve `$N` instantiation behaviour, whose value is supplied after the ordinary
  declaration prepass;
- they keep recovery cheap when a file already has errors;
- they provide the same result when the declaration-value query has no entry.

### 4. Pending provisional shapes are explicit placeholders

The provisional pass may need to type a declaration before its count exists. It interns the same
zero-length placeholder shape used for an uninstantiated `$N`, records it as pending, and withholds
length-dependent diagnostics. Final signatures never expose a pending placeholder: they either
resolve the evaluated value or emit the consumer's existing diagnostic and return poison.

A pending alignment, placement, enum value, or SoA count likewise uses no fabricated semantic
answer. It is omitted or auto-numbered only inside the private provisional pass, then resolved or
diagnosed in the final pass.

## Rejected alternatives

### Fold integer syntax inside `jr-sema`

Rejected because it creates the second evaluator ADR-0070 explicitly refused. Sharing low-level
arithmetic helpers would reduce duplicated arithmetic code but would still duplicate expression
order, name traversal, traps, calls, and future language operations.

### Support only `literal op literal`

Rejected because it creates a new arbitrary boundary immediately below the real use:
`WIDTH * HEIGHT` is usually two names, and aliases are ordinary constant structure. The dependency
graph already needs a fixpoint, so literals-only buys little.

### Make final signatures directly depend on `file_consts`

Rejected because `file_consts` already depends on checked signatures. That is a query cycle, not an
implementation inconvenience. The private provisional pass is the adapter that breaks it.

### Put placeholder values in the public signature result

Rejected because a real-looking `[0]T`, offset 0, or enum ordinal is a silent wrong answer. Pending
state is private and marked; final consumers receive a value or poison.

## Consequences

- One VM-backed seam answers every declaration-shaping integer question.
- `[WIDTH * HEIGHT]T`, local alias chains, computed enum values, and computed layout/SoA operands
  become possible without changing MIR or either native back end.
- Imported constants and callable `#run` remain visible follow-up sub-waves, not undocumented
  exceptions.
- The formatter and tree-sitter need no grammar change: both already accept an expression in these
  positions.
- Gate 7 is not required for the first slice because no MIR variant, layout representation, pool
  layout, or back end changes. The ordinary differential corpus still proves the resulting shapes
  execute identically in the VM and Cranelift.
