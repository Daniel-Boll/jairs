# ADR-0218: E0258 offers explicit missing-case arms, never a catch-all

- **Status:** Accepted
- **Date:** 2026-09-09
- **Deciders:** dboll

## Context

The compiler already proves exhaustiveness for an enum or tagged-variant `switch`.
E0258 points at the whole construct and carries the declaration-ordered uncovered
alternatives:

```text
this `switch` does not handle every member of `Colour`
note: missing: `GREEN`, `BLUE`
help: add a `case` for each, or an `else` arm
```

The language server exposes code actions but offers nothing for that diagnostic.
A reader therefore has the exact repair in the diagnostic and must still type its
mechanical spelling by hand. The same diagnostic and HIR statement serve ordinary
`switch` and Jai's `if #complete value == { ... }` spelling.

Three repairs were considered:

1. insert every missing explicit arm;
2. insert `else;`;
3. offer both.

The decider chose the first. A catch-all makes the current program total but hides a
future enum member, which discards the maintenance benefit exhaustiveness checking
exists to provide.

## Decision

### §1. E0258 gets one preferred quick fix: `add all missing cases`

The action inserts one empty bare-member arm for every name in E0258's `missing:`
note, in the order the compiler supplied:

```jairs
case .GREEN;
case .BLUE;
```

An empty arm is the smallest valid statement of intent in Jairs: the value is handled
and deliberately does nothing. The action does not invent a trap, return value or
other behaviour it cannot infer.

There is no `else` action. It would be shorter today and weaker after the enum grows.
Offering both would make the weaker repair the tempting neighbouring choice.

### §2. The compiler remains the only source of the missing set

`jr-lsp` does not resolve the scrutinee or enumerate enum members. It reads the names
from E0258, preserving ADR-0031's rule that editor actions consume compiler analysis
rather than become a second front end.

Enum aliases need no editor rule. Exhaustiveness already groups members by runtime
value and lists one declaration-ordered representative for each uncovered value
class (ADR-0215 §3), so the action inserts exactly that list.

Tagged variants receive the same action. ADR-0068 deliberately reuses E0258 for their
finite case set, their case spelling is the same bare `.name`, and distinguishing the
two in the editor would add a difference semantics does not have.

### §3. A stale client diagnostic cannot edit the current buffer

The request carries the diagnostics the client currently displays, which may belong
to an older revision. Before constructing an edit, the action finds a current E0258
with the same range and requires its missing-name list to equal the client's list.

If either differs, no action is returned. Diagnostics will refresh and offer the
current repair. Applying a plausible but obsolete case list is worse than briefly
showing no lightbulb.

The current HIR must also contain a `Stmt::Switch` with that diagnostic span, and the
span must end at a real `}` byte. These checks keep an insertion from targeting a
recovered or shifted construct.

### §4. The edit inserts before the closing brace and preserves local style

The compiler-produced switch span supplies the insertion boundary. On an ordinary
multiline construct, the edit inserts complete arm lines at the start of the closing
brace's line. On a one-line construct, it first breaks before the new arms and writes
the closer at the switch's indentation.

An existing arm's indentation wins when its header begins a line. An empty or
single-line construct uses the project's formatter configuration for one additional
indentation level. The action does not run the formatter or reprint the construct;
it changes only the whitespace immediately before the closer and the new arm lines.

Both UTF-8 and UTF-16 clients receive positions through the existing `Positions`
converter. The edit is computed from byte spans first and converted once, so a
non-ASCII line before the switch cannot shift it.

## Rejected alternatives

- **Insert `else;`.** It hides later enum growth and does not enumerate the cases the
  diagnostic already knows.
- **Offer explicit cases and `else`.** More choice is not useful when one choice
  weakens future exhaustiveness.
- **Recompute enum members in `jr-lsp`.** That duplicates semantic resolution,
  imported-type handling and alias value classes in the editor.
- **Parse names from source arms and subtract them.** Computed cases and aliases make
  that textual set different from the runtime coverage set.
- **Run the formatter over the whole construct.** A quick fix should not reflow
  unrelated source, and `jr-fmt` remains the one owner of canonical layout.

## Consequences

An E0258 lightbulb repairs ordinary `switch`, `if #complete`, imported enum types and
tagged variants with one explicit edit. Alias handling remains byte-for-byte the
compiler's judgement.

This is an LSP-only wave. It changes no language syntax, semantic rule, MIR, pool
layout, runtime or back end, so it adds no corpus file and gate 7 is not required.
