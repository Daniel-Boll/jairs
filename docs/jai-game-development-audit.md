# Jai game-development audit

**Status:** primary-source research plus the staged `Game` plan. ADR-0210 decided the six forks and
implemented the lifecycle foundation; `PLAN.md` §7 remains the work queue for later slices.

This audit answers three questions raised while reviewing the games book:

1. What does real Jai game code use?
2. How closely does Jairs' graphics stack match Jai's?
3. Does Jairs need a smaller, raylib-shaped layer above `Simp`?

The short answers are:

- Jai's `Simp` is **OpenGL-backed, not SDL-backed**. Its platform backend creates WGL, GLX,
  NSOpenGL or EGL contexts directly. Jai's `Window_Creation` and `Input` modules provide the
  platform-facing pieces around it.
- Jairs' drawing is also OpenGL, but **SDL2 creates the window, GL context, events and image
  surfaces**. Calling the whole Jairs stack "SDL rendering" is wrong; calling it identical to
  Jai's stack is also wrong.
- Jai exposes a low-level `Window_Creation` + `Input` + `Simp` stack. No built-in raylib-shaped
  facade was found. A community project, `jai-simpler`, exists specifically to add that layer,
  while two other projects bind raylib itself. That is enough evidence to plan a small Jairs-native
  facade, but not an engine.

## 1. Sources and reproducibility

The sources were read at these revisions on 2026-09-06:

| Source | Revision | Why it matters |
|---|---|---|
| [`tsoding/jaibreak`](https://github.com/tsoding/jaibreak/tree/e7e206a66ae3140c5c588feb72591c2d641b0882) | `e7e206a` | A complete game with a native Simp target, a WASM target, text, resizing, generated build input and headless game logic |
| [`tsoding/tetris-jai`](https://github.com/tsoding/tetris-jai/tree/6fd371e38fcbefccdd9cf6d392aebefa6760d1bd) | `6fd371e` | Compact game code using Jai literals, item conditionals and the ordinary Simp/Input loop |
| [`tsoding/ditch`](https://github.com/tsoding/ditch/tree/6b85eea844250d2378ba34b9220328525727ab50) | `6b85eea` | Simp text, UI, window resizing, `#load` and platform conditionals in an application |
| [`tsoding/infinite-pong-jai-wasm64`](https://github.com/tsoding/infinite-pong-jai-wasm64/tree/d3319ec39005da662610e5f66ca894afa04d5f04) | `d3319ec` | A small browser game using computed array lengths, `ifx`, inferred literals and a callback |
| [`tsoding/voronoi-browser`](https://github.com/tsoding/voronoi-browser/tree/2a686f67362e715ce865652c7cf2c2e3db267cec) | `2a686f6` | A vendored Jai `Simp` module, including its platform GL backend |
| [`dbechrd/jai-simpler`](https://github.com/dbechrd/jai-simpler/tree/f46df0bcf960488b942b5c2077170d3948f43baf) | `f46df0b` | A community facade explicitly described as a simpler wrapper for Simp |
| [`JassielL/raylib-for-jai`](https://github.com/JassielL/raylib-for-jai/tree/755d4ed50a7a3ae2da1687c86f3085631c11b7f3) | `755d4ed` | An older direct raylib binding |
| [`ahmedqarmout2/raylib-jai`](https://github.com/ahmedqarmout2/raylib-jai/tree/01429c324e9a6126cea50f73d5e832e8374a64e7) | `01429c3` | A current generated raylib 6 binding with ported examples |
| [`karl-zylinski/learning-jai-by-making-a-game`](https://github.com/karl-zylinski/learning-jai-by-making-a-game/tree/c85677e1ed836e8b885fece71aa5548ea175a6fb) | `c85677e` | A game tutorial using Simp directly and writing its own sprite helpers |

The GitHub account `rexim` was also searched for `.jai` game repositories. Its public Jai code found
in this pass is chiefly Advent of Code and site content; no additional game repository was found.
That is a bounded search result, not a claim that none exists.

Jai is a closed beta, so no public-source survey can prove the complete contents of the current
distribution. Claims below are therefore limited to vendored module source and real call sites.

## 2. What Simp actually is

The vendored `modules/Simp/module.jai` describes itself as a deliberately bounded framework for
programs that are "not too demanding". It exposes immediate drawing, textures, text, render targets
and scissoring while leaving the game loop and application structure to the caller.

Its `#module_parameters` names `NONE`, `SOFTWARE`, `OPENGL` and `METAL`, but the source in this revision
loads an implementation only for OpenGL. `backend/gl.jai` creates:

- WGL on Windows;
- GLX on Linux;
- `NSOpenGLContext` on macOS;
- EGL on Android.

It asks for OpenGL 3.3 and swaps buffers through the native platform API. There is no SDL call in
the vendored module.

Jairs has the same broad separation but not the same backend:

| Concern | Jai source observed | Jairs today |
|---|---|---|
| Window | `Window_Creation`, native per OS | `modules/Window`, SDL2 |
| Events | `Input`, native/platform module | `modules/Input`, SDL2 |
| GL context | Simp's WGL/GLX/NSOpenGL/EGL backend | `SDL_GL_CreateContext` inside `modules/Simp` |
| Drawing | Simp over OpenGL 3.3 | Simp-shaped renderer over OpenGL 2.1 / GLSL 1.20 |
| Image decode | Simp imports `stb_image` family | `modules/Image`, SDL2 BMP surfaces only |
| Text | `Dynamic_Font`, glyph cache and prepared text | absent |

The accurate description is therefore:

> Jairs has a **Simp-shaped subset**. SDL2 supplies platform windows, events, context creation and
> BMP surfaces; OpenGL performs the drawing.

It is not accurate to say either that Jairs draws through SDL or that its public API exactly matches
Jai's.

### Public API differences that matter

The observed Jai module has:

- overloaded window and texture render targets;
- `prepare_window` and multisampling selection;
- text and font loading (`get_font_at_size`, `prepare_text`, `draw_text`,
  `draw_prepared_text`, `release_font`);
- scissor rectangles;
- texture loading from files and memory;
- multiple 2D and 3D `immediate_quad` overloads;
- 3D triangles, normals, tangents and two UV channels;
- per-thread Simp state through `#add_context`;
- OpenGL 3.3 shader and target machinery.

Jairs has:

- one window render target taking `*Window.Window`;
- a numeric coordinate-system default because a named constant cannot be used there;
- separate names where Jai uses overloads, such as `immediate_quad_corners`;
- 2D positions, one colour and one UV channel;
- one process-wide renderer state;
- an extra `destroy_render_target`, needed by its SDL-owned context;
- no text, scissor, texture render target or 3D immediate surface.

The common names make porting recognizable. They do not make the two modules signature-identical.

## 3. What real Jai games build on top

`jaibreak` is the most useful shape to copy into teaching material:

- `game.jai` holds the game rules and platform-neutral drawing requests.
- `simp_platform.jai` adapts those requests to `Window_Creation`, `Input` and `Simp`.
- `wasm_platform.jai` adapts the same game to browser foreign calls.
- `first.jai` constructs separate compiler workspaces and injects generated source and configuration.

The native path creates a window, selects it as the render target, loads a font, drains input,
updates on a measured delta, draws and swaps. It also handles resize records by calling
`Simp.update_window` and reloading the size-dependent font. That is a better tutorial spine than
listing every field in every ABI overlay.

The other projects repeatedly add facilities above Simp:

- sprite drawing from UV rectangles;
- rotated rectangles and circles;
- collision helpers;
- key-held state rather than only events;
- texture caches and stable handles;
- audio;
- fixed ticks and frame timing;
- a single init/update/draw/end-frame loop.

`jai-simpler` puts all of these in one community module. It owns a window, input state, frame and
tick clocks, game modes, audio, a texture cache backed by `Bucket_Array` plus a string table, optional
asset watching, and helpers such as `draw_rect`, `draw_circle` and `immediate_line`.

That project is evidence for the missing layer, not a template to copy wholesale. It has compile-time
feature flags, a global current game mode, hot reload and a memory debugger. A first Jairs facade needs
none of those.

The raylib bindings show the ergonomic target in a smaller vocabulary:

```text
InitWindow
WindowShouldClose
BeginDrawing / EndDrawing
ClearBackground
GetFrameTime
IsKeyDown / IsKeyPressed
LoadTexture / UnloadTexture
DrawTexture
```

Jairs should copy the *size of the first useful loop*, not raylib's C naming or its entire surface.

## 4. Language gaps exposed by game code

This table records constructs observed in the sources above. “Priority” is for game-development
leverage, not a commitment to implement.

| Gap | Evidence in real code | Why a game wants it | Priority |
|---|---|---|---|
| Imported/call-backed array lengths | `infinite-pong` declares dimensions such as `[BOARD_HEIGHT][BOARD_WIDTH]`, where one constant is an expression | Local products and alias chains work through ADR-0245; grids shared across modules still need imported declaration constants, and call-backed values still report E0233 | **P0** |
| Pointer iteration, `for *item` | `jaibreak`, `learning-jai`, `jai-simpler` | Mutating particles, enemies and cached resources without manual indexing | **P0** |
| `ifx` conditional expression | `infinite-pong`, `learning-jai`, raylib examples | Small vector, colour and state expressions without a temporary `var` | **P1** |
| Inferred array literals, `.[…]` | all surveyed games | Struct `.{…}` is present; vector/array-heavy code still repeats element types | **P1** |
| General procedure overloading | Simp and `jai-simpler` overload rectangles, scissor, fonts and quads | One conceptual operation can accept points, rectangles, textures or scalar coordinates | **P1** |
| Item/block `#if` | `jaibreak`, `tetris-jai`, `ditch`, `jai-simpler` | Debug-only declarations and platform-specific imports or bodies | **P1** |
| `#load` | `jaibreak`, `ditch`, generated raylib binding | Split one module across implementation files without creating import namespaces | **P1** |
| `#module_parameters` | Simp and `jai-simpler` | Configure backend or optional subsystems once at import | **P2** |
| `Code` values and for-expansion macros | `jai-simpler` key registration; prior chess audit | Type-safe registration and boilerplate generation | **P2** |
| Named returns | Simp, raylib tooling, application helpers | Documents multi-return meaning and permits direct assignment in a procedure | **P2** |
| Inline procedures | `jai-simpler` and generated bindings | Tiny math/input wrappers without relying entirely on an optimiser | **P2** |
| Per-thread module context, `#add_context` | Simp | Independent renderer state on threads/windows | **P3** until Jairs supports multi-window drawing |
| Compiler workspace composition and import remapping | `jaibreak/first.jai` | Native/WASM builds from the same game, generated source, backend and module substitution | Separate build-system programme |

Two cautions:

1. Jairs already supports typed array literals as `T.[…]`, typed constants, `type_of`, `#code`
   statements and operator overloading. The rows above must not be expanded into claims that those
   broader families are absent.
2. `#if` should not be added merely to spell a value the existing compile-time evaluator can choose.
   The game evidence is strongest for conditional *declarations/imports* and debug-only bodies.

## 5. Library gaps exposed by game code

These are higher-value for games than most syntax work because each removes application boilerplate:

| Gap | Current consequence |
|---|---|
| Held/pressed/released key state and a complete key table | Every game folds SDL events into its own booleans; arrow keys and function keys need local constants |
| Mouse state, wheel and coordinate conversion | `UI` can consume events, but a game-level query API does not exist |
| Text and fonts | Pong and Snake draw score pips; widgets cannot label themselves |
| PNG/JPEG decode | `Image` is BMP-only; most game assets need conversion before use |
| Audio | No standard game sound path exists |
| Float scalar math and 2D helpers | No float `min`/`max`/`clamp`, `lerp`, `atan2`, radians conversion, rotate/perpendicular/angle helpers |
| Screen/world transforms | No `mat4_inverse`, so general unprojection is unavailable |
| Primitive drawing | Rectangles are verbose Simp quads; circles and lines are application code |
| Sprite helper | UV selection, origin, rotation, scale and tint are rebuilt by callers |
| Resource ownership | `Image.load_texture` returns a raw `Simp.Texture`; the caller must remember destruction and cannot use a stable game-facing handle |
| Better graphics errors | Shader/program logs and several deletion bindings are absent from `GL` |

The game facade should hide repetitive composition, but it must not pretend these substrate gaps
are solved. In particular, a `draw_text` facade cannot exist until a real font path exists beneath it.

## 6. Verdict: add a small `Game` facade

The condition for planning a separate facade is met:

- the vendored Jai Simp source deliberately stops at a bounded graphics framework;
- real Jai games write helpers above it;
- `jai-simpler` exists specifically to provide a simpler wrapper;
- raylib bindings are popular enough to maintain broad example ports;
- no built-in Jai module with raylib's init/input/draw/resource loop was found.

This is a source-bounded verdict, not proof about unpublished future Jai builds. ADR-0210 accepted it
for Jairs and implemented the first slice.

### Recommended boundary

Add `modules/Game`, implemented entirely in Jairs over:

```text
Game
├── Window       window lifetime
├── Input        event drain and per-frame state
├── Simp         rendering
├── Image        texture creation/loading
├── Time         delta time
├── Math         vectors and matrices
└── Basic        allocator installation and ordinary memory
```

`Bucket_Array` cannot yet store a generic `Texture_Entry` across this module boundary, and `Map`
cannot yet map strings to texture handles. The first resource registry should therefore be a
purpose-built `[..]Texture_Entry` with generation-tagged integer handles. When cross-module generic
containers become usable for arbitrary element types, it can migrate to the standard containers
without changing the public handle.

### First useful loop

The foundation implemented by ADR-0210 is:

```jr
Game :: #import "Game";

main :: () {
    app, opened := Game.open(960, 540, "Pong");
    if !opened {
        return;
    }
    defer Game.close(*app);

    while Game.begin_frame(*app) {
        dt := Game.delta_time(*app);
        update(dt);

        // Drawing still uses Simp until the primitive slice lands.
        draw_with_simp();
        Game.end_frame(*app);
    }
}
```

The v1 surface should be intentionally small:

- lifetime: `open`, `close`, `begin_frame`, `end_frame`, `should_close`, `delta_time`;
- input: `key_down`, `key_pressed`, `key_released`, mouse position/buttons/wheel;
- drawing: `clear`, rectangle, line, circle, texture and source-rectangle sprite;
- resources: `load_texture`, `unload_texture`, automatic cleanup at `close`;
- coordinates: top-left, y-down, because it matches window input, UI and raylib;
- escape hatch: expose the underlying window through an accessor. Jairs cannot make selected fields
  private, so `App`'s visible fields are an ownership convention rather than a false opacity promise.

Do **not** put game modes, entity storage, collision systems, hot reload, an asset watcher, fixed-tick
policy or global callbacks in v1. Those are framework decisions, while this module's job is to make
the first playable loop short.

### Implementation waves

1. **Foundation — implemented by ADR-0210:** caller-owned `App`, startup/cleanup order, frame timing
   and close handling. It installs no allocator because this slice allocates nothing.
2. **Input state:** key/mouse snapshots and edge transitions, plus a complete named key table.
3. **Primitive drawing:** colour, rectangle, line and circle helpers over Simp batches.
4. **Resources and sprites:** generation-tagged texture handles, BMP load/unload, source rectangles,
   tint, origin, rotation and scale.
5. **Substrate follow-ups:** add text/fonts and PNG beneath the facade. Earlier slices may land and
   use BMP fixtures, but the facade is not advertised as a game-ready v1 before these two ordinary
   asset paths exist.
6. **Tutorial cutover:** once that threshold is met, teach `Game` first; keep a separate “under the
   hood with Simp” chapter for readers who need control.

Each wave needs its own ADR and branch under the repository's normal process.

## 7. Design forks decided by ADR-0210

These choices are now part of the module's contract.

### A. State ownership

- **Chosen: explicit `App` value.** Cleanup is visible and a hidden module singleton does not become
  the public API. Simp still permits only one active App, which Game enforces privately.
- Module-global singleton. Shorter calls and closer to raylib, but it makes tests, multiple windows
  and ownership harder.

### B. Resource identity

- **Chosen for the resource slice: generation-tagged integer `Texture` handle.** The registry owns GPU objects,
  stale handles can be rejected, and internal storage can change later.
- Return raw `Simp.Texture`. Thinnest wrapper, but it does not solve ownership or stale copies.

### C. Coordinate system

- **Chosen: top-left, y-down.** Input coordinates, `UI`, image editors and raylib all agree.
- Preserve Simp's right-handed default. Better for mathematics and existing Pong/Snake world code,
  but every UI/mouse caller converts.

### D. Naming fidelity

- **Chosen: Jairs snake_case concepts:** `open`, `begin_frame`, `draw_texture`.
- Raylib-compatible PascalCase names. Easier to translate raylib tutorials, but imports a C API style
  and promises parity the module will not have.

### E. Loop policy

- **Chosen: expose `delta_time`; teach a fixed-step accumulator separately.** The facade does
  not choose simulation semantics.
- Own fixed ticks inside `Game`. Less boilerplate, but immediately turns the facade into a framework.

### F. First asset formats

- **Chosen: stage the implementation with BMP, but reserve “v1/game-ready” for PNG and text.**
  Foundation, input, primitives and resource ownership can be reviewed independently; the first
  public teaching surface still handles the two assets almost every beginner expects.
- Ship and document a BMP-only v1. Fastest route to a short loop, but it advertises an API whose first
  real project immediately needs external conversion and cannot label a button.
- Block every facade change until PNG and text are complete. The cleanest launch, but it needlessly
  couples reviewable ownership/input work to two separate substrate projects.

## 8. Consequences for the games book

The uncommitted book is technically useful as a reference but is not yet a persuasive tutorial:
chapters open with module internals, copy long signatures and explain ABI overlays before the reader
has drawn anything.

The rewrite should:

1. start with the result and the command that runs it;
2. teach one loop or game idea per chapter;
3. quote only the few lines the reader changes;
4. link to module source/reference pages for signatures and ABI detail;
5. call Jairs' API a **Simp-shaped subset**, never “Jai's exact API”;
6. explain the backend once: SDL2 for platform/context/events/images, OpenGL for rendering;
7. keep headless Pong and Snake as the testing lesson;
8. present the current boilerplate as motivation for `Game`, not as ceremony the reader should admire.

Once `Game` reaches the PNG-and-text threshold, the book should begin with it and teach raw Simp
later. The foundation alone is intentionally not that cutover point, so the current concise Simp
tutorial remains candid and useful.
