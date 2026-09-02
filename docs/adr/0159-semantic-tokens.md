# ADR-0159: Semantic tokens — and W9's DWARF item was mis-estimated, with evidence

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **PLAN §8.4 — W9 Tooling depth.** Delivers its first item and **re-scopes its second**, which the section
  described from a false premise.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

W9 has three items. **Neovim packaging** was already delivered — the runtime directory works and §8.4's own
row says so, with VS Code declined by ADR-0036. **Semantic tokens** were "the one LSP capability absent", and
are delivered here. **Richer DWARF** was described as "line tables exist; locals and layouts do not", and that
is where this wave stopped to check.

It is false. `jr build` produces a binary with **no DWARF whatsoever**: `dwarfdump --debug-line` reports an
empty section, `otool -l` finds no `__DWARF` segment, no crate depends on `gimli`, and nothing in either back
end sets a source location on an instruction. The README's capability table said "**Not started** — no DWARF
at all; a native binary is not debuggable", which was right; §8.4's table row was written from the wrong one.

That changes the item's size, not its value, and this ADR says so rather than delivering a fraction of a
mis-described item and calling W9 done.

## Decision

### 1. Semantic tokens ship, as the fourteenth and last LSP capability

`textDocument/semanticTokens/full`, with sixteen token types and two modifiers.

**It is the only capability whose whole value is information the parser does not have.** ADR-0025's
tree-sitter grammar and highlight queries are genuinely good — fast, incremental, correct for everything
decidable from *shape* — and they cannot tell one identifier from another. `Point` and `count` are both
`IDENT` to a grammar, and so are a parameter, a local, a field, a procedure and a module. Every other
capability this server offers is a *lookup at a position*; this is a **classification of every token in the
file**, which is why it is last and why it is worth having beside a grammar that already colours the file.

### 2. Context leads, resolution follows

Each token is classified by its **syntactic context** — parent node kind and position — and only a bare
`NAME_EXPR` is resolved. Three reasons, in the order they mattered.

**A declaration's own name is not a reference**, so no resolution answers it: `Point :: struct { … }` has
nothing to look up, and the parent node says "a type is being declared" directly. **Context survives a file
that does not parse cleanly**, which is the state an editor spends most of its time in — resolution needs a
HIR, and a HIR needs a tree without holes in the interesting places; a corpus-style test pins this by asking
for tokens in a file with `return p.` in it and requiring the struct and the type annotation to still
classify. And context is *cheap*: one walk, no queries.

Resolution is asked only where context genuinely cannot decide — `count` could be a local, a parameter, a
file constant or a procedure.

**The `CONST_DECL` kind comes from the declaration's value**, not from a name convention: `Point :: struct {}`
is a struct, `f :: () {}` a function, `N :: 4` a constant. This language has no naming convention, and
inventing one would be a guess that looks like knowledge.

`item_kind` matches **exhaustively** over `ItemKind` and `ConstValue`, so adding a declaration form is a
compile error here — the house rule that would have caught `variant` being coloured as an ordinary constant
when ADR-0068 added it.

### 3. Sixteen types, two modifiers

A type earns its place by being **distinguishable by this compiler** and **useful to a reader**. Anything else
is a legend entry that never appears, costing a client a lookup table for nothing.

Absent: `class` and `interface` (this language has neither), `event` and `regexp` (likewise), and
`modifier`/`static`/`abstract` (declarations this language does not have). `typeParameter` **is**
distinguishable and is reported as `type` anyway, because a reader wants `$T` to look like the type it stands
for rather than like a fourth colour.

Two modifiers, because two are decidable and useful: `declaration`, and `readonly` — which is exactly what
`::` means, and the distinction this language cares about most, since `a :: 1` and `a := 1` differ in nothing
else.

Punctuation is deliberately **unclassified**. A client colours it from the grammar already, and reporting `,`
as an operator would fight the editor's own theme for no information gained.

### 4. Full document only: no range, no delta

A range request would save work on a large file and a delta would save bandwidth on an edit. Both are
optimisations, and both need the server to hold **per-document state it does not otherwise keep** — a delta
needs the previous response, keyed by a result id, invalidated correctly on every edit.

A file this compiler parses in microseconds needs neither yet, and `full: true` with `range: false` is the
honest advertisement: a client that wants a range asks for the whole file, which is correct rather than merely
tolerable. The Neovim verifier asserts both flags, so a later change to either is a deliberate one.

### 5. The encoding is written out, and the tokens are sorted first

The protocol wants five integers per token, delta-encoded against the previous one: line delta, then start
delta *within a line* but **absolute** start on a new line, then length, type, modifiers. The fiddly part is
that the second number changes meaning depending on the first.

Two things guard it. The tokens are **sorted by offset before encoding**, because one out-of-order token does
not misplace itself — it misplaces every token after it, corrupting the whole file's colouring from one
entry. The walk is already in source order for a well-formed tree, so the sort is a cheap guarantee rather
than a fix for a known bug, which is exactly when to add one.

And the **length is computed from two positions**, not from the byte range: under UTF-16 a non-ASCII token's
byte length would overrun into the next token. The tests decode the stream back into
`(line, character, length, type, modifiers)` tuples rather than asserting on raw integers, because a raw
assertion is unreadable *and* would not notice the corruption a misplaced token causes.

A token spanning a line break is **dropped**: the protocol has no encoding for one, and a truncated length
would colour into the next line.

### 6. `jr-lsp` gains a dependency on `jr-syntax`, deliberately

Thirteen capabilities work from the HIR and its spans, which is why this crate had no syntax dependency. A
token classifier cannot: its job is to say what **every** token is, including the ones the HIR never sees —
punctuation, keywords, comments, and the name of a declaration that failed to lower. The CST is the only
artefact that has all of them, in order, with offsets.

Recorded because a new dependency on a crate this one deliberately avoided deserves a stated reason.

### 7. DWARF is its own wave, and §8.4 is corrected

**There is no DWARF at all.** Probed: an empty `.debug_line`, no `__DWARF` segment, no `gimli` consumer, no
source location set on any instruction. So the item is not "locals and layouts on top of existing line
tables"; it is a **from-scratch DWARF writer**, and its parts are:

- a `gimli::write` unit emitting `.debug_abbrev`, `.debug_info`, `.debug_line` and `.debug_str`, placed into
  `cranelift-object`'s product — and into `jr-codegen-llvm` separately, since the two back ends share no
  emission path;
- a line program built from `MirSpan`, which is the one part that is genuinely ready: the Cranelift back end
  already tracks a current span per statement for trap locations (ADR-0020), so the information exists and
  only needs a second consumer;
- type DIEs from the pool, which is also tractable — a struct layout is static, so `DW_TAG_structure_type`
  with `DW_AT_data_member_location` needs nothing the compiler does not already compute;
- **locals**, which are the real work. A local's location is a frame offset or a register, and it varies by
  code offset. Cranelift reports this through `CompiledCode::value_labels_ranges`, which is populated only
  for values the producer *labelled* — and this back end labels none. So locals need `ValueLabel`s attached
  during lowering, the value-label tracking flag enabled, and a DWARF location list per range;
- and a Mach-O question: whether to emit `__DWARF` in the object and let `dsymutil` build a bundle, or to
  keep it in the executable, which `jr-link` has no opinion about yet.

That is comparable to W9's whole original estimate, so it becomes **W12 — Debug info**, named in §2.1 rather
than left as an item that would be quietly dropped or quietly become a quarter of work. That is exactly what
§8.3 did to `Thread` for the same reason, and the precedent is the argument.

**Rejected: delivering the line table alone and calling W9 done.** It is the highest-value third of the item
and would leave §8.4's row half-true in the other direction, which is how the row got wrong in the first
place. **Rejected: leaving §8.4 as written.** A plan whose premise is false is worse than one that admits a
gap, and this project has an audit habit precisely because that has happened before.

## Consequences

- **`crates/jr-lsp/src/tokens.rs`** is new, wired through `capabilities()` and the worker. `jr-lsp` depends
  on `jr-syntax`.
- **Five new tests** in `jr-lsp`'s handler suite: the identifiers a grammar cannot separate, a field access's
  receiver against its field, trivia and literals, monotonic positions, and a file that does not parse.
- **Four new Neovim checks** — 170 total, up from 166 — asserting the capability, both legend halves, and the
  deliberate `full`/`range` choice.
- **W9 — Tooling depth is DONE as re-scoped**: semantic tokens delivered, Neovim packaging already there, VS
  Code declined by ADR-0036, DWARF split out to W12.
- **§8.4 and the README are corrected.** The README's own capability table was right all along, which is the
  argument for keeping it.
