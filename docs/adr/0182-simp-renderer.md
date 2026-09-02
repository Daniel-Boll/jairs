# ADR-0182: The graphics modules restructured onto a Simp-shaped API

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **Groups D and E of the Simp-shaped-graphics plan**, and the wave the two language groups existed for.
  Three modules become five, on `SDL_RenderGeometry` instead of `SDL_RenderFillRect`.
- The plan's design was **wrong in three places**, each found by writing the thing. All three corrections are
  recorded at their point of decision.

## Context

### What Simp actually is, verified from primary sources

Read from the Jai Community Library wiki and from complete working programs in `The_Way_to_Jai`
(`33.15_drawing_texture.jai`, `52.1_simp_pong.jai`, `33.8_load_font.jai`).

Four modules: `Window_Creation`, `Simp`, `Input`, `GetRect`. The wiki states the backend plainly:

> SIMP is a simple rendering framework for programming simple 2D graphics. SIMP has a GL backend.
> Eventually, SIMP will have other backends.

**So Simp is single-backend, and all the per-OS code lives in `Window_Creation`.** Its
cross-platform-ness is not three backends behind one API; it is one API on one portable graphics library
with the platform work quarantined. That is the shape to imitate, and it is why this wave is a *restructure*
rather than a rewrite.

The verified call surface:

```jai
window := create_window(window_name = "Load a texture", width = width, height = height);
set_render_target(window);
window_width, window_height := get_render_dimensions(window);
clear_render_target(0.15, 0.08, 0.08, 1.0);
set_shader_for_images(texture);
immediate_begin();
immediate_quad(v0, v1, v2, v3, uv0 = uv0, uv1 = uv1, uv2 = uv2, uv3 = uv3);
Simp.immediate_quad(x, y, x + w, y + h, color);
immediate_flush();
swap_buffers(window);
```

Four things about it that the old `modules/Window` got wrong, and this wave fixes:

- **colours are floats in 0..1**, not the 0-255 integers `Window.set_color` took;
- **two `immediate_quad` forms** — two opposite corners plus one colour, and four arbitrary corners plus four
  UVs. The second cannot be expressed by `SDL_RenderFillRect` at all;
- **colour is per-quad, passed in**, not renderer-global state set beforehand;
- **shader mode is selected before a batch**, and a `Texture` carries its own `.width`/`.height`.

### Why SDL2 and not OpenGL

Jairs cannot name OpenGL portably: it is `OpenGL.framework` on macOS, `GL` on Linux and `opengl32` on
Windows, and a per-OS *library name* stays out of reach even after ADR-0180 — the operand would have to be
evaluated before the library is known, and ADR-0180 records that cycle. `-lSDL2` is one name on all three.

`SDL_RenderGeometry` is what makes the mapping faithful: per-vertex position, colour and normalised texture
coordinate is exactly `immediate_quad`'s four-point form. Verified against the installed header (SDL
2.32.10, `SDL_render.h:1650`) **and by writing a Jairs program that calls it** before a line of the module
existed — six vertices, two triangles, a per-vertex colour, and `SDL_GetRendererOutputSize` reading back 320.

`SDL_Vertex`'s layout was measured, not reasoned about: a `cc`-compiled `offsetof` says **20 bytes, align 4,
position at 0, colour at 8, tex_coord at 12**, and the Jairs `Vertex` of eight flat scalars lands on exactly
those offsets.

## Decision

### §1 — There is no module-level mutable state, so the state is caller-owned

**This is the plan's first wrong premise, and it invalidated two of its five items.** The plan said the
renderer handle, the current texture and the vertex batch are *module-level state*, "because
`set_render_target` and `set_shader_for_*` are stateful in Simp and threading a context through every call
would be a different API". Likewise `Input`'s frame buffer.

**Jairs has no module-level mutable state.** A file-scope `var` is E0245 — *"a file-level item has no value
until jr-vm"* — probed for a scalar and for an array before either module was written, and it is the same
gap ADR-0178 gave a trapping stub to. So both designs were unbuildable as written.

The answer is not a compiler feature but the pattern **this library already uses**: `modules/UI` declares a
`UI` struct the caller owns, keeps between frames, and passes by pointer, for the reason its own docs give —
*"a caller declares one, keeps it between frames, and never allocates"*. So:

- `Simp.Renderer` holds the handle, the texture, the batch flag, the vertex count and the vertices;
- `Input.Events` holds the frame buffer and its count;
- every routine takes one by pointer.

Rejected: implementing file-scope mutable variables to make the plan's design work. That is a `.data`
section, static initialisation, and three engines — a *language* wave, and one this graphics wave would have
been blocked behind for no gain in the API a caller sees.

**And the explicit version is the better API.** Two windows can have two renderers and two event queues,
which a module global cannot express. Jai's Simp is global because Jai has globals; the shape that matters —
a batch opened, quads carrying their own colour, a flush, a shader mode chosen before the batch — is
preserved exactly.

The cost is named rather than hidden: a `Simp.Renderer` is about **20 KB** and an `Input.Events` about
**14 KB**, because the batch and the buffer are fixed-size arrays. Fixed-size because a dynamic array needs
the context allocator and no graphics module allocates.

### §2 — `Window` becomes `Window_Creation`, with C's widths

`Window` keeps window creation, `start`/`stop`, `delay`, `video_driver_count`, `Rect` and `rect`. The
renderer went to `Simp`; the events and the `SDL_Event` overlay went to `Input`. `set_color`, `clear`,
`fill`, `outline`, `line` and `present` were **deleted**, not deprecated: `Simp` owns drawing, and a second
way to draw would be the opposite of a clean cutover.

`open` became `create_window`. Jai's name, and a correction rather than a rename: `open` collided with
`File.open`, which is the E0211 that made a program that both draws and reads a file **unwritable** and the
whole reason ADR-0179 came first.

**Every `#foreign` width is C's now.** They were all `s64`, even where SDL declares `int`, `Uint32` or
`Uint8` — which contradicted this file's own `Rect`, whose docs explain at length why a narrow field matters
at a C boundary. Correcting them is unambiguously right on every target, and it is a prerequisite for
trusting the boundary on Windows x64, whose first four arguments are register-passed and whose remainder are
stack-passed. The *wrapper* parameters stay `s64` so callers need no casts: the narrowing happens once, at
the boundary, exactly as `rect` already did it.

**A Jairs constant has no width**, so every constant crossing the boundary is cast explicitly — probed:
`CENTERED : s32 : 805240832` does not parse. That is the typed-constant gap ADR-0165 §5 already owed for
`QUIT : u32 : 256`, met here at four more call sites.

### §3 — `Simp`, and where the plan put a procedure that could not compile

`Simp` owns the renderer, the vertex batch, and the two `immediate_quad` forms — named `immediate_quad` and
`immediate_quad_uv`, two procedures rather than one, because Jai distinguishes them by argument *count* and
Jairs has no overloading.

**`get_render_dimensions` is in `Simp`, not `Window`.** The plan put it in `Window` and said it binds
`SDL_GetRendererOutputSize` — which needs the *renderer*, and after this restructure `Window` does not have
one. The plan's version could not have compiled. Jai puts it in `Simp` too. `Window` gets
`get_window_size` instead, binding `SDL_GetWindowSize`, and both exist because both questions get asked: on
a high-DPI display the window's size and the target's pixels genuinely differ, and a program that sized its
geometry from the wrong one draws at half scale.

Behaviour, each a decision a caller can observe:

- `immediate_begin` resets the batch. Calling it twice without a flush **discards** the first, which is what
  Simp does: a begin starts a batch.
- `immediate_quad` outside a batch is a **no-op rather than a trap**. A dropped quad is a visible bug the
  caller can see; a trap would make a mis-ordered draw call kill a program mid-frame, which is worse for the
  thing a renderer is used for.
- **Six vertices per quad, no index list.** `SDL_RenderGeometry` accepts a null index array meaning
  "vertices in order", so two extra vertices per quad buys the removal of the whole index buffer.
- **A full batch flushes and continues**, checked six at a time so a quad is never split across a flush —
  half a quad is a triangle nobody asked for, and harder to diagnose than a whole quad in the next batch.
- **Both shader-mode setters flush an open batch first.** Changing mode mid-batch is the one thing Simp's
  contract forbids: the queued vertices were meant for the previous mode, and drawing them under this one
  would silently draw the wrong thing.
- `immediate_flush` on an **empty** batch returns `true`. A frame that happened to draw nothing must not
  read as a failure.
- `swap_buffers` does **not** flush. A caller who forgot `immediate_flush` sees an empty frame, which is a
  visible bug; an implicit flush would hide the missing call and make the batch's lifetime unreadable.
- The 0..1 → 0-255 conversion is **clamped, in one place**. `cast(u8, 300)` gives 44, so an out-of-range
  channel would come out a *different colour* rather than a saturated one.

### §4 — `Image` produces a `Simp.Texture` and asks for the renderer

`Image` had its own one-field `Texture`. `Simp.Texture` carries `width` and `height` too — because Simp's
does and every caller sizing a quad needs them — so `Image`'s is **deleted** and there is one texture type.
Two would mean a caller converting at every boundary.

The dimensions are filled from the *surface* the texture came from, which already has them: a
`SDL_QueryTexture` round trip would ask SDL for a number we are holding.

And `Image` no longer reads `renderer.handle`. It calls `Simp.current_renderer(simp)`, so the dependency
between the two modules is a **procedure** rather than a struct layout — which means `Simp.Renderer` can
change shape without this file noticing. `Simp.get_error` exposes `SDL_GetError` for the same reason: a null
handle plus a bare `false` was every graphics module's only failure report, and "the BMP is malformed" and
"the file is not there" are different problems.

### §5 — `UI` is migrated, and the outline costs four quads

Forced by §2, not optional: `UI` drew through `Window.set_color`, `fill`, `outline` and `rect`, and all four
are gone. `draw_button` is now one `immediate_begin`/`immediate_flush` pair holding **five quads in two
colours** — a filled body and four thin edge quads, because `SDL_RenderGeometry` draws triangles and has no
line primitive.

That is a real cost of the new backend, written down rather than glossed: one call became five. What it buys
is that the body and the outline are different colours **in one batch**, where `SDL_SetRenderDrawColor`
forced a draw call per colour.

**Every assertion in the UI integration test is unchanged.** Only the helper's types and `main`'s setup and
teardown moved: the same button, clicked the same way, produces the same exit code while every pixel now
goes through `SDL_RenderGeometry`. An unchanged assertion over a replaced backend is the only check that a
migration preserved behaviour rather than merely compiling.

### §6 — What is deferred, with reasons

- **Fonts and text.** Out of scope by the plan, and it stays out: a font needs `SDL_ttf` (a second library's
  version skew) or a bitmap glyph table carried as data. `UI` is still label-less.
- **A GL backend.** A later swap behind an unchanged API, which is the point of having the API.
- **`UI` is not widened.** It keeps its single `button`. Migrating it was forced; growing it was not.

## Consequences

- The graphics stack is five modules that compose, and the composition is a test rather than a claim.
- `jr run` still cannot execute any of it — the comptime VM reaches libc and nothing else — so every check
  here is `jr build` plus running the binary, with `-L` and `SDL_VIDEODRIVER=dummy`.
- **Windows is source-portable and unrun.** `-lSDL2` is the link name there, every binding is a plain C
  function of scalars and pointers, and the widths are now C's. What is untested is whether `clang` on
  Windows resolves `-lSDL2` to `SDL2.lib`, and `jr-vm`'s `libloading::os::unix` means the compiler itself
  cannot host there. Both are named in PLAN rather than claimed here.
- **SDL 2.0.18 is the floor**, because `SDL_RenderGeometry` was added there. An older SDL2 fails to *link*,
  which is the honest failure.
- The `Renderer` and `Events` structs are large enough to matter (20 KB and 14 KB). A caller declares them
  in `main`, not per frame.

## Verification

Six integration tests, all `jr build` then run, all skipping when SDL2 is absent.

- **`the_simp_stack_draws_a_frame_end_to_end` exits 42.** Five modules imported at once, four aliased, with
  `Window` beside `File` — the collision that could not be written before ADR-0179. Each earlier failure
  exits with its own code, so a failure **names the call that broke**.
- **`a_rotated_textured_quad_draws_through_render_geometry` exits 42**, and it is the check that the new API
  expresses something the old one could not: four non-axis-aligned corners with four UVs, which
  `SDL_RenderFillRect` cannot draw at any angle. The texture is built by the program, so no binary fixture
  is in the repository and the *decode* is exercised. It also reads `texture.width`/`.height`, which
  `Image`'s old one-field `Texture` could not carry, and it changes shader mode with a batch open — which
  must flush what it finds rather than draw it in the new mode.
- **`a_window_opens_and_draws_through_sdl2` exits `2047 % 251`**, eleven bits: the batch of two quads in
  different colours, the non-axis-aligned second one, an empty flush succeeding, and the render target's
  size *and* the window's checked separately.
- **`an_event_loop_reads_the_sdl_event_union` exits `4194303 % 251`**, twenty-two bits — the fourteen it had
  plus eight for the per-frame API: a caller-owned `Events`, one drain, then several questions of the same
  frame, which `wants_to_close` cannot answer because it consumes what it reads.
- **`an_immediate_mode_button_fires_on_release_inside`** and **`a_bmp_round_trips_into_a_texture`** keep
  their exit codes exactly.
- The layout claims are asserted by the programs, not by the docs: `Window.RECT_LAYOUT_IS_SDL2`,
  `Input.EVENT_LAYOUT_IS_SDL2` and `Simp.VERTEX_LAYOUT_IS_SDL2`, each a file-scope constant which ADR-0180
  §3 made possible.
- All seven gates green; `jr fmt --check` over every corpus directory and `modules/`; tree-sitter
  regenerated and the whole corpus parsed with no `ERROR` node; Neovim's checks passing.

**One trap worth carrying forward.** `"literal".data` — a field of a string *literal* — does not lower:
*"a memory reference has no place"*. Binding the literal to a local first works, which is what every program
here does and what the pre-existing tests already did. It cost one confused build; it is recorded in PLAN as
its own item.
