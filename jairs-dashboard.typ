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
#pill[919 tests]
#h(4pt)
#pill[ADR-0065 latest]
#h(4pt)
#pill(fill: rgb("#fdf2e6"), stroke: warn)[W3 in progress · backtraces the last feature]

#v(0.5em)
#grid(
  columns: (1fr, 1fr, 1fr, 1fr, 1fr),
  gutter: 8pt,
  metric("Tests", "919", "workspace, all passing"),
  metric("Corpus", "149", "jr files, both engines"),
  metric("ADRs", "65", "0001 to 0065, immutable"),
  metric("Diagnostics", "88", "codes, E0258 next free"),
  metric("Editor checks", "156", "Neovim, verified not gated"),
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
        *MIR snapshots pin the exact instruction sequence.* This wave the basic-module snapshot shows
        `talloc` reading the two new context fields and bumping the cursor with pointer arithmetic —
        the whole allocator, and no new MIR node, because it is three prior waves composed.
      ]
    - #text(size: 7.4pt)[
        *A feature that adds no machinery is the payoff of the ones that did.* Temporary storage is a
        `malloc`'d region, a cursor moved by pointer arithmetic, and two context fields — the compiler
        change is two entries in one array, and everything else is Basic code. That is what the
        allocator and pointer-arithmetic waves were building toward.
      ]
    - #text(size: 7.4pt, fill: warn)[
        *A limit stated is better than a limit faked.* `talloc` returns a `*u8`, so an arena hands out
        byte buffers only — storing a wider type needs a pointer cast the language does not have yet.
        The corpus stores bytes and says so, rather than pretending a wider store works.
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
  ("union, nominal, untagged, fields at offset 0", "a tagged variant type (W4.5)"),
  ("enum and enum_flags, namespaced, bare dot-member", "explicit backing type; a switch (W4.5)"),
  ("Fixed arrays and views, bounds-checked, returnable", "array literals, sub-slicing"),
  ("cast and xx from context; operator overloading", "unary, index and call overloading"),
  ("Trapping arithmetic, wrapping variants, bitwise", "transmute; float printing"),
  ("if, else, while, for, break, continue, defer, using", ""),
  ("Multiple returns, named args, literal defaults", "#must; a multi-result call in a return"),
  ("import, foreign, system_library, #scope_module", "polymorphs, macros (W5)"),
  ("One trivial compile-time run, folded", "arbitrary run, RTTI, insert (W4)"),
  ("Traps name their source line", "backtraces (W3)"),
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
  ("Neovim integration", "works", "Runtimepath dir, no plugin manager; 156 checks"),
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
    "W3 Runtime core", "in progress",
    "Data structures done; one feature left. context (ADR-0057), the bounds-check build setting (ADR-0058, finishing ADR-0003), indirect calls (ADR-0059), null plus a memory source (ADR-0060/0061), the allocator protocol (ADR-0062), push_context (ADR-0063), pointer arithmetic (ADR-0064), and temporary storage (ADR-0065) — talloc hands out bytes from a per-context bump arena, W3's last data structure and one that composes the previous three waves rather than adding machinery. Remaining: traps with backtraces, the only W3 feature left.",
  ),
  (
    "W4 Comptime", "not started",
    "Arbitrary compile-time execution, RTTI and Type values, insert, code. PLAN section 5 names this the project's top risk: sema and comptime become mutually recursive.",
  ),
  (
    "W4.5 Pattern matching", "not started",
    "switch with exhaustiveness, a bare dot-member as a case, and a tagged variant type. Was missing from the wave table entirely — two accepted ADRs deferred decisions to it while no wave scheduled it.",
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
    #sub[Seventeen waves shipped]
    #text(size: 7.4pt)[
      ADR-0049 through 0065: for and defer, using, aggregate returns, multiple returns, named and
      default arguments, scope visibility, imported constants, float constants, context, the
      bounds-check build setting, indirect calls, null plus a memory source, the allocator
      protocol, push_context, pointer arithmetic, and temporary storage.
    ]

    #v(0.3em)
    #text(size: 7.4pt)[
      Test count 900 to 919. Corpus 116 to 149 files. Neovim checks 103 to 156. W2 closed; W3 opened
      and all its data structures landed — an allocator in the context, push_context to scope it, raw
      pointer arithmetic, and a temporary-storage arena built from all three. Only backtraces remain.
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
