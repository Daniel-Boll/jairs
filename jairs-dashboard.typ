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
#pill[960 tests]
#h(4pt)
#pill[ADR-0075 latest]
#h(4pt)
#pill(fill: rgb("#fdf2e6"), stroke: warn)[W4 open · 7 sub-waves done]

#v(0.5em)
#grid(
  columns: (1fr, 1fr, 1fr, 1fr, 1fr),
  gutter: 8pt,
  metric("Tests", "960", "workspace, all passing"),
  metric("Corpus", "168", "jr files, both engines"),
  metric("ADRs", "75", "0001 to 0075, immutable"),
  metric("Diagnostics", "97", "codes, E0267 next free"),
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
        *Check the query a claim is about.* "The arithmetic around a `#run` is not folded" was true of the
        *built* MIR and false of the *optimized* one — and the corpus only ever snapshots the built
        query, so nothing the tests display would have shown it. A claim about optimisation needs a probe
        of the optimized body.
      ]
    - #text(size: 7.4pt, fill: warn)[
        *A well-typed placeholder is invisible to every gate this project has.* `t := Point;` — a type
        bound to a local — type-checked cleanly and *both engines exited 0*, storing an undefined value
        into a slot of a type with no runtime layout at all. Three of these now, and each was found by
        asking a question no test asks: what does this lower to? The corpus checks exit codes and
        snapshots the built MIR; a construct that is legal, silent and unread sits outside all of it.
      ]
    - #text(size: 7.4pt, fill: warn)[
        *A test naming an unimplemented thing has a one-wave shelf life.* The refused-body test has now
        had its construct replaced twice, both times because the wave after it implemented the gap it
        named. It uses something refused *by design* now. And one of this wave's own new tests passed
        vacuously until it was teeth-checked.
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
  ("Fixed arrays and views, bounds-checked; a length may name a constant", "a length needing evaluation; array literals"),
  ("cast and xx from context; operator overloading", "unary, index and call overloading"),
  ("Trapping arithmetic, wrapping variants, bitwise", "transmute; float printing"),
  ("if, else, while, for, break, continue, defer, using", ""),
  ("switch with exhaustiveness checking over an enum; else", "patterns, ranges, guards; a jump table"),
  ("Multiple returns, named args, literal defaults", "#must; a multi-result call in a return"),
  ("import, foreign, system_library, #scope_module", "polymorphs, macros (W5)"),
  ("Compile-time run at file scope or in a body, across files", "type_info(), Any, insert, Code"),
  ("A type as a compile-time value: T :: Point aliases one, usable anywhere Point is", "a chain B :: A; comparing types; Type as an annotation"),
  ("A type in a runtime position is refused — it has no representation to store", ""),
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
    "W4 Comptime", "in progress",
    "Delivered in sub-waves, because a 10-14 week wave cannot be verified the way a one-ADR wave can. Seven have shipped. A #run may call an imported procedure and appear in a body (ADR-0069), turning two internal compiler errors into working programs. An array length may name a constant (ADR-0070), which replaced the scheduled aggressive const folding after probing showed const-prop already did it. A type is a compile-time value (ADR-0071), which closed a silent miscompile — a type bound to a local compiled to an undefined value in a slot with no layout. Insert of a string literal lowers where it is written (ADR-0072), in the enclosing scope, every synthesized span pointing at the directive because jr-diag clamps an out-of-range offset rather than rejecting it. And a computed insert (ADR-0073) evaluates its operand at compile time and splices the text — the point sema and the VM become mutually recursive, PLAN section 5's named top risk, broken by an acyclic pre-pass that reuses the constant evaluator and re-lowers only the affected bodies rather than by salsa fixed-point recovery. An aggregate compile-time value (ADR-0074): a #run returning a struct or array interns as its element values rather than a byte image, because the pool is target-independent and an image is not. And type_info(T) (ADR-0075), reflection's first half: a type's kind, name, size and alignment, the numbers coming from the same layout_of every real layout decision uses, so reflection cannot disagree with the layout it describes. Type_Info is declared in modules/Basic in Jairs rather than inside the compiler, because it has to be spellable — no compiler-declared type can be named at all — and the resulting dependency on a declaration the compiler does not own is validated on lookup, so editing that struct is a diagnostic rather than a read of whatever now sits at the old offset. Getting there first needed a constant that may hold a string, which ADR-0074's own closing claim said was already done and was not: the fourth false scheduled dependency this project has found, and the first where the false claim was its own ADR about the very next wave. What remains: Any; per-kind detail in Type_Info, each member variable-length and wanting a memory-ownership decision; and #code, a quoted syntax tree as a value.",
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
    #sub[Twenty-six waves shipped]
    #text(size: 7.4pt)[
      ADR-0049 through 0074: for and defer, using, aggregate returns, multiple returns, named and
      default arguments, scope visibility, imported constants, float constants, context, the
      bounds-check build setting, indirect calls, null plus a memory source, the allocator
      protocol, push_context, pointer arithmetic, temporary storage, trap backtraces, switch,
      tagged variants, compile-time run across files and in a body, an array length from a
      constant, a type as a compile-time value, insert of a literal and a computed string, and an aggregate compile-time value.
    ]

    #v(0.3em)
    #text(size: 7.4pt)[
      Test count 900 to 960. Corpus 116 to 168 files. Neovim checks 103 to 166. *W2, W3 and W4.5 are all
      closed*, and *W4 is open* with six sub-waves shipped: a `#run` reaches across files and into a body,
      an array length may name a constant, a type is a value, `#insert` of a literal lowers where it is
      written, and now a *computed* `#insert` evaluates its operand at compile time and splices it — the
      point sema and the VM become mutually recursive, the cycle broken by an acyclic pre-pass. *Three
      times* a plan's stated reason turned out not to hold — W4.5's dependency on comptime, sub-wave 2's
      folding work, and a nesting hang escaping makes impossible. An *aggregate* compile-time value now
      interns too — as its element values, never a byte image, because the pool is target-independent —
      which is what RTTI was really blocked on. Remaining: `#code`/`Code`, then `type_info()` and `Any`,
      now a *schema* decision rather than a representation one.
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
    *Every wave is now committed as it greens*, on its own `feat/` branch, which closed the risk this box
    used to name. Nothing has been merged: `main` is still at `ec150a5`, and twenty-three waves sit on
    stacked branches ahead of it. That is a decision rather than a gap — merging needs your say-so, and
    the per-wave commits mean no work is at risk while it waits.
  ]
  #v(0.15em)
  #text(size: 7.4pt, style: "italic")[
    The one open slice criterion is still a verified Linux x86-64 CI run, which needs a push.
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
