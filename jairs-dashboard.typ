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
    #text(size: 8pt, fill: muted)[2 September 2026]
  ],
)
#v(-0.5em)
#text(size: 8.2pt, fill: muted)[
  A Jai-inspired systems language in Rust. Incremental salsa front end, lossless CST, typed SSA
  mid-end, and *three* execution engines — a bytecode VM, Cranelift and LLVM — held byte-identical
  by a differential harness.
]

#v(0.4em)
#pill[7/7 gates green]
#h(4pt)
#pill[1071 tests]
#h(4pt)
#pill[ADR-0178 latest]
#h(4pt)
#pill(fill: rgb("#eaf5ee"), stroke: good)[ALL TWELVE WAVES DONE]
#h(4pt)
#pill(fill: rgb("#eaf5ee"), stroke: good)[everything merged to main]

#v(0.5em)
#grid(
  columns: (1fr, 1fr, 1fr, 1fr, 1fr),
  gutter: 8pt,
  metric("Tests", "1071", "workspace; 1075 with LLVM in"),
  metric("Corpus", "255", "jr files, all three engines"),
  metric("ADRs", "178", "0001 to 0178, immutable"),
  metric("Diagnostics", "128", "codes, E0292 next free"),
  metric("Editor checks", "170", "Neovim, verified not gated"),
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
    - #done #h(3pt) a *third* engine agrees (LLVM)
    - #done #h(3pt) rustc-grade diagnostics
    - #done #h(3pt) formatter round-trips the corpus
    - #done #h(3pt) editor integration (Neovim)
    - #done #h(3pt) DWARF: lines, types, locals
    - #open #h(3pt) verified Linux x86-64 CI run

    #v(0.3em)
    #text(size: 7.2pt, fill: muted)[
      The last one needs a push. Configured, never run, so it is a decision rather than a technical
      gap — and after twelve waves it is the *only* exit criterion still open.
    ]
  ],
  [
    #sub[How correctness is established]
    #set list(marker: none, indent: 0pt, body-indent: 0pt, spacing: 0.5em)
    - #text(size: 7.4pt)[
        *All three engines run every corpus program* and must agree, on output and on trap wording,
        from one shared formatter. The LLVM back end agreed with the VM on all 114 executable programs
        on its *first* run — because both native engines read the same MIR and ask `jr-pool` the same
        layout questions, which is what ADR-0018 §2 centralising layout bought.
      ]
    - #text(size: 7.4pt)[
        *Probe the premise by writing the thing.* Eight times now. The sharpest was `#simd`: Cranelift's
        type *constructor* happily makes a 256-bit vector that no backend can compile, so a plan built
        on the constructor would have looked right until the first compile. Probing set the legal
        widths, and separately found that no ISA has an integer vector divide.
      ]
    - #text(size: 7.4pt, fill: warn)[
        *A performance claim needs a measurement, including when it says no.* Parallel sema was written,
        worked, and produced byte-identical output at every thread count — then measured at 1.20x
        against a 2.5x ceiling, because 40% of a check runs inside the type pool's exclusive critical
        sections. Reverted. The number is the deliverable.
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
    - #text(size: 7.4pt, fill: warn)[
        *A count is not an enforcement.* The test guarding `TrapKind::ALL` asserted `len() == 11` — which
        catches nothing, since a variant left out keeps the length right. Replaced with an exhaustive
        match, it *immediately* found four of fifteen kinds had never been in the list, in a list whose
        doc comment described a driver loop that does not exist. Those four had therefore never been
        checked for message distinctness — the property that keeps a real engine disagreement visible.
        Third instance of one shape: a hand-kept list, a comment claiming something enforces it, nothing
        that does.
      ]
    - #text(size: 7.4pt, fill: warn)[
        *A plan entry saying "blocked on X" is worth twenty minutes of checking that X exists.* W12's last
        item was recorded as blocked on `enable_value_labels` in Cranelift's ISA flags. That flag does not
        exist — not in the settings, not in the meta crate, nowhere. The real gate is one
        `collect_debug_info()` call, and wiring it produced ten real register ranges in twenty minutes.
      ]
  ],
)

// ---------------------------------------------------------------------------
// The language
// ---------------------------------------------------------------------------

#section[The language today]

#let lang = (
  ("Full integer tower, bool, string, pointers", "pointer difference p - q, deferred"),
  ("float32 and float64, plain IEEE-754, no traps; a Math module with vectors, Matrix4 and Quaternion", "percent on floats (E0223); is_nan"),
  ("struct, nominal, one level", ""),
  ("union, nominal, untagged — a cross-field read reinterprets", ""),
  ("variant — a tagged union: a wrong-case read traps, switch destructures", "a recursive variant; eliding the check in an arm"),
  ("enum and enum_flags, namespaced, bare dot-member, switch cases", "an explicit backing type"),
  ("Fixed arrays, views and [..]T dynamic arrays, bounds-checked; a length may name a constant", "a length needing evaluation; array literals"),
  ("struct #soa(N) — one array per field, and e[i].x means e.x[i]", "a bare e[i]; using inside one"),
  ("#simd [N]T — a vector at one of the six register widths; elementwise +% -% *% on integers, + - * / on floats, lane indexing, .count", "any other width; integer /; comparisons (need a mask type); swizzles"),
  ("Per-field #align N (a minimum, power of two up to 4096) and #place N (an exact offset, may overlap, may be unaligned)", "a struct-level #align; any packing form; an operand needing evaluation"),
  ("..T variadics, including ..Any — arguments are pointers", "a bare value coercing to Any"),
  ("#c_variadic — a C-convention variadic a #foreign declaration may take", "a Jairs procedure being one"),
  ("An aggregate crosses a #foreign boundary, by the platform C ABI, in all three engines", ""),
  ("A (T) -> U #c_call procedure *type*, so a body can be handed to C", "a #c_call procedure inside a #run"),
  ("atomic_load, atomic_store, atomic_add, atomic_compare_exchange on s64, sequentially consistent", "wider types; other ops; weaker orderings; a fence"),
  ("Threads: spawn, join, joinable, yield_now, and a spin lock — modules/Thread", "a per-thread backtrace; Thread_Local; channels"),
  ("cast and xx from context; operator overloading", "unary, index and call overloading"),
  ("Trapping arithmetic, wrapping variants, bitwise", "transmute; float printing"),
  ("if, else, while, for, break, continue, defer, using", ""),
  ("switch with exhaustiveness checking over an enum; else", "patterns, ranges, guards; a jump table"),
  ("Multiple returns, named args, literal defaults", "#must; a multi-result call in a return"),
  ("import, foreign, system_library, #scope_module, #expand macros that splice, #modify predicates, #bake_arguments, @note metadata, and noted_count / noted_name / noted_declarations to ITERATE it", ""),
  ("$T and $$T procedures, Box($T) structs, $N comptime-value parameters, #expand macros that splice, #modify predicates, #bake_arguments — W5 complete", "inference through Box($T); a length needing arithmetic"),
  ("Compile-time run at file scope or in a body, across files; type_info(), Any, #insert, #code", "a cross-file #run value; a Code value (declined)"),
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
  bits: documented in three places because it cannot be diagnosed. *Indirect calls work* (ADR-0059) —
  this note used to call them the project's largest gap, and it went stale where nothing could catch
  it, which is the argument for a dashboard whose generator is committed. *The largest remaining gaps*
  are no longer wave-shaped, because every wave is closed: a *per-thread backtrace* (the shadow call
  stack is one global, so a trap in a spawned thread may name the wrong frames), a *register-resident
  local in DWARF* (measured reachable, needs `.debug_loclists`), a *file-scope mutable variable*, and
  a *typed constant* plus *qualified imports* — the last two found by building the library rather
  than by design review.
]

// No forced page break here. There was one, added when the language table happened to end near the
// foot of page 1; six rows later it pushed this section onto page 3 and left page 2 about 60% blank.
// Flowing is the right default for a document whose tables grow every wave.

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

#section[Compiler internals, 17 crates]

#let stages = (
  ("Lexer, parser, CST, typed AST", "works", "Hand-written, error-recovering, trivia-preserving"),
  ("Formatter", "works", "Pure function over the CST; has lost a construct in most waves that added a node kind — #simd made it 9"),
  ("HIR, name resolution, modules", "works", "Flat import merge; cycles legal; export filtering"),
  ("InternPool: types, values, layout", "works", "One layout computation and one integer evaluator, shared. Behind an RwLock: reads share, interning excludes"),
  ("Sema: signatures, checking", "works", "128 codes, E0292 next free, ownership enforced by a cross-crate test; no const-eval here, by design"),
  ("MIR: typed SSA", "works", "Block parameters, not phis; explicit bounds check and zeroing"),
  ("Mid-end", "5 passes", "Inline (non-leaf, bounded rounds), forwarding (cross-block), const-prop, DCE, plus the bounds-check strip. -O0 skips all of it"),
  ("Const-eval", "works", "Runs MIR through the bytecode VM"),
  ("VM: register bytecode, libffi", "works", "No JIT; indirect calls, malloc from its own region; a vector is memory and an elementwise loop"),
  ("Cranelift back end", "works", "Aggregate returns via sret; indirect calls via func_addr; a vector is one register; .debug_line and .debug_info written by hand with gimli"),
  ("LLVM back end", "works", "Via inkwell + LLVM 21, behind a default-off cargo feature; gate 7 is its own test run; DWARF via !dbg metadata, so none of the gimli work carries over"),
  ("DWARF debug info", "works", "Line tables, base and struct type DIEs with real field offsets, a subprogram per function, and stack-resident locals — in BOTH back ends, from one span source"),
  ("Language server", "13 caps", "Diagnostics, hover, goto, completion, rename, actions, hints, symbols, signature help, and semantic tokens — the last one landed in ADR-0159, so the set is complete"),
  ("Neovim integration", "works", "Runtimepath dir, no plugin manager; 170 checks"),
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

#section[Roadmap: all twelve waves closed — see PLAN §1.5 and §7]

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
    "W4 Comptime", "done",
    "DONE in ten sub-waves (ADR-0069 to ADR-0080), delivered that way because a 10-14 week wave cannot be verified the way a one-ADR wave can. A #run may call an imported procedure and appear in a body (ADR-0069), turning two internal compiler errors into working programs. An array length may name a constant (ADR-0070), which REPLACED the scheduled aggressive const folding after probing showed const-prop already did it. A type is a compile-time value (ADR-0071), which closed a silent miscompile — a type bound to a local compiled to an undefined value in a slot with no layout at all. #insert lowers where it is written (ADR-0072/0073), in the enclosing scope, and a computed operand evaluates at compile time and splices before the point sema and the VM become mutually recursive. type_info(T) (ADR-0075) reads its numbers from the same layout_of every real layout decision uses, so reflection cannot disagree with the layout it describes, and Type_Info lives in modules/Basic in Jairs rather than inside the compiler. Any erases a pointer at a named boundary (ADR-0076); #code is a value the metaprogram waves consume.",
  ),
  (
    "W4.5 Pattern matching", "done",
    "switch with exhaustiveness checking, a bare dot-member as a case (settling ADR-0041 §2 step 5), and a tagged variant type (ADR-0067, ADR-0068). Reordered ahead of W4 after checking showed its stated dependency on comptime was a want rather than a need. The variant follows ADR-0045 §1's own instruction — a different declaration form, not a change to union — and union is untouched, still untagged and still one word smaller.",
  ),
  (
    "W5 Polymorphism", "done",
    "DONE in fifteen sub-waves (ADR-0081 to ADR-0097). $T inference, then $$T mixing inference with baking in one signature, then parameterised structs Box($T), then $N comptime-VALUE parameters — a length that is a constant rather than a type. #expand macros SPLICE their body into the caller's scope rather than calling it, #modify predicates run at instantiation and can REJECT one (E0275), and #bake_arguments produces a specialised procedure. The last piece lowers to a REAL procedure — a clone with the baked parameters dropped, their literals substituted and the kept ones remapped, which is the same machinery $N instantiation uses, so the wave ends on a REUSE rather than a new mechanism. Two plans were corrected by building: ADR-0096 intended to use the const-eval pre-pass and found it runs AFTER lowering, and the expansion fixed point (ADR-0120) needed a settling check because an instantiation family can fail to converge.",
  ),
  (
    "W6 Metaprogram", "done",
    "DONE, closed by ADR-0154. @note attaches metadata to a declaration (ADR-0098) as its own node kind rather than a generic attribute, because a note is DATA for a metaprogram while the directives are INSTRUCTIONS to the compiler. has_note and note_value read it at compile time (ADR-0099), folded in sema with no VM and no new query — unlike type_info, which folds later because it needs a layout. The first argument is the declaration itself rather than its name as text, so a misspelling is an unresolved name instead of a silent false. noted_count / noted_name / noted_declarations then let a metaprogram ITERATE what it found, and the compiler-emitted static-data table was the wave-sized architectural decision that made it possible. The wave headline claim is met: a metaprogram finds declarations by note and generates code for each one.",
  ),
  (
    "W7 Stdlib", "done",
    "DONE in twenty-three sub-waves, closed by ADR-0158, and every one was driven by a refusal or a bug rather than a checklist. String exists because ADR-0099 refused == on two strings — same storage and same contents are both plausible for a {data, count} pair — and named a byte loop as the fix, which is a library job since an == that looped would be the only implicitly-looping operator in the language. Its OWN module rather than more of Basic, and the deciding argument is not size: adding to Basic would mean nothing ever tested that TWO modules can be imported at once. Then Sort, Array, Map, Math (vectors, Matrix4, Quaternion — ADR-0115 had declared Math complete when none of the three existed), Random, Time, Bucket_Array, JSON, File, File_Utilities, Process and Socket. Twenty modules. Four polymorphism defects and two silent divergences were found by USING the language rather than by testing it, which is the argument for dogfooding as the acceptance test.",
  ),
  (
    "W8 Performance", "done",
    "DONE in eight sub-waves (ADR-0142 to ADR-0149). An optimisation level -O0/-O1 whose real deliverable is the check the mid-end never had: every corpus program behaves identically at both levels, so a wrong answer is attributable to lowering rather than a pass. The LLVM back end via inkwell behind a default-off feature, making the differential three-way — it agreed with the VM on all 114 executable programs on the FIRST run, because both native engines read the same MIR and ask jr-pool the same layout questions. #align and #place, the first features whose whole implementation is a layout FEATURE rather than a fix. Inliner maturity: the leaf rule is gone, termination is a bounded round count, and forwarding follows a single-predecessor chain — 024-hello's optimised MIR now flattens three call layers the old pipeline could not. A published compile-throughput number, and heap_sort chosen by a COMPARISON COUNT rather than a wall clock, because this project can measure its own throughput and deliberately cannot measure the programs it compiles. #soa, where the sugar IS the feature — without e[i].x it buys nothing over writing [N]T by hand. #simd, whose legal widths were set by probing Cranelift rather than by taste, and whose integer lanes take the wrapping operators because no vector add can trap. And parallel sema, MEASURED AND REFUSED: 1.20x against a 2.5x ceiling, reverted, with the 40%-serial measurement and two named blockers left behind.",
  ),
  (
    "W9 Tooling depth", "done",
    "DONE, closed by ADR-0159. Semantic tokens were the one LSP capability still missing and the other thirteen had landed early; the set is now complete. The wave also RE-SCOPED its own DWARF item with evidence rather than carrying the plan's description: the plan said 'richer DWARF (locals, struct layouts)' as one line, and probing showed it is two implementations — Cranelift wants .debug_info DIEs written by hand while LLVM writes DWARF itself from metadata — so it moved to W12 where it could be budgeted honestly. VS Code stays descoped by ADR-0036, and any LSP client works unpackaged.",
  ),
  (
    "W10 Graphics, in Jairs", "done",
    "DONE in four steps (ADR-0164 to ADR-0167), on a foundation the plan had wrong. It was described as ALL library work with no compiler changes; PLAN §8.5 corrected that, and the corrections cost three compiler waves first: no aggregate crossed a #foreign boundary (ADR-0160/0161, a shared C ABI classification in jr-pool so all three engines agree), objc_msgSend is C-variadic on top of that (ADR-0162, the #c_variadic marker and E0289), and a library search path was needed to link SDL2 at all (ADR-0163). Then Window and a 2D surface, an event loop ADR-0164 had said was impossible — recorded and then DISPROVED by one line, because SDL_Event is a union and a *pointer to one is takeable — immediate-mode UI widgets, and Image with BMP and textures.",
  ),
  (
    "W11 Concurrency", "done",
    "DONE, and the LAST of the twelve (ADR-0175 to ADR-0177). The blocker was not the one this row used to name. PLAN §8.3 said a per-thread VM stack, atomics as language operations, and a comptime rule; it did not say a thread body could not be NAMED. pthread_create takes a function pointer, and #c_call was a DECLARATION attribute with no spelling in a TYPE — jr-pool had modelled the two conventions as distinct types since ADR-0001 and the checker interned the distinction away with a comment explaining why that was safe. Found by three probes in four minutes, the third reporting 'expected (s64) -> s64, found (s64) -> s64': two identical types, because the type describer did not render the convention either. Then atomics as a MIR Rvalue rather than library calls, which the exhaustive-match rule turned into nine compile errors each of which had to be argued — forwarding would have moved a write past a synchronisation point, DCE would have deleted a compare-exchange whose EFFECT is the lock. Then modules/Thread as a binding, with a spin lock because pthread_mutex_t is 64 opaque platform-sized bytes this language cannot spell. Three threads, 3000 atomic increments, exactly 3000, in both native back ends, five runs per test.",
  ),
  (
    "W12 Debug info", "done",
    "DONE in six waves (ADR-0169 to ADR-0174), and §8.4 had claimed line tables already existed when dwarfdump on a built binary printed an empty section. So it started from zero. A .debug_line for Cranelift written by hand with gimli — a SourceLoc indexing a (path, line) vocabulary, a relocation writer for sequence addresses — and then for LLVM, where NONE of that is reusable because LLVM writes DWARF itself from !dbg metadata. Both verified by PARSING the section the way lldb does rather than grepping dwarfdump. Items 2 and 3 turned out COUPLED: a struct mapping was written, was correct, and dwarfdump showed no struct, because LLVM prunes a type nothing declares and a signature is not a declaration — what retains a type is a VARIABLE of it. One item remains and is specified rather than vague: a register-resident local, whose plan entry named an ISA flag that does not exist.",
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
        // `blocked` is amber, not the grey `not started` gets: a wave that cannot be started until
        // something else lands is a different fact from one nobody has picked up, and the whole point
        // of PLAN §8.5 is that W10 is the first kind while the table used to imply the second.
        fill: if state == "done" { good } else if state == "in progress" { accent } else if state == "blocked" { warn } else { absent },
      )[#state],
      text(size: 7.1pt, fill: muted)[#note],
    )
  ]
}

// ---------------------------------------------------------------------------
// This session
// ---------------------------------------------------------------------------

#section[What the last stretch did, and the three claims it settled]

#grid(
  columns: (1fr, 1fr),
  gutter: 14pt,
  [
    #sub[Four waves shipped; the programme closed]
    #text(size: 7.4pt)[
      ADR-0155 through 0178. Test count 1033 to *1071* (1075 with LLVM compiled in), corpus 237 to
      *255* files, ADRs 149 to *178*. W7, W9, W10, W11 and W12 all closed, which means *every wave in
      the plan is done* — and three of the five had to correct the plan's own description of them
      before they could start.
    ]

    #v(0.3em)
    #text(size: 7.4pt)[
      *The wave order was right and the wave contents were not.* W10 was listed as pure library work;
      it needed three compiler waves first (FFI aggregates, C-variadics, a library search path). W9's
      "richer DWARF" was one line and is two implementations. W11's blocker was a missing *type*, not
      the runtime work the row named. In each case the correction came from writing the thing, and in
      each case the plan row now records what was wrong rather than quietly reading as though it had
      always said this.
    ]

    #v(0.3em)
    #text(size: 7.4pt)[
      *A seam drawn early keeps paying.* ADR-0009 confined `cranelift-*` behind `jr-codegen::Backend`
      twelve waves before a second back end existed. The DWARF waves are the third collection to
      benefit: `TrapKind`, `SourceInfo::position` and now the whole span vocabulary live in the shared
      crate, so a line-table row and a trap's `--> file:line:col` come from *one* resolution and cannot
      say 41 and 40.
    ]
  ],
  [
    #sub[Three claims the probes settled]
    #text(size: 7.4pt)[
      *A negative result from one program is evidence about that program.* ADR-0172 §3 concluded, from
      a single test, that a structure-typed local could never be shown by name. ADR-0174 disproved it an
      hour later with a one-line change: it depends on *use*. One passed to a procedure by value is
      named, in both engines; one only assigned field by field is not. Generalising a negative needs a
      second program that differs in the suspected dimension.
    ]

    #v(0.3em)
    #text(size: 7.4pt)[
      *An impossibility claim is a probe you have not run.* ADR-0164 recorded that an event loop was
      impossible because `SDL_Event` is a union and no pointer to one could be taken. ADR-0165 built it
      the next wave — the premise was simply wrong, and it had been *written down* as a design
      constraint. Both the claim and its withdrawal are in the record, because a project that quietly
      deletes its wrong claims cannot learn the shape of them.
    ]

    #v(0.3em)
    #text(size: 7.4pt, fill: warn)[
      *A data race is worth measuring, not asserting.* The memory model's data-race clause is not a
      promise: the same three-thread program with a plain `+ 1` instead of an atomic add produced *1000
      instead of 3000* on one run of three. Two thousand increments lost, no diagnostic. That number is
      why §3 of ADR-0177 exists as a section rather than a paragraph saying one would be written later.
    ]
  ],
)

#v(0.5em)
#block(
  width: 100%,
  inset: 7pt,
  radius: 2pt,
  fill: rgb("#eaf5ee"),
  stroke: 0.5pt + good,
  // Unbreakable: this box split across a page boundary mid-sentence, which reads as a rendering
  // fault rather than a design choice. A callout is one unit or it is not a callout.
  breakable: false,
)[
  #text(size: 7.4pt, weight: "bold", fill: good)[SETTLED SINCE THE LAST DASHBOARD]
  #v(0.15em)
  #text(size: 7.4pt)[
    *All twelve waves are closed, and everything is merged.* Twenty-three more waves went onto `main`
    with `--no-ff`, one merge commit each, so `git log --merges main` still reads as the wave history —
    a claim you can check rather than a revision that goes stale the moment this file is committed.
    (That is why no SHA appears here: the commit that would record one changes the thing it records.)
    The branch names were corrected first *again*: sixteen waves had accumulated on a single
    `feat/c-variadic` branch, the same hygiene slip the last dashboard recorded fixing for two W8
    sub-waves, so each now names its own work.
  ]
  #v(0.15em)
  #text(size: 7.4pt, style: "italic")[
    The one open slice criterion is still a verified Linux x86-64 CI run — configured, never run, which
    makes it a decision rather than a technical gap, and it is now the *only* one. Owed and specified
    rather than vague: a per-thread shadow call stack, so a trap in a spawned thread names the right
    frames; a register-resident local in DWARF, measured reachable and needing `.debug_loclists`; the
    security audit's remaining two dispatches (by hand — six subagent dispatches returned empty); and
    `tree-sitter test` added to gate 6, which catches grammar drift but not a broken rule.
  ]
]

#v(0.4em)
#text(size: 6.6pt, fill: muted)[
  Sources: PLAN.md §1.5 and §7, the ADR directory, `docs/decisions/DECISIONS.md`, and all seven gates
  run today on `main` *after* the merge, not on a branch. Every number was measured rather than carried
  forward — the test count from a full workspace run (1071, zero failures) and a second under
  `--features jr-cli/llvm` (1075), the corpus count from a file walk (255 `.jr` files under
  `tests/corpus/` outside `tests/corpus/modules/`), the ADR count from `docs/adr/`, the diagnostic codes
  from the `const E0nnn` definitions across the crates — whose ownership is now enforced by
  `jr-cli/tests/codes.rs` rather than asserted in prose — and the editor-check count from a real
  `verify.lua` run (170). Rebuild with `typst compile jairs-dashboard.typ`.
]
