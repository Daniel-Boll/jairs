---
title: Drawing with Simp
description: The immediate-mode renderer at the centre of the graphics stack — a render target, two shaders, a batch of quads, and the frame that presents them.
sidebar:
  order: 3
---

Build and run Pong, then ignore the paddles for a moment:

```sh
cargo run -q -p jr-cli -- build examples/games/pong/main.jr -o /tmp/pong \
    -I modules -I examples/games/pong/modules -L /opt/homebrew/lib
/tmp/pong
```

Every visible part of that window comes from the same five-step frame:

```jr
Simp.clear_render_target(0.03, 0.04, 0.08, 1.0);
Simp.set_shader_for_color();
Simp.immediate_begin();
Simp.immediate_quad(40.0, 40.0, 120.0, 90.0, white);
Simp.immediate_flush();
Simp.swap_buffers(*window);
```

Clear, choose a shader, submit geometry, flush, present. That is the useful mental model for
Jairs' `Simp`.

## Create the render target once

After opening the window, make it the target and check that OpenGL setup succeeded:

```jr
Simp.set_render_target(*window);
if !Simp.is_ready() {
    Window.close(*window);
    Window.stop();
    return;
}
```

`Simp` is a Simp-shaped 2D subset. It keeps one current render target and exposes immediate-mode
calls without passing a renderer object through the game. See `modules/Simp/module.jr` for every
declaration; the basic frame above is enough for Pong and Snake.

If the window is resized, call `Simp.update_window(*window)` so the viewport and projection use the
new drawable dimensions.

## Batch by drawing mode

There are two common modes:

```jr
Simp.set_shader_for_color();
Simp.set_shader_for_images(*texture);
```

Changing modes flushes the open batch first. Keep coloured shapes together and textured sprites
together when practical; each switch can become another draw call.

`immediate_quad` draws an axis-aligned rectangle. `immediate_quad_corners` accepts four positions
and UV coordinates for rotation or a sprite-sheet region. `immediate_triangle` is the primitive
underneath both. The renderer flushes automatically before its vertex buffer fills, so callers do
not count vertices themselves.

`swap_buffers` also flushes an open batch. An explicit `immediate_flush` still makes frame boundaries
and shader changes easier to see.

## Pick one coordinate system

The default is `Simp.RIGHT_HANDED`: origin at the bottom-left, positive y upward. Pong uses it
because its simulation is also y-up.

UI-style programs usually choose the top-left origin:

```jr
Simp.set_render_target(*window, Simp.LEFT_HANDED);
```

The choice belongs to the whole render target. Mixing y-up world geometry and y-down widgets in one
target requires an explicit conversion; silently flipping individual calls is a reliable way to
misalign input and drawing.

Game simulation values are commonly `float64`, while `Simp` vertices are `float32`. Keep the cast at
the rendering boundary:

```jr
Simp.immediate_quad(
    cast(float32, x0 * scale),
    cast(float32, y0 * scale),
    cast(float32, x1 * scale),
    cast(float32, y1 * scale),
    colour,
);
```

That leaves physics independent of pixels and keeps narrowing conversions out of the rulebook.

## Tear down in ownership order

Release textures first, then the OpenGL target, then the window:

```jr
Image.destroy_texture(*sheet);
Simp.destroy_render_target();
Window.close(*window);
Window.stop();
```

The GL context belongs to the window. Destroying the window first would leave the renderer pointing
at a context it no longer owns.

Drawing calls do not return success flags. The examples inspect OpenGL's error queue after
presenting:

```jr
if GL.error_code() != GL.NO_ERROR {
    print("a GL error was observed while drawing\n");
}
```

`GL.NO_ERROR` means this check observed no queued GL error. It does **not** prove that every call
succeeded or that the pixels look right—shader compilation and visual mistakes need their own
diagnostics.

Next: [The game loop](/games/the-game-loop/) — decide how much simulation belongs in each frame.
