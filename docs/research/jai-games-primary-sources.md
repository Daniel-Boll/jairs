# Jai game-development code: primary-source findings

Date: 2026-09-06

## Scope and method

This report reads public Jai source code directly. It does not treat articles, videos, or
generated summaries as evidence. Repository state is pinned to a commit SHA wherever possible;
links below are immutable `blob/<sha>/...` URLs.

The public repositories do **not** establish what the current private Jai distribution ships.
Copies of `Simp`, `Input`, and `Window_Creation` in public projects are vendored or forked
snapshots. This report therefore distinguishes:

- **Declaration evidence:** a public source file contains the declaration.
- **Call-site evidence:** a project calls a procedure, but its declaration is not public there.
- **Inference:** a conclusion drawn from several facts, explicitly labelled as such.

## Findings in brief

### Facts

1. Real Jai programs commonly use a direct frame loop:
   create a window, make it a `Simp` render target, drain `Input`, update state, select a
   shader, issue immediate draws, and swap buffers. Jaibreak, Tetris, Ditch, and
   Voronoi Browser all have this shape.
2. Jaibreak isolates gameplay from its platform adapters. The same game code is paired with
   a desktop `Window_Creation`/`Input`/`Simp` adapter and a browser adapter made of `#foreign`
   host calls.
3. In the inspected vendored Jai stack, **`Window_Creation` does not use SDL**. It uses native
   Windows, macOS, and Linux display paths. `Input` reads those native platform events.
   `Simp` uses OpenGL and creates/manages the GL context in its GL backend.
4. SDL is nevertheless used by other Jai engines. The inspected Vulkan engine uses SDL for
   its window and events, proving that SDL is an ecosystem choice, not the implementation
   behind the inspected `Window_Creation`/`Simp` stack.
5. Public `Simp` copies disagree on signatures. In particular, render-target coordinates,
   `immediate_begin`, font construction, and text colour differ. “Jai's exact Simp API”
   is not a stable claim without naming a source revision.
6. Public Jai games use more than drawing primitives: fonts/text, textures, audio, resize
   events, held input state, build workspaces, resource copying/generation, and platform
   adapters are normal project concerns.
7. Public Jai games also use a Raylib-shaped API through community bindings. That establishes
   demand for the style, but it does **not** establish Raylib as a module shipped with Jai.

### Inference

Jairs should separate two goals:

- **Compatibility layer:** a revision-pinned `Window_Creation`/`Input`/`Simp` surface.
- **Ergonomic game layer:** a small state-owning facade, implemented on Jairs' own
  `Window`/`Input`/`Simp`/resource modules, with calls such as window initialization,
  `begin_frame`/`end_frame`, held-key queries, texture loading, text, and sound.

Calling the second layer `Simp` would conflate two different APIs. Calling it `Raylib` would
imply compatibility that should only be claimed if intentionally tested. A neutral name such
as `Game` or `Simple_Game` is safer.

## Source inventory

| Repository | Pinned commit | What was inspected |
|---|---|---|
| [tsoding/jaibreak](https://github.com/tsoding/jaibreak/tree/e7e206a66ae3140c5c588feb72591c2d641b0882) | `e7e206a66ae3140c5c588feb72591c2d641b0882` | Desktop and WASM adapters, shared gameplay, build workspaces |
| [tsoding/tetris-jai](https://github.com/tsoding/tetris-jai/tree/6fd371e38fcbefccdd9cf6d392aebefa6760d1bd) | `6fd371e38fcbefccdd9cf6d392aebefa6760d1bd` | Compact `Window_Creation`/`Input`/`Simp` game loop |
| [tsoding/ditch](https://github.com/tsoding/ditch/tree/6b85eea844250d2378ba34b9220328525727ab50) | `6b85eea844250d2378ba34b9220328525727ab50` | Real-time loop, fonts/text, threads/network integration |
| [tsoding/voronoi-browser](https://github.com/tsoding/voronoi-browser/tree/2a686f67362e715ce865652c7cf2c2e3db267cec) | `2a686f67362e715ce865652c7cf2c2e3db267cec` | Real-time graphical app and an older vendored `Simp` copy |
| [tsoding/infinite-pong-jai-wasm64](https://github.com/tsoding/infinite-pong-jai-wasm64/tree/d3319ec39005da662610e5f66ca894afa04d5f04) | `d3319ec39005da662610e5f66ca894afa04d5f04` | Browser-hosted callback loop and WASM build |
| [focus-editor/focus](https://github.com/focus-editor/focus/tree/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30) | `7f9a2420068bcd34e0e86163dff8b2f8f5f08b30` | Current public vendored `Window_Creation`, `Input`, and `Simp` declarations/backends |
| [valignatev/hitboxer](https://github.com/valignatev/hitboxer/tree/285bca220b8778b6829f301a4d219779f22d6ea6) | `285bca220b8778b6829f301a4d219779f22d6ea6` | Another vendored/forked `Simp`; custom platform/input layer |
| [karl-zylinski/learning-jai-by-making-a-game](https://github.com/karl-zylinski/learning-jai-by-making-a-game/tree/c85677e1ed836e8b885fece71aa5548ea175a6fb) | `c85677e1ed836e8b885fece71aa5548ea175a6fb` | Tutorial-sized game with assets, animation, raw GL settings, and input state |
| [danieltan1517/chess-jai](https://github.com/danieltan1517/chess-jai/tree/ab5793b8a0e74bc49c88c924a4f050f0b8665b2d) | `ab5793b8a0e74bc49c88c924a4f050f0b8665b2d` | Large game/UI loop, resize handling, audio call sites, build variants |
| [Breush/lava-jai](https://github.com/Breush/lava-jai/tree/b27ef37d80472c2697b71063044dc5311b062203) | `b27ef37d80472c2697b71063044dc5311b062203` | Custom native-window/Vulkan engine with fixed update |
| [ostef/Vk-Engine](https://github.com/ostef/Vk-Engine/tree/53add015f68cbd6d3cddd950a7fb5b0a95f358aa) | `53add015f68cbd6d3cddd950a7fb5b0a95f358aa` | SDL window/events, Vulkan renderer, dynamic game modules |
| [xyaman/breakout](https://github.com/xyaman/breakout/tree/d60f5fe26fc924f6d6ea8a5a67f4ebdc87ab06c4) | `d60f5fe26fc924f6d6ea8a5a67f4ebdc87ab06c4` | GLFW/OpenGL game and direct C-binding style |
| [marvhus/jai-games](https://github.com/marvhus/jai-games/tree/55ff563888224378025d9c2e73072fc3ad56fc3a) | `55ff563888224378025d9c2e73072fc3ad56fc3a` | Raylib-shaped Pong |
| [ahmedqarmout2/raylib-jai](https://github.com/ahmedqarmout2/raylib-jai/tree/01429c324e9a6126cea50f73d5e832e8374a64e7) | `01429c324e9a6126cea50f73d5e832e8374a64e7` | Generated community Raylib bindings and examples |
| [Grouflon/raylib-jai](https://github.com/Grouflon/raylib-jai/tree/5270cc65db74ffe780d6cd2051b39559cbc8320d) | `5270cc65db74ffe780d6cd2051b39559cbc8320d` | Independent community Raylib binding; inventory corroboration |

The tsoding repositories above are the game-relevant `.jai` repositories found in the sampled
public search. The `rexim` account did not add another representative public game in that
sample; this is not an exhaustive claim about every historical branch or deleted repository.

## 1. Actual game-loop and module syntax

### 1.1 Jaibreak: one game, two platform adapters

Jaibreak's build entry imports compiler/build modules and loads project fragments:
[`first.jai:3-11`](https://github.com/tsoding/jaibreak/blob/e7e206a66ae3140c5c588feb72591c2d641b0882/first.jai#L3-L11).
It creates separate native and WASM workspaces, including release/debug variants:
[`first.jai:81-184`](https://github.com/tsoding/jaibreak/blob/e7e206a66ae3140c5c588feb72591c2d641b0882/first.jai#L81-L184).
The file-level `#run` invokes those build procedures:
[`first.jai:210-219`](https://github.com/tsoding/jaibreak/blob/e7e206a66ae3140c5c588feb72591c2d641b0882/first.jai#L210-L219).

The desktop adapter imports `Window_Creation`, `Input`, and `Simp`, then performs the expected
loop: window creation, render-target setup, font loading, time delta, event draining, resize
handling, clear/update/render, and swap:
[`src/simp_platform.jai:114-203`](https://github.com/tsoding/jaibreak/blob/e7e206a66ae3140c5c588feb72591c2d641b0882/src/simp_platform.jai#L114-L203).

The browser adapter instead declares host operations with `#foreign`; its `main` initializes
the game while the host owns frame scheduling:
[`src/wasm_platform.jai:1-26`](https://github.com/tsoding/jaibreak/blob/e7e206a66ae3140c5c588feb72591c2d641b0882/src/wasm_platform.jai#L1-L26).

**Fact:** gameplay is shared; platform, rendering, text, and frame scheduling are adapters.

**Inference for Jairs:** this is a stronger example architecture than binding the rules
directly to SDL or OpenGL. A Jairs tutorial can keep a plain `update`/`render` game module and
show native and future browser drivers independently.

### 1.2 Small desktop loops use the same explicit stack

Tetris imports `Basic`, `Window_Creation`, `Input`, `Math`, and `Simp`, then creates a window,
sets a render target, drains events, draws immediate quads, and swaps:
[`tetris.jai:187-252`](https://github.com/tsoding/tetris-jai/blob/6fd371e38fcbefccdd9cf6d392aebefa6760d1bd/tetris.jai#L187-L252).

Ditch uses the same explicit loop while adding text/UI and asynchronous work:
[`main.jai:63-193`](https://github.com/tsoding/ditch/blob/6b85eea844250d2378ba34b9220328525727ab50/main.jai#L63-L193).
Its font/text setup is visible at
[`main.jai:195-283`](https://github.com/tsoding/ditch/blob/6b85eea844250d2378ba34b9220328525727ab50/main.jai#L195-L283)
and font loading at
[`main.jai:450-462`](https://github.com/tsoding/ditch/blob/6b85eea844250d2378ba34b9220328525727ab50/main.jai#L450-L462).

Voronoi Browser handles resizing, mouse input, wheel input, text, drawing, and swapping in one
loop:
[`main.jai:351-446`](https://github.com/tsoding/voronoi-browser/blob/2a686f67362e715ce865652c7cf2c2e3db267cec/main.jai#L351-L446).

These are not engine callbacks. The application owns the `while` loop and calls the platform
and renderer modules directly.

### 1.3 Callback loops also occur, especially for WASM

Infinite Pong declares browser-host drawing and frame-scheduling functions with `#foreign`.
Its `update_frame(dt)` updates and renders, and `main` registers that callback:
[`main.jai`](https://github.com/tsoding/infinite-pong-jai-wasm64/blob/d3319ec39005da662610e5f66ca894afa04d5f04/main.jai).
Its build file targets LLVM/WASM:
[`first.jai`](https://github.com/tsoding/infinite-pong-jai-wasm64/blob/d3319ec39005da662610e5f66ca894afa04d5f04/first.jai).

**Inference:** an ergonomic Jairs game layer should not require ownership of the outer loop.
Expose frame operations that work both inside `while` and inside a host callback.

## 2. Exact public declarations: `Window_Creation`, `Input`, and `Simp`

The declarations in this section come from Focus commit
`7f9a2420068bcd34e0e86163dff8b2f8f5f08b30`. They are exact for that vendored copy, not a
claim about every Jai beta.

### 2.1 `Window_Creation`

The Linux declaration is:

```jai
create_window :: (
    width: int,
    height: int,
    window_name: string,
    window_x := 0,
    window_y := 0,
    parent := INVALID_WINDOW,
    background_color_rgb := DEFAULT_WINDOW_CREATION_COLOR
) -> Window_Type
```

Source:
[`modules/Window_Creation/linux.jai:8-10`](https://github.com/focus-editor/focus/blob/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30/modules/Window_Creation/linux.jai#L8-L10).

The macOS and Windows files expose the same leading `width, height, window_name` shape but have
platform-specific parent/default/MSAA details:

- [macOS declaration and native window creation](https://github.com/focus-editor/focus/blob/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30/modules/Window_Creation/osx.jai#L85-L115)
- [Windows declaration and `CreateWindowExW` path](https://github.com/focus-editor/focus/blob/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30/modules/Window_Creation/windows.jai#L2-L48)
- [platform dispatch](https://github.com/focus-editor/focus/blob/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30/modules/Window_Creation/module.jai)

### 2.2 `Input`

The public module state includes:

```jai
events_this_frame: [..] Event;
input_button_states: ...;
input_application_has_focus: bool;
```

The frame API includes:

```jai
update_window_events :: ();
get_window_resizes :: () -> [] Window_Resize_Record;
get_window_moves :: () -> [] Window_Move_Record;
```

The event declarations and public state are in
[`modules/Input/module.jai:153-230`](https://github.com/focus-editor/focus/blob/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30/modules/Input/module.jai#L153-L230);
per-frame reset/cleanup is in
[`module.jai:241-262`](https://github.com/focus-editor/focus/blob/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30/modules/Input/module.jai#L241-L262).

**Fact:** this API owns global frame state and exposes a no-argument event update. That differs
from Jairs' caller-owned `Events` buffer.

### 2.3 `Simp`

Focus's public copy defaults to an OpenGL renderer:

```jai
render_api := Render_API.OPENGL;
```

Its public surface includes these forms:

```jai
set_render_target :: (window: Window_Type);
update_window :: (window: Window_Type);
get_render_dimensions :: (window: Window_Type) -> (...);
set_shader_for_color :: (enable_blend := false, $extra_draw_info := Extra_Draw_Info.{});
set_shader_for_images :: (texture: *Texture);
clear_render_target :: (...);
swap_buffers :: (window: Window_Type, vsync := true);
immediate_flush :: ();
```

There are overloaded `immediate_quad` declarations. This Focus copy does not contain
`immediate_begin`.

Source files:

- [public Simp module](https://github.com/focus-editor/focus/blob/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30/modules/Simp/module.jai)
- [immediate drawing declarations](https://github.com/focus-editor/focus/blob/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30/modules/Simp/immediate.jai)

Its current public font entry point is:

```jai
get_font_at_size :: (name: string, font_data: string, pixel_height: int) -> *Dynamic_Font
```

Text is prepared and then drawn with a colour-map index (`u8`) in this copy:
[font source](https://github.com/focus-editor/focus/blob/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30/modules/Simp/font.jai).

## 3. Does Jai's `Simp` use SDL and OpenGL?

### Facts from the inspected public stack

- `Window_Creation` selects native Windows, Linux, or macOS implementations:
  [dispatch source](https://github.com/focus-editor/focus/blob/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30/modules/Window_Creation/module.jai).
- Windows calls Win32 window APIs:
  [`windows.jai:2-48`](https://github.com/focus-editor/focus/blob/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30/modules/Window_Creation/windows.jai#L2-L48).
- macOS creates an `NSWindow` and OpenGL view:
  [`osx.jai:85-115`](https://github.com/focus-editor/focus/blob/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30/modules/Window_Creation/osx.jai#L85-L115).
- Linux delegates through its native Linux display layer rather than SDL:
  [`linux.jai:8-10`](https://github.com/focus-editor/focus/blob/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30/modules/Window_Creation/linux.jai#L8-L10).
- `Input` has platform implementations over Cocoa, Linux display events, and Win32:
  [Input module tree](https://github.com/focus-editor/focus/tree/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30/modules/Input).
- `Simp/backend/gl.jai` creates a WGL, Linux-display GL, or NSGL context and sets a minimum
  OpenGL version:
  [`backend/gl.jai:7-40`](https://github.com/focus-editor/focus/blob/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30/modules/Simp/backend/gl.jai#L7-L40).
  It clears with OpenGL and presents with the platform swap operation:
  [full GL backend](https://github.com/focus-editor/focus/blob/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30/modules/Simp/backend/gl.jai).

### Conclusion

For this public vendored stack: **no SDL for platform/window/input; OpenGL for `Simp`
rendering.**

That conclusion must not be generalized to all Jai game code. Vk-Engine creates an
`SDL_WINDOW_VULKAN` window, polls SDL events, and renders through Vulkan:
[`Source/Core/main.jai`](https://github.com/ostef/Vk-Engine/blob/53add015f68cbd6d3cddd950a7fb5b0a95f358aa/Source/Core/main.jai).

Jairs using SDL2 underneath its compatibility-shaped modules is therefore a valid portability
choice, but it is not implementation parity with this public Jai stack.

## 4. API drift and stale/forked evidence

The public copies disagree:

| Area | Focus `7f9a...` | Hitboxer `285b...` | Voronoi Browser `2a686...` |
|---|---|---|---|
| `set_render_target` | Window only | Includes coordinate-system form | Includes coordinate-system form |
| `immediate_begin` | Absent | Present | Present |
| Text colour | `u8` colour-map index | `u8` colour-map index | `Vector4` colour |
| Font construction | Name + font bytes + pixel height | Project copy differs | Path/name-style form |
| Platform layer | `Window_Creation` + `Input` | Custom JDL/input/window | `Window_Creation` + `Input` |

Evidence:

- [Focus Simp](https://github.com/focus-editor/focus/tree/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30/modules/Simp)
- [Hitboxer Simp](https://github.com/valignatev/hitboxer/tree/285bca220b8778b6829f301a4d219779f22d6ea6/modules/Simp)
- [Voronoi Browser Simp](https://github.com/tsoding/voronoi-browser/tree/2a686f67362e715ce865652c7cf2c2e3db267cec/modules/Simp)

Both older copies contain an effectively no-op `immediate_begin`:

- [Hitboxer `immediate.jai:84-86`](https://github.com/valignatev/hitboxer/blob/285bca220b8778b6829f301a4d219779f22d6ea6/modules/Simp/immediate.jai#L84-L86)
- [Voronoi Browser `immediate.jai:84-86`](https://github.com/tsoding/voronoi-browser/blob/2a686f67362e715ce865652c7cf2c2e3db267cec/modules/Simp/immediate.jai#L84-L86)

**Fact:** Jairs' current surface most closely resembles the coordinate-system/
`immediate_begin` family, not the pinned Focus copy:

- `set_render_target(window, coords)`:
  [`modules/Simp/module.jr:159`](../../modules/Simp/module.jr#L159)
- `immediate_begin`:
  [`modules/Simp/module.jr:254`](../../modules/Simp/module.jr#L254)
- `swap_buffers(window, vsync)`:
  [`modules/Simp/module.jr:342`](../../modules/Simp/module.jr#L342)

**Inference:** documentation should say “compatible with the `<repository>/<commit>` Simp
surface” instead of “Jai's real/exact API.” The current game documentation makes the stronger
claim at
[`games/index.md:27-29`](../../docs-site/src/content/docs/games/index.md#L27-L29) and
[`games/drawing-with-simp.md:8-15`](../../docs-site/src/content/docs/games/drawing-with-simp.md#L8-L15).

## 5. Window, input, render, audio, and resource APIs used in practice

### Window/input/render

The recurring native shape is:

```jai
window := create_window(width, height, title);
Simp.set_render_target(window, ...);

while !quit {
    update_window_events();
    for event: events_this_frame {
        // close, key, mouse, wheel, resize
    }

    Simp.clear_render_target(...);
    Simp.set_shader_for_color(...);
    Simp.immediate_quad(...);
    Simp.immediate_flush();
    Simp.swap_buffers(window);
}
```

The exact details vary by `Simp` revision, but this ownership pattern is stable across the
inspected tsoding projects.

### Text/fonts

Text is not an edge feature:

- Jaibreak loads a font and prepares/draws text in its desktop adapter:
  [`src/simp_platform.jai:114-203`](https://github.com/tsoding/jaibreak/blob/e7e206a66ae3140c5c588feb72591c2d641b0882/src/simp_platform.jai#L114-L203).
- Ditch has explicit font loading and text drawing:
  [`main.jai:195-283`](https://github.com/tsoding/ditch/blob/6b85eea844250d2378ba34b9220328525727ab50/main.jai#L195-L283).
- Focus's vendored `Simp` has `Dynamic_Font`, text preparation, and draw calls:
  [`modules/Simp/font.jai`](https://github.com/focus-editor/focus/blob/7f9a2420068bcd34e0e86163dff8b2f8f5f08b30/modules/Simp/font.jai).

Jairs currently has no font/text procedures in `modules/Simp`.

### Textures/assets

The learning-game project loads textures from files, sets texture filtering through raw GL,
maintains sprite/animation state, and draws textured quads:
[`main.jai`](https://github.com/karl-zylinski/learning-jai-by-making-a-game/blob/c85677e1ed836e8b885fece71aa5548ea175a6fb/main.jai).

This is representative of actual tutorial code: asset loading and lifetime are introduced
alongside the loop, not after an exhaustive renderer implementation tour.

### Audio

Chess-jai provides call-site evidence for the Jai audio stack:

```jai
Sound.load_audio_file("resources/move.wav");
Sound.sound_player_init(.{});
Sound.make_stream(...);
Sound.start_playing(...);
Sound.update(...);
```

The loop and audio use are at
[`main.jai:1038-1152`](https://github.com/danieltan1517/chess-jai/blob/ab5793b8a0e74bc49c88c924a4f050f0b8665b2d/main.jai#L1038-L1152)
and
[`main.jai:2211-2250`](https://github.com/danieltan1517/chess-jai/blob/ab5793b8a0e74bc49c88c924a4f050f0b8665b2d/main.jai#L2211-L2250).

This is **call-site evidence**, not an exact public declaration of `Sound_Player` or
`Wav_File`. Jairs currently has no audio module.

## 6. Project and build structure

Observed structures include:

1. **Single-file game:** Tetris; suitable while assets and platform variation are small.
2. **Shared game + platform adapters:** Jaibreak; native Simp and browser/WASM adapters.
3. **Compiler workspace build file:** Jaibreak, Chess, and the Raylib examples create
   workspaces, select targets/options, add source/build strings, and produce multiple artifacts.
4. **Custom engine modules:** Lava and Vk-Engine separate platform, renderer, input, assets,
   and game code. Vk-Engine also builds dynamic modules.
5. **Generated/bound C facade:** raylib-jai exposes a flat C-shaped API and links platform
   libraries from a module.

Jaibreak's `first.jai` is the clearest primary source for a multi-target build:
[`first.jai:81-219`](https://github.com/tsoding/jaibreak/blob/e7e206a66ae3140c5c588feb72591c2d641b0882/first.jai#L81-L219).

## 7. Language features exercised, and Jairs impact

### Confirmed Jairs gaps exposed by the sources

| Jai feature seen in game/library source | Jairs state | Practical impact |
|---|---|---|
| Procedure overloading (`immediate_quad` and other library families) | Absent | Jairs needs suffixed names or a smaller selected subset. |
| Iterate by reference, `for *item: items` | Absent | Asset/entity updates require indexing and taking addresses manually. |
| `#add_context` for per-thread library state | Absent | Jairs `Simp` state is process-wide rather than context/thread-local. |
| `Code` as a value for metaprogramming | Refused in current plan | Reusable custom iteration/transformation patterns cannot be ported directly. |
| Cross-file polymorphic procedure instantiation | Refused as E0268 | Generic containers/helpers need concrete wrappers in importing projects. |

Jairs evidence:

- iteration and overloading limits:
  [`docs/capabilities.md:75-91`](../../docs/capabilities.md#L75-L91)
- polymorphic cross-file limit:
  [`docs/capabilities.md:108`](../../docs/capabilities.md#L108)
- `#add_context` gap:
  [`docs/capabilities.md:87`](../../docs/capabilities.md#L87)

### Features observed but already substantially present in Jairs

The projects make heavy use of struct literals, `#run`, polymorphic procedures/types, `#expand`,
`#c_call`, multiple returns, file-scope state, typed enums, fixed/dynamic arrays, and compiler-driven
builds. These should not be reported wholesale as missing: Jairs implements substantial
versions of them. Portability still needs per-program testing because edge semantics and
cross-file restrictions differ.

### Features requiring a separate parity probe

Real sources also use forms such as `#load`, `#complete`, `#through`, `#char`, mutation during
iteration (`remove it`), module parameters, and richer compiler workspace options. This audit
records their use but does not claim a current Jairs result for every form; each needs a
minimal parser/sema/build probe before becoming a roadmap item.

## 8. Jairs API comparison

### Current Jairs compatibility-shaped surface

Jairs currently exposes:

```jr
Window.create_window(width, height, title, x, y) -> Window
Simp.set_render_target(*window, coords)
Simp.update_window(*window)
Simp.get_render_dimensions(*window)
Simp.clear_render_target(...)
Simp.set_shader_for_color(...)
Simp.set_shader_for_images(*texture)
Simp.immediate_begin()
Simp.immediate_quad(...)
Simp.immediate_quad_corners(...)
Simp.immediate_flush()
Simp.swap_buffers(*window, vsync)
```

Declarations:

- [`modules/Window/module.jr:173-229`](../../modules/Window/module.jr#L173-L229)
- [`modules/Simp/module.jr:159-355`](../../modules/Simp/module.jr#L159-L355)

Jairs input instead uses:

```jr
update_window_events :: (events: *Events, limit: s64) -> s64
events_this_frame :: (events: *Events) -> []Event
```

Declarations:
[`modules/Input/module.jr:229-292`](../../modules/Input/module.jr#L229-L292).

This is a valid caller-owned design, but it is not the exact Focus `Input` shape, whose public
copy has module-owned `events_this_frame` and `update_window_events :: ()`.

### Highest-impact library gaps for real games

1. **Text/font rendering.** Used directly by Jaibreak, Ditch, Focus, and Chess.
2. **Held and edge input queries.** Real projects maintain `down`/`pressed`/`released` state;
   Jairs currently makes each caller fold raw events.
3. **Resize, wheel, text-input, and fuller key/gamepad coverage.**
4. **Audio.** Chess and Raylib examples establish ordinary game demand.
5. **Common image formats.** Jairs' `Image` is BMP-only; public projects commonly load ordinary
   assets rather than synthesizing surfaces.
6. **2D math conveniences.** Float clamp/min/max, `atan2`, scalar lerp, angle helpers,
   `Vector2i`, rectangle/circle collision, and projection/reflection reduce per-game boilerplate.
7. **A frame/resource facade.** Current examples expose allocator setup, SDL/OpenGL failure
   plumbing, event storage, and teardown before a newcomer can draw a moving object.

## 9. Is a Raylib-like facade appropriate?

### Primary-source facts

`marvhus/jai-games` uses a Raylib-shaped Pong loop with:

```jai
InitWindow(...);
SetTargetFPS(...);
LoadFont(...);
while !WindowShouldClose() {
    dt := GetFrameTime();
    if IsKeyDown(...) { ... }
    BeginDrawing();
    ClearBackground(...);
    DrawRectangle(...);
    DrawTextEx(...);
    EndDrawing();
}
CloseWindow();
```

Source:
[`pong/main.jai:176-195`](https://github.com/marvhus/jai-games/blob/55ff563888224378025d9c2e73072fc3ad56fc3a/pong/main.jai#L176-L195).

The public `raylib-jai` repository describes generated Jai bindings, supported platforms, and
Raylib versioning:
[`README.md:1-20`](https://github.com/ahmedqarmout2/raylib-jai/blob/01429c324e9a6126cea50f73d5e832e8374a64e7/README.md#L1-L20).
Its minimal example shows the same flat frame API:
[`README.md:44-74`](https://github.com/ahmedqarmout2/raylib-jai/blob/01429c324e9a6126cea50f73d5e832e8374a64e7/README.md#L44-L74).
The module loads generated Raylib/Raymath/RLGL/Raygui bindings and platform libraries:
[`raylib/module.jai`](https://github.com/ahmedqarmout2/raylib-jai/blob/01429c324e9a6126cea50f73d5e832e8374a64e7/raylib/module.jai).

An independent binding also exists:
[Grouflon/raylib-jai](https://github.com/Grouflon/raylib-jai/tree/5270cc65db74ffe780d6cd2051b39559cbc8320d).

### Inference and recommendation

A Raylib-like facade is appropriate as a **Jairs ergonomic layer**, not as a claim about Jai's
`Simp` API. It should own the repetitive state that current Jairs examples expose:

- one `Window.Window`;
- one `Input.Events` buffer plus held/pressed/released tables;
- current render target and frame timing;
- texture/font/sound handles and teardown order.

A source-backed first slice is:

```text
init_window / close_window / window_should_close
begin_frame / clear_background / end_frame
frame_time / optional target_fps
key_down / key_pressed / key_released
mouse_position / mouse_button_down / wheel_delta
draw_rectangle / draw_circle / draw_text
load_texture / unload_texture / draw_texture_region
load_sound / play_sound / unload_sound
screen_width / screen_height / resized
```

Implement it in Jairs over Jairs' `Window`, `Input`, `Simp`, `Image`, future font, and future
audio modules. Do not bind Raylib internally unless the goal changes to actual Raylib
compatibility.

## 10. Brief audit of the pre-rewrite Jairs tutorials

This section records the working-tree draft as it existed before ADR-0208. The current pages were
rewritten in response, so their line numbers and ordering intentionally no longer match this audit.

### Facts

The tutorial draft led with six-module architecture, foreign-library resolution, linker paths and
comptime-VM constraints before showing a minimal playable loop.

The game-loop chapter spent its first ninety lines on clock API and implementation detail before
reaching either loop.

The window chapter moved from opening a window into ABI layout and SDL union overlays.

The Simp chapter explained GL-context birth, shader compilation, backing-store dimensions and
batch-flush invariants before the first complete frame.

### Inference

Those details are useful reference material, but their order is unlike the inspected game
sources and makes the tutorial read as an implementation audit rather than a path to a game.
A source-aligned rewrite should:

1. Put one complete, runnable, moving-object loop first.
2. Introduce `dt`, event folding, and teardown only at their first use.
3. Put SDL ABI, foreign-library lookup, GL context ownership, and VM constraints in
   “why/under the hood” sidebars or reference pages.
4. Add text and one loaded asset as soon as the APIs exist; real projects reach them early.
5. Keep the excellent headless simulation split, but present it after the first visible result.
6. Replace unqualified “Jai's exact API” wording with a pinned source revision and a short drift
   note.

## Unresolved questions

1. Which Jai beta and which `Simp` revision should Jairs intentionally target?
2. Is there a publicly distributable, canonical Jai module snapshot newer than the inspected
   vendored copies?
3. What are the exact declarations and ownership rules of the current `Sound_Player`,
   `Wav_File`, and `Gamepad` modules? Chess provides call sites but not authoritative sources.
4. Which Focus/Hitboxer/Voronoi API differences are upstream evolution versus project-local
   forks?
5. Should Jairs preserve its caller-owned input buffer as the low-level API and add a
   module-owned facade, or add compatibility globals directly to `Input`?
6. Should the ergonomic facade imitate Raylib names exactly, or use a smaller Jairs-native
   vocabulary to avoid a compatibility promise?
7. Which unverified language forms used by the projects (`#load`, `#complete`, `#through`,
   module parameters, `remove it`) fail in current Jairs, and at which compiler phase?
