---
title: Sprites and widgets
description: A texture built at runtime, an animated sprite sheet, and two immediate-mode buttons that force the whole target to left-handed coordinates.
sidebar:
  order: 10
---

This example combines the previous chapters into one moving scene: a four-frame sprite bounces
around the window, Pause stops it, and Reset returns it to the centre.

```sh
cargo run -q -p jr-cli -- build examples/games/sprites/main.jr -o /tmp/sprites \
    -I modules -L /opt/homebrew/lib
SPRITES_FRAMES=120 /tmp/sprites
```

`SPRITES_FRAMES` makes the graphical example stop on its own, which is useful for a smoke run.

## Animate one texture

The example builds a four-frame surface in memory and uploads it after the GL context exists.
The [textures chapter](/games/textures-and-images/) covers that ownership sequence.

Animation does not replace the texture. It changes the UV slice drawn from it:

```jr
frame := cast(s64, elapsed * 8.0) % FRAMES;

Simp.set_shader_for_images(*sheet);
Simp.immediate_begin();
draw_sprite(x, y, SPRITE_SIZE, frame);
Simp.immediate_flush();
```

Inside `draw_sprite`, frame `index` maps to the horizontal interval
`index / FRAMES .. (index + 1) / FRAMES`. `immediate_quad_corners` accepts those four UV corners;
the simpler two-corner quad always draws the whole texture.

Passing white as the vertex colour preserves the original pixels. Changing that colour gives the
sprite a tint, useful for flashes and selection states.

## Add controls without adding a widget tree

The target uses top-left, y-down coordinates so UI hit-testing and drawing agree:

```jr
Simp.set_render_target(*window, Simp.LEFT_HANDED);
```

After feeding this frame's input events, declare the two buttons directly:

```jr
if draw_button(*ui, ID_PAUSE, 12, 12, 90, 28) {
    paused = !paused;
}
if draw_button(*ui, ID_RESET, 112, 12, 90, 28) {
    x = cast(float64, WIDTH) / 2.0;
    y = cast(float64, HEIGHT) / 2.0;
}
```

An outer Simp batch around these calls would contribute nothing. Each `draw_button` selects its
shader, opens its batch, draws, and flushes before returning.

The sprite still has its own image batch because it uses a different shader and texture. Keeping
those batches visibly separate makes the render order easy to follow: clear, sprite, widgets,
swap.

## Clean up what the scene owns

```jr
Image.destroy_texture(*sheet);
Simp.destroy_render_target();
Window.close(*window);
Window.stop();
```

The example also reads the OpenGL error queue. Finishing with no reported error is useful smoke
evidence; it does not prove that every GL operation succeeded or that the pixels match an expected
image.

Next: [What a game cannot do yet](/games/not-implemented/) — a compact map of the missing pieces.
