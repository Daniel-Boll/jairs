// The Jairs status dashboard — the source the committed `jairs-dashboard.pdf` is built from.
//
// Regenerate with:
//
//     typst compile jairs-dashboard.typ
//
// # Why this file exists
//
// The first dashboard was generated from a Typst source that was never committed, so a PDF sat in
// the tree with no way to rebuild it — and it went nine waves stale (it still claimed 892 tests and
// ADR-0047 as the latest) because updating it meant rewriting it from nothing. A generated artefact
// without its generator is worse than no artefact: it looks current and cannot be checked.
//
// # The rule for every number here
//
// Measured, not carried forward. The test count comes from a full `cargo test --workspace` run, the
// corpus count from a file walk, the ADR count from `docs/adr/`, the diagnostic count from the
// `const E0nnn: &str` definitions across the crates, and the editor-check count from a real
// `verify.lua` run. PLAN.md §1.5 and §7 are the prose sources. A number that cannot be measured
// does not belong on a dashboard.

#set page(
  paper: "a4",
  margin: (x: 1.4cm, y: 1.2cm),
)
#set text(font: ("Helvetica Neue", "Helvetica", "Arial"), size: 8.2pt)
#set par(justify: false, leading: 0.55em)

#let ink = rgb("#1a1a1a")
#let muted = rgb("#6b6b6b")
#let rule = rgb("#d8d8d8")
#let good = rgb("#1f7a3d")
#let warn = rgb("#a8620f")
#let absent = rgb("#8a8a8a")
#let accent = rgb("#1b3f8f")

#set text(fill: ink)

// A section heading: small caps, a hairline under it.
#let section(title) = block(
  above: 0.9em,
  below: 0.55em,
  width: 100%,
)[
  #text(size: 9.5pt, weight: "bold", fill: accent)[#title]
  #v(-0.45em)
  #line(length: 100%, stroke: 0.5pt + rule)
]

// A subheading inside a column.
#let sub(title) = block(above: 0.7em, below: 0.35em)[
  #text(size: 7.2pt, weight: "bold", fill: muted, tracking: 0.06em)[#upper(title)]
]

// One headline metric.
#let metric(label, value, note) = block(width: 100%)[
  #text(size: 6.6pt, fill: muted, tracking: 0.08em)[#upper(label)]
  #v(-0.55em)
  #text(size: 19pt, weight: "bold", fill: accent)[#value]
  #v(-0.7em)
  #text(size: 6.6pt, fill: muted)[#note]
]

#let pill(body, fill: rgb("#eef2fb"), stroke: accent) = box(
  inset: (x: 4pt, y: 2pt),
  radius: 2pt,
  fill: fill,
  text(size: 6.8pt, weight: "bold", fill: stroke)[#body],
)

#let done = text(size: 6.6pt, weight: "bold", fill: good)[DONE]
#let open = text(size: 6.6pt, weight: "bold", fill: warn)[OPEN]

// ---------------------------------------------------------------------------
// Title
// ---------------------------------------------------------------------------

#grid(
  columns: (1fr, auto),
  align: (left + bottom, right + bottom),
  [
    #text(size: 21pt, weight: "bold")[Jairs]
  ],
  [
    #text(size: 8pt, fill: muted)[31 July 2026]
  ],
)
#v(-0.5em)
#text(size: 8.2pt, fill: muted)[
  A Jai-inspired systems language in Rust. Incremental salsa front end, lossless CST, typed SSA
  mid-end, two execution engines held byte-identical by a differential harness.
]

#v(0.4em)
#pill[6/6 gates green]
#h(4pt)
#pill[928 tests]
#h(4pt)
#pill[ADR-0068 latest]
#h(4pt)
#pill(fill: rgb("#e8f5ec"), stroke: good)[W4.5 complete · W4 comptime next]

#v(0.5em)
#grid(
  columns: (1fr, 1fr, 1fr, 1fr, 1fr),
  gutter: 8pt,
  metric("Tests", "928", "workspace, all passing"),
  metric("Corpus", "155", "jr files, both engines"),
  metric("ADRs", "68", "0001 to 0068, immutable"),
  metric("Diagnostics", "91", "codes, E0261 next free"),
  metric("Editor checks", "166", "Neovim, verified not gated"),
)

// ---------------------------------------------------------------------------
// The slice
// ---------------------------------------------------------------------------

#section[The vertical slice, and what it costs to be honest about it]

#grid(
  columns: (1fr, 1.35fr),
  gutter: 14pt,
  [
    #sub[Exit criteria]
    #set list(marker: none, indent: 0pt, body-indent: 4pt, spacing: 0.42em)
    - #done #h(3pt) hello.jr runs in the VM
    - #done #h(3pt) compiles to a native binary
    - #done #h(3pt) rustc-grade diagnostics
    - #done #h(3pt) formatter round-trips the corpus
    - #done #h(3pt) editor integration (Neovim)
    - #open #h(3pt) verified Linux x86-64 CI run

    #v(0.3em)
    #text(size: 7.2pt, fill: muted)[
      The last one needs a push. Configured, never run, so it is a decision rather than a technical
      gap.
    ]
  ],
  [
    #sub[How correctness is established]
    #set list(marker: none, indent: 0pt, body-indent: 0pt, spacing: 0.5em)
    - #text(size: 7.4pt)[
        *Both engines run every corpus program* and must agree, on output and on trap wording, from
        one shared formatter.
      ]
    - #text(size: 7.4pt)[
        *A plan's stated reason is checkable.* W4.5 was scheduled after W4 because exhaustiveness
        "wants comptime type info". Three greps and a two-line program showed it does not: the enum
        member set is already in the pool during checking. The wave moved forward and the table records
        the amendment — a wave order resting on a dependency that does not exist is a plan contradicting
        itself.
      ]
    - #text(size: 7.4pt)[
        *A new enum variant only asks the question; answering it is still manual.* `AggregateKind` and
        `TagCheck` each turned every match site into a compile error, which is what *found* the sites —
        but a compile error only asks "which group does this belong to", and this wave answered two of
        them wrongly. Both were silent: DCE deleted a variant's stores, and the slot-liveness collector
        panicked. Running found them; no verifier would have.
      ]
    - #text(size: 7.4pt, fill: warn)[
        *The formatter destroyed a declaration, again.* A two-way `if` whose `else` meant "struct" turned
        every `variant` into a `struct` — the exact mistake that function's own docs already warned about
        for `enum_flags`, made again one form later. Thirteenth wave in fifteen.
      ]
  ],
)

// ---------------------------------------------------------------------------
// The language
// ---------------------------------------------------------------------------

#section[The language today]

#let lang = (
  ("Full integer tower, bool, string, pointers", "pointer difference p - q, deferred"),
  ("float32 and float64, plain IEEE-754, no traps", "percent on floats, is_nan, math (W7)"),
  ("struct, nominal, one level", ""),
  ("union, nominal, untagged — a cross-field read reinterprets", ""),
  ("variant — a tagged union: a wrong-case read traps, switch destructures", "a recursive variant; eliding the check in an arm"),
  ("enum and enum_flags, namespaced, bare dot-member, switch cases", "an explicit backing type"),
  ("Fixed arrays and views, bounds-checked, returnable", "array literals, sub-slicing"),
  ("cast and xx from context; operator overloading", "unary, index and call overloading"),
  ("Trapping arithmetic, wrapping variants, bitwise", "transmute; float printing"),
  ("if, else, while, for, break, continue, defer, using", ""),
  ("switch with exhaustiveness checking over an enum; else", "patterns, ranges, guards; a jump table"),
  ("Multiple returns, named args, literal defaults", "#must; a multi-result call in a return"),
  ("import, foreign, system_library, #scope_module", "polymorphs, macros (W5)"),
  ("One trivial compile-time run, folded", "arbitrary run, RTTI, insert (W4)"),
  ("Traps name their source line and the live call chain", "a per-frame line; inlined frames (none exist)"),
  ("Bounds checks, strippable by build setting or #no_abc", "a per-index #no_abc; any other build setting"),
  ("context, a hidden parameter passed by pointer", ""),
  ("Procedures as values: call, pass, return, struct field", "a cross-file or foreign proc value; comparing one"),
  ("null, a context-typed pointer literal; malloc and free", "cast to a pointer"),
  ("An allocator in the context, installed and called through", ""),
  ("Pointer offset p + n, n + p, p - n — element-scaled, unchecked", "the difference p - q; p[n]; ordering"),
  ("push_context, a block with its own copy of the context", ""),
  ("talloc / reset_temporary_storage — a per-context bump arena", "hands out *u8 only; alignment; a growable region"),
)

#table(
  columns: (1fr, 1fr),
  stroke: none,
  inset: (x: 0pt, y: 1.6pt),
  align: left,
  table.header(
    text(size: 6.6pt, weight: "bold", fill: muted, tracking: 0.08em)[WORKS, END TO END],
    text(size: 6.6pt, weight: "bold", fill: muted, tracking: 0.08em)[ABSENT (WAVE)],
  ),
  ..lang.map(((w, a)) => (
    text(size: 7.2pt)[#w],
    text(size: 7.2pt, fill: absent)[#a],
  )).flatten()
)

#v(0.35em)
#text(size: 7pt, fill: muted)[
  *No error-handling model yet* — ADR-0008 reserves the slot and the first half exists (several
  return values); `#must` is owed its own ADR. *No GC and no RAII*, which is a design value rather
  than a gap. *A union is untagged*, so reading a field other than the one last written reinterprets
  bits: documented in three places because it cannot be diagnosed. *No indirect calls*, which is the
  single largest gap in the project and is what blocks the rest of W3.
]

#pagebreak()

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

#section[Compiler internals, 17 crates]

#let stages = (
  ("Lexer, parser, CST, typed AST", "works", "Hand-written, error-recovering, trivia-preserving"),
  ("Formatter", "works", "Pure function over the CST; lost source in 7 of the last 8 waves"),
  ("HIR, name resolution, modules", "works", "Flat import merge; cycles legal; export filtering"),
  ("InternPool: types, values, layout", "works", "One layout computation and one integer evaluator, shared"),
  ("Sema: signatures, checking", "works", "E0212 to E0257; no const-eval here, by design"),
  ("MIR: typed SSA", "works", "Block parameters, not phis; explicit bounds check and zeroing"),
  ("Mid-end", "5 passes", "Inline, forwarding, const-prop, DCE, plus the bounds-check strip"),
  ("Const-eval", "works", "Runs MIR through the bytecode VM"),
  ("VM: register bytecode, libffi", "works", "No JIT; indirect calls, and malloc from its own region"),
  ("Cranelift back end", "works", "Aggregate returns via sret; indirect calls via func_addr"),
  ("LLVM back end", "not started", "W8 owns it"),
  ("Language server", "12 caps", "Diagnostics, hover, goto, completion, rename, actions, hints"),
  ("Neovim integration", "works", "Runtimepath dir, no plugin manager; 166 checks"),
  ("Driver", "stub", "Should consume the workspace notion that now exists"),
)

#table(
  columns: (auto, auto, 1fr),
  stroke: (x, y) => if y == 0 { (bottom: 0.5pt + rule) } else { none },
  inset: (x: 3pt, y: 1.8pt),
  align: left,
  table.header(
    text(size: 6.6pt, weight: "bold", fill: muted, tracking: 0.08em)[STAGE],
    text(size: 6.6pt, weight: "bold", fill: muted, tracking: 0.08em)[STATE],
    text(size: 6.6pt, weight: "bold", fill: muted, tracking: 0.08em)[NOTE],
  ),
  ..stages.map(((name, state, note)) => (
    text(size: 7.2pt)[#name],
    text(
      size: 7pt,
      fill: if state == "not started" or state == "stub" { warn } else { good },
    )[#state],
    text(size: 7.2pt, fill: muted)[#note],
  )).flatten()
)

// ---------------------------------------------------------------------------
// Roadmap
// ---------------------------------------------------------------------------

#section[Roadmap: W1 and W2 closed, W3 started]

#let waves = (
  (
    "W1 Data", "done",
    "Numeric tower, wrapping ops, enum, enum_flags, union, arrays, views, cast, xx, operator overloading. Closed by ADR-0048.",
  ),
  (
    "W2 Flow and scope", "done",
    "for with it and it_index, labelled break, defer, using, multiple returns, named and default arguments, #scope_module. Closed by ADR-0054; ADR-0055 then closed a gap six corpus files had carried for eleven waves.",
  ),
  (
    "W3 Runtime core", "done",
    "context (ADR-0057), the bounds-check build setting (ADR-0058, finishing ADR-0003), indirect calls (ADR-0059), null plus a memory source (ADR-0060/0061), the allocator protocol (ADR-0062), push_context (ADR-0063), pointer arithmetic (ADR-0064), temporary storage (ADR-0065), and traps with backtraces (ADR-0066) — a trap names the frames that were live, byte-identically in both engines. Closed by ADR-0066; a source-level backtrace with inlined frames is deferred, because inlined frames have no runtime existence.",
  ),
  (
    "W4 Comptime", "not started",
    "Arbitrary compile-time execution, RTTI and Type values, insert, code. PLAN section 5 names this the project's top risk: sema and comptime become mutually recursive.",
  ),
  (
    "W4.5 Pattern matching", "done",
    "switch with exhaustiveness checking, a bare dot-member as a case (settling ADR-0041 §2 step 5), and a tagged variant type (ADR-0067, ADR-0068). Reordered ahead of W4 after checking showed its stated dependency on comptime was a want rather than a need. The variant follows ADR-0045 §1's own instruction — a different declaration form, not a change to union — and union is untouched, still untagged and still one word smaller.",
  ),
  (
    "W5 Polymorphism", "not started",
    "Polymorphic parameters, modify, bake_arguments, expand macros with hygiene, instantiation caching.",
  ),
  (
    "W6 Metaprogram", "not started",
    "Workspaces, the compiler message loop, build scripts replacing makefiles, note attributes.",
  ),
  (
    "W7 Stdlib", "not started",
    "Written in Jairs: String, dynamic array, hash table, Sort, Math, Random, File.",
  ),
  (
    "W8 Performance", "not started",
    "LLVM via inkwell for release builds, inliner maturity, struct-of-arrays, SIMD, parallel sema. Also owns the compile-throughput number, still unpublished.",
  ),
  (
    "W9 Tooling depth", "not started",
    "Semantic tokens are the one LSP capability still missing; the other twelve landed early.",
  ),
  (
    "W10 Graphics, in Jairs", "not started",
    "Window creation through foreign calls, Metal then Vulkan, immediate-mode 2D.",
  ),
)

#for (name, state, note) in waves {
  block(above: 0.42em, below: 0.42em)[
    #grid(
      columns: (4.6cm, auto, 1fr),
      gutter: 5pt,
      align: (left + top, left + top, left + top),
      text(size: 7.4pt, weight: "bold")[#name],
      text(
        size: 6.6pt,
        fill: if state == "done" { good } else if state == "in progress" { accent } else { absent },
      )[#state],
      text(size: 7.1pt, fill: muted)[#note],
    )
  ]
}

// ---------------------------------------------------------------------------
// This session
// ---------------------------------------------------------------------------

#section[What this session did, and how each wave was scoped]

#grid(
  columns: (1fr, 1fr),
  gutter: 14pt,
  [
    #sub[Twenty waves shipped]
    #text(size: 7.4pt)[
      ADR-0049 through 0068: for and defer, using, aggregate returns, multiple returns, named and
      default arguments, scope visibility, imported constants, float constants, context, the
      bounds-check build setting, indirect calls, null plus a memory source, the allocator
      protocol, push_context, pointer arithmetic, temporary storage, trap backtraces, switch, and
      tagged variants.
    ]

    #v(0.3em)
    #text(size: 7.4pt)[
      Test count 900 to 928. Corpus 116 to 155 files. Neovim checks 103 to 166. *W2, W3 and W4.5 are all
      closed* — W4.5 a wave early, because its stated dependency on comptime turned out not to exist. A
      switch over an enum or a tagged variant is exhaustiveness-checked, and a wrong-case read traps.
      What remains is W4 — comptime, the project's top risk.
    ]

    #v(0.3em)
    #text(size: 7.4pt)[
      *Each wave was scoped by writing the feature and seeing what the compiler refused*, not by
      reading the handoff. That found gaps the handoff had missed in both directions: a proc-pointer
      struct field it called absent (it worked), and a void-returning proc-pointer type it never
      mentioned (unspellable, and blocking).
    ]
  ],
  [
    #sub[Two claims the code disproved]
    #text(size: 7.4pt)[
      *An ADR corrected mid-wave.* ADR-0060 §4 asserted the VM dereferences a `malloc`'d pointer via
      libffi. Running the corpus file the same ADR promised would pass disproved it: the VM's memory
      is a linear region and a host address is not an offset into it. ADR-0061 corrects it — the VM
      allocates from its own region, native calls libc, the bits differ and nothing observes them.
    ]

    #v(0.3em)
    #text(size: 7.4pt)[
      *A diagnostic that could not be acted on.* Installing an imported `#foreign` procedure into an
      allocator field reported "expected `(s64) -> *u8`, found `(s64) -> *u8`" — the same text twice,
      because the types differ only in an invisible calling convention. It is E0256 now, the code
      that says to wrap it.
    ]
  ],
)

#v(0.5em)
#block(
  width: 100%,
  inset: 7pt,
  radius: 2pt,
  fill: rgb("#fdf2e6"),
  stroke: 0.5pt + warn,
)[
  #text(size: 7.4pt, weight: "bold", fill: warn)[THE THING WORTH YOUR DECISION]
  #v(0.15em)
  #text(size: 7.4pt)[
    Fourteen waves sit uncommitted across five branches. Two waves ago a careless `git checkout`
    reverted the tree-sitter grammar nine waves and cost an hour's reconstruction — the concrete price
    of not committing. Committing each wave as it goes green would bound the damage from any future
    slip to a single wave.
  ]
  #v(0.15em)
  #text(size: 7.4pt, style: "italic")[
    Recorded as a recommendation, not a change: the authorisation is yours to give.
  ]
]

#v(0.4em)
#text(size: 6.6pt, fill: muted)[
  Sources: PLAN.md §1.5 and §7, the ADR directory, `docs/decisions/DECISIONS.md`, and the six gates
  run today. Every number was measured rather than carried forward — the test count from a full
  workspace run, the corpus count from a file walk, the diagnostic codes from the `const E0nnn`
  definitions across the crates, and the editor-check count from a real `verify.lua` run. Rebuild with
  `typst compile jairs-dashboard.typ`.
]
