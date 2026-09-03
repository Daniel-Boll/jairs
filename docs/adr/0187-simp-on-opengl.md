# ADR-0187: `Simp` and `Window` on Jai's real API, over OpenGL

- **Status:** Accepted
- **Date:** 2026-09-03
- **Deciders:** dboll
- The wave ADR-0182 could not finish. It restructured the graphics modules into Jai's *shape* while
  guessing at its *signatures*; this one replaces the guesses with the declarations.

## Context

### Where the API came from, and why that matters more than the code

The Jai compiler is a closed beta and its `modules/` tree is unpublished. It **is** vendored verbatim
by two open-source Jai applications, `valignatev/hitboxer` and `focus-editor/focus`. Both copies were
read and **diffed against each other**, so a divergence between them is visible rather than silently
inherited — and one exists: Focus changes `draw_text`'s `color` from a `Vector4` to a `u8` colour-map
index, so Focus is not a source for that routine.

Every signature below is therefore **declaration-confirmed**, not inferred from a call site. That
distinction is the whole reason this ADR replaces ADR-0182's design rather than extending it: ADR-0182
was written from Jai's *documentation and examples*, and it got the argument order, the return types,
the coordinate system and the state model wrong — each in a way that compiles.

### What was wrong, measured against the real declarations

| Jairs before | Jai, declaration-confirmed |
|---|---|
| `create_window(title, width, height, flags) -> (Window, bool) #must` | `create_window(width, height, window_name, …) -> Window_Type` |
| `set_render_target(simp, window, flags) -> bool #must` | `set_render_target(window, coords := RIGHT_HANDED)` |
| `clear_render_target(simp, r,g,b,a) -> bool #must` | `clear_render_target(r,g,b,a)` |
| `set_shader_for_color(simp, blend) -> bool #must` | `set_shader_for_color(enable_blend := false)` |
| `immediate_quad(simp, x0,y0,x1,y1, color)` | `immediate_quad(x0,y0,x1,y1, color)` |
| `immediate_flush(simp) -> bool #must` | `immediate_flush()` |
| `swap_buffers(simp)` | `swap_buffers(window, vsync := true)` |
| origin **top-left**, y down | origin **bottom-left**, y up |
| `SDL_RenderGeometry` | OpenGL |
| `MAX_VERTICES :: 1020` | `2400` |

**Two of those are not cosmetic.** The coordinate flip means every quad an ADR-0182 program drew came
out mirrored against what the same source draws in Jai. And the state handle means no Jai program
could be copied here at all.

## Decision

### §1 — The state lives in file-scope globals, which is what the handle was standing in for

Jai's `Simp` declares `#add_context simp: *Immediate_State` and its GL backend keeps `the_gl_context`
as "one process-wide global". Jairs had neither — a file-scope `var` was E0245 — which is *why*
ADR-0182 §1 threaded a `*Renderer`, and it recorded that as a design choice ("the shape they chose is
*better* — two windows can have two renderers").

That claim is withdrawn. It was a rationalisation of a limitation: the reason two renderers looked
like a feature is that one was impossible. ADR-0186 built file-scope globals, so the state is now
where Jai's is and every signature above is Jai's.

**`#add_context` is still owed**, and the difference is exactly thread-locality: two threads drawing
to two windows share this module's state where Jai's would not. No program here does that, and the
remedy is a language feature rather than a shape change.

### §2 — `create_window` takes `width, height, title` and creates a GL-capable window

The order is Jai's, from `Window_Creation/osx.jai:85` cross-checked against `linux.jai` and
`windows.jai`. The title is a `string` and not a `*u8`, also Jai's, so the module copies it into
NUL-terminated temporary storage — which is what Jai's own version does with `temp_c_string`.

**It returns one value.** Dropping `(Window, bool) #must` is a real loss of a good marker, and it goes
anyway: a library whose whole purpose is to match another library's API does not get to improve one
call in isolation. `is_open` is the predicate, and every routine is already safe on a closed window.

`SDL_WINDOW_OPENGL` is **always set**, and there is no flags argument. SDL decides GL capability at
creation time, so `SDL_GL_CreateContext` on a window made without the flag fails *late*, with a
message about the context. Jai's `create_window` has no such flag because its per-OS backends make a
GL-capable window unconditionally, so setting it here is what makes this module behave as Jai's does
rather than an extra knob.

`window_x`/`window_y` default to `0`, which this module reads as **centred**. Jai defaults both to `0`
on macOS and Linux and to `-1` on Windows and documents no rule; a window at the very corner is
nobody's intent, so `0` centres and `1` is available for a caller who means it.

### §3 — The GL context is created by `Simp`, not by `Window`

Jai's `Window_Creation` creates no context; `Simp/backend/gl.jai:backend_init` does. The reasoning
holds independently of Jai: a window is useful without a renderer, and a program that only reads
events should not pay for a GL context.

So `set_render_target` creates the context on its first call, compiles the shaders, and stores both in
globals. It is idempotent, and a second call re-makes the context current and re-reads the render
dimensions — which is what a resize needs, and `update_window` is the name Jai gives that.

### §4 — GL 2.1 with GLSL 1.20, measured

Jai's backend asks for GL 3.3. This asks for the default, which on macOS is **2.1 / GLSL 1.20**,
Metal-backed — measured with a `cc`-compiled probe (`GL_VERSION = "2.1 Metal - 90.5"`), not assumed.

**3.3 was rejected for one reason**: on macOS a 3.3 context is *core profile only*, which removes the
compatibility features this module does not use but which a texture path might, and 2.1 is available
on every target this library claims. A shader-based renderer needs GL 2.0, so 2.1 is enough.

The renderer is a vertex buffer, two shaders and `glDrawArrays(GL_TRIANGLES, …)` — six vertices per
quad and **no index list**, which is what Jai's does. One fragment shader with a `textured` uniform
rather than two programs, because switching a uniform is cheaper than switching a program and this
module has exactly two modes; Jai has separate shaders and more modes.

### §5 — The trap worth the most: a Jairs `string` has no NUL, and `glShaderSource` wants one

`glShaderSource(shader, 1, &source, null)` makes GL read each string **to a NUL**. A Jairs `string` is
`{data, count}` with no terminator, so the shaders compiled from whatever followed the text in memory,
`GL_COMPILE_STATUS` was 0, and **`glGetError` said `GL_NO_ERROR`**. A silent failure with a clean error
code, and identical C code succeeded — which is what proved it was this side.

The fix is to pass the length explicitly, and it is *correct* rather than a workaround: the length is
known. `GL.set_shader_source` takes a `string` and does it, so no caller can get it wrong once.

### §6 — One honest cosmetic defect, recorded rather than hidden

The fragment shader declares a `sampler2D` that is unread when `textured == 0`, and macOS logs
*"UNSUPPORTED (log once): unit 0 GLD_TEXTURE_INDEX_2D is unloadable and bound to sampler type
(Float)"* on the first flat-colour draw. It is a driver notice, logged once, and the sampler is never
sampled.

It is **not** silenced, and the alternative is named: binding a 1×1 white texture as a default would
remove the message and cost a texture upload at startup. That is what a mature renderer does, and it
is deferred rather than done because the message is honest about what the shader contains.

## Consequences

- A Jai program that draws can be copied here and compiles: `create_window(320, 240, "demo")`,
  `set_render_target(*w)`, `clear_render_target(…)`, `set_shader_for_color(true)`,
  `immediate_quad(…)`, `immediate_flush()`, `swap_buffers(*w)`.
- **`modules/UI` and `modules/Image` had to move with it**, and the coordinate flip is the sharp edge
  for `UI`: a hit test and a draw that disagree is worse than a picture that is upside down.
- **ADR-0182 §1's claim that a caller-owned renderer is the better API is withdrawn**, with the reason:
  it was a limitation described as a choice.
- Text and fonts stay out, with the same reason as ADR-0182: a font needs `stb_truetype` or a bitmap
  glyph table, which is a module rather than a routine.
- No 3D overloads. Jai has six `immediate_quad`s taking `Vector3`; Jairs has no overloading, so the 2D
  forms exist under distinct names and `immediate_quad_corners` is the four-corner one.

## Verification

- A Jai-shaped program **builds, links and draws**: two quads through GLSL shaders, `glGetError` clean
  after the flush and after the swap, exit 63 over six independent bits.
- The GL context path is proven separately: SDL init, a GL-capable window, `SDL_GL_CreateContext`,
  `SDL_GL_MakeCurrent`, a real `glClear`, and `SDL_GL_SwapWindow` — exit 63, and it fails at the window
  under `SDL_VIDEODRIVER=dummy`, which is how the tests know the dummy driver has no GL.
- Shader compilation is proven against **C**: the identical shader source compiled in a `cc` program
  and failed from Jairs until the length was passed, which is what located the NUL problem.
- All seven gates green.
