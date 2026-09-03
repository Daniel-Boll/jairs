# Jai parity: what real Jai code uses that Jairs lacks

**Status:** research, not a plan. Nothing here is scheduled; `PLAN.md` §7 owns the order.

Two inventories, both built from **primary sources** and both **probed** where a probe was possible:

1. **Syntax** — the constructs real Jai programs use, each tried against this compiler.
2. **Libraries** — the modules Jai ships and the community wrote, each traced to a file that imports it.

## How much to trust each claim

Jai's compiler is a closed beta and its `modules/` tree is unpublished, so nothing here rests on reading
it. Four kinds of evidence appear, and every row says which it used:

| Evidence | Proves | Example |
|---|---|---|
| **Vendored module source** | behaviour, and a signature | `focus-editor/focus` and `valignatev/hitboxer` both carry Jai's `Simp` verbatim; both were read and **diffed** |
| **A real program's call sites** | behaviour, not a declaration | `chess-jai/ui.jai` drives six `GetRect` widgets across 2244 lines |
| **A file that imports 95 modules** | that a *name* is importable | `SogoCZE/jai_parser/tests/performance_test.jai`, a parser stress test |
| **Beta users' documentation** | intent, weakest | the Jai Community Library wiki, `The_Way_to_Jai` |

Version: `focus/first.jai:1-2` pins Jai **beta 0.2.029** as its minimum, so this describes the
0.2.02x–0.2.03x module set.

**A vendored copy can be a fork.** Focus changes `Simp.draw_text`'s colour from a `Vector4` to a `u8`
colour-map index. That divergence is only visible because two copies were compared, which is why they
were.

---

## 1. Syntax, probed

Read across `danieltan1517/chess-jai` (12 files, 12,283 lines), `SogoCZE/jai_parser` (40 files, 6,286)
and `SogoCZE/jai_wgpu_native` — then each construct was **run against this compiler**, because a
document saying a feature works is a claim and not a result.

| Construct | Occurrences | Jairs | Note |
|---|---|---|---|
| `s64.[1, 2, 4]` array literal | **39** | absent | Parse error. The most-used construct Jairs lacks |
| `Code` value + `for`-expansion macro | **58** call sites | absent | **One gap, one fix**: a for-expansion macro's second parameter is literally `body: Code` (`chess-jai/movegen.jai:1802`). ADR-0080 *declined* a `Code` value "until something can inspect a tree" — real code inspects one 58 times, so that decision now has evidence against it |
| `cast,no_check` / `cast,trunc` | **65** | **works** — differently | Jairs traps on overflow (ADR-0002) and has `+% -% *%`. Probed: `1u64 << 63` and `cast(u64, -1)` both behave, so the bitboard patterns port |
| `type_of(x)` | **14** | absent | E0201. Jairs has `type_info`, so this is a small addition beside it |
| statement `#if` on a `$` parameter | ~**14** | absent | Parse error |
| `for v, i: a` — element and index | **14** | **works** | Probed, exits 17 |
| `for *p: a` — by pointer | **11** | absent | Parse error |

**One correction to this repository's own contract.** The brief for this research listed "no
context-based `push_context`" as a known gap; `docs/adr/README.md:86` records **ADR-0063 as Accepted**
— "`push_context` gives a block its own copy of the context". The brief was wrong, and it was written
from memory.

**A caveat that applies to every "works" row anywhere.** A construct marked supported *by a document*
is a claim about the last time someone ran it. This project has been bitten repeatedly — ADR-0125 found
the README's "Absent" column listing three shipped features, ADR-0168 found three stale
`[NOT DELIVERED]` markers. The seven rows above are probed. Nothing else here is.

---

## 2. Libraries: the ranked eight

Ranked by **value per unit work**, where value means how many other things an item unblocks or how large
a *recorded* debt it closes. **No item in this eight needs a compiler change** — that is the result of
the ordering, not a coincidence.

### 1. `Text_File_Handler` — unblocks a family, costs almost nothing

Line-oriented reading with comment stripping. **Three independent Jai libraries build directly on it** —
`jai-ini/module.jai:36`, `toml-jai/module.jai:37`, `jai-csv/module.jai:303`. Jairs has
`read_entire_file` and nothing line-oriented, so every future format module would hand-roll splitting.
~200 lines. The best ratio in the inventory.

### 2. `stb_image` / `stb_image_write` — one binding replaces a crippled module

`modules/Image` reads and writes **BMP only**. Real Simp imports these three
(`focus/modules/Simp/module.jai:44-47`), so this is also a fidelity fix. Scalar-and-pointer API, so
nothing crosses a `#foreign` boundary by value. ~40 lines of bindings plus a wrapper.

### 3. Ryū (`ostef/jai-ryu`) — one algorithm closes two debts this repo wrote down

`modules/JSON` defers serialisation for exactly one stated reason, "a correct `dtoa`", and `Basic`
cannot print a float at all. Pure arithmetic with tables, ~400 lines, no FFI. **The only item that
closes a debt the repository itself recorded.**

### 4. `Pool` / `Flat_Pool` — fixes the allocator seam AGENTS.md names

Jairs' only arena is a fixed 64 KiB `talloc` region that cannot grow. AGENTS.md records the
consequence: `List` and `Map` use `malloc`, `String` uses the context, and `JSON` was the first module
to straddle the two. ~150 lines; the context allocator already exists to install it behind.

### 5. `Unicode` — deletes code rather than adding it

`modules/JSON` already hand-rolls `surrogate_pair_at`, `unicode_escape`, `utf8_width`, `write_utf8` and
`escape_width`. A second consumer would copy them. ~150 lines, most already written.

### 6. `System` + `POSIX` — one error story, five fewer copies of libc

Jairs reports failure as a bare `bool` in `Window`, `Image`, `Socket` and `File`; only `Simp.get_error`
reaches a message, and only SDL's. `System.get_error_value_and_string()` is the whole answer. `POSIX`
additionally removes five hand-written `#foreign libc` blocks — `File`, `Process`, `Socket`, `Time` and
`Thread` each has its own. The `POSIX` half lands imperfect: **no typed constants**, so
`O_CREAT : u32 : 0x200` does not parse.

### 7. FreeType + text in `Simp` — the largest gap in the graphics stack

`modules/Simp` cannot draw a character. Jai's flow is `get_font_at_size`, `prepare_text`,
`draw_prepared_text` over a `Dynamic_Font` holding an `FT_Face` and a glyph cache;
`focus/modules/Simp/font.jai` is 400+ lines and is only the drawing half. FreeType passes aggregates by
**pointer**, so E0286 does not bite. Highest raw value in the report, ranked here purely on size.

### 8. A `GetRect`-shaped `UI` — most visible, worst ratio, and it needs 7 first

`modules/UI` has **one** widget. `GetRect` has `button`, `dropdown`, `slider`, `text_input`, `label`,
subwindows, scrollable regions and a theme tree. Every widget beyond `button` is independent work with
no shared unlock, and the text-bearing ones cannot start until FreeType lands — doing this first
produces widgets that cannot show a label.

### What lost, and why

- **`Hash_Table` with a string key** — very high value, but a generic `Table($K,$V)` is **E0269**
  (cross-module parameterised structs), and a second concrete instance is a copy of `Map` rather than a
  general answer. It becomes top-three the day E0269 lifts.
- **`Command_Line`** — small, but **a Jairs program cannot reach `argv` at all**. That is compiler work,
  so the cheap module cannot pay off yet.
- **`Bindings_Generator`** — would pay for items 2 and 7 and every graphics binding, but it needs a C
  parser.
- **`Objective_C`, Metal, native `Window_Creation`** — blocked on a real refusal rather than on effort:
  a `#c_variadic` call is **E0289**, so `objc_msgSend` is unreachable and Cocoa with it.
- **Audio (`Sound_Player`, `Wav_File`)** — genuinely absent, and SDL2 is already linked so `SDL_audio`
  is at hand. It lost to item 8 only because a GUI has more callers here than a mixer.

## 3. The strategically interesting one: `jai_wgpu_native`

`SogoCZE/jai_wgpu_native` is a real library — 2143 lines of generated `wgpu-native` bindings, a
`generate.jai` that rebuilds them, and **prebuilt binaries for all three targets**.

**WebGPU is one library name on macOS, Linux and Windows.** That is exactly the property the graphics
work wanted and could not get from OpenGL, which is `OpenGL.framework` / `GL` / `opengl32` — three names
and two linker argument forms (ADR-0183). It needs no `objc_msgSend`, and its descriptors are large
structs passed **by pointer**, so E0286 does not bite either.

It is not in the eight because it replaces a backend that works rather than filling a hole, and 2143
lines is item-7-sized. It is recorded here because a *later* decision about a portable GPU backend
should start from this rather than rediscover it.

## 4. `jai_parser` is worth mining, not porting

Its `tests/` directory is 40+ files of real Jai syntax corner cases — here-strings, `ifx`, dotless
struct literals, inline assembly, named returns, `#exists`, import-with-arguments, discard,
comma-separated declarations. **That is a ready-made checklist for the next syntax audit.** Several are
deliberately *invalid* Jai (`tests/dot.jai:17`, `tests/hang.jai`), so treat a construct that appears
only under `tests/` as "the grammar admits it", not "real programs need it".

Its `tests/performance_test.jai` imports 95 modules in one file and is the best public inventory of
Jai's module tree. Two of the 95 are **not** Jai's — `wgpu` is the repository author's own and
`uniform` is a community regex library — so it proves a name is importable and never that a module ships.

## 5. Two claims corrected before they became rows

- **`Linked_List` is not a Jai module.** `The_Way_to_Jai/book/08A:218` presents it as a *reader
  exercise*, and it is absent from the 95-module import list.
- **`Atomics` is not a Jairs gap.** Jairs made atomics a MIR variant rather than a module
  (ADR-0175–0177), so porting it would duplicate the language.

## 6. What could not be verified

Named here rather than smoothed over, because an honest gap is worth more than a plausible guess.

- **The distribution itself.** No row rests on reading `jai/modules/`.
- **Purpose inferred from a module *name* only**, behaviour unverified anywhere: `Adpcm`, `Treemap`,
  `Shared_Memory_Channel`, `Zip_File_Directory`, `executable_formats`, `Unmapping_Allocator`,
  `wait_group`, `PCG`, `Hash`, `Crc`, `Base64`, `Srgb`, `Float16`, `linux_build`, `Thekla_Atlas`,
  `Thekla_Baker`, `MojoShader`, `uSockets`, `Codes`, `Code_Visit`, `Windows_Registry`, `Gamepad`,
  `Keymap`.
- **`GetRect` has no published source.** Every signature for it is call-site-inferred, from
  `chess-jai`, which states no minimum compiler version and may lag.
- **`Text_File_Handler`'s declarations.** Real call sites and field names from two independent
  libraries; the module's own declarations were never read.
- **Whether `Array` is still a separate Jai module.** The wiki documents one; it is absent from the
  import list, and the book uses array procedures out of `Basic`. Probably folded in. Unresolved.
