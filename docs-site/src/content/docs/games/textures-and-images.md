---
title: Textures and images
description: modules/Image — loading and building BMP surfaces, uploading them to GL textures Simp can draw, and a sprite sheet selected by UV sub-rectangle.
sidebar:
  order: 5
---

This chapter turns CPU-side pixels into a texture and draws one frame from a sprite sheet. The
important idea is ownership: a surface and a texture are separate resources, and uploading one
does not destroy the other.

Jairs' `Simp` is a Simp-shaped subset, not a copy of Jai's complete module. In this implementation,
SDL2 creates the window, carries input, creates the GL context, and swaps buffers. OpenGL performs
the rendering.

## Build and upload a small image

`examples/games/sprites/main.jr` creates its sprite sheet in memory so the example needs no binary
asset:

```jr
surface, made := Image.create_surface(FRAME * FRAMES, FRAME);
if !made {
    return texture, false;
}

whole := Window.rect(0, 0, FRAME * FRAMES, FRAME);
if !Image.fill_surface(*surface, *whole, rgba(30, 40, 70, 255)) {
    Image.free_surface(*surface);
    return texture, false;
}

texture, uploaded := Image.texture_from(*surface);
Image.free_surface(*surface);
return texture, uploaded;
```

`texture_from` reads the surface and uploads a GL texture. It does **not** free the caller's
surface; the explicit `free_surface` above is required. If you only need a file as a texture,
`Image.load_texture` is the convenience path: it loads a BMP and frees its own intermediate
surface.

Create or load textures only after `Simp.set_render_target`. That call establishes the current GL
context needed by the upload.

## Pitch is a stride, not a promise

An image row has a byte stride called its pitch. The common RGBA surface created by this module is
tightly packed, so its pitch is normally `width * 4`. Other SDL surfaces may use a different
stride.

You do not need to repack rows yourself. `texture_from` checks the pitch and copies into a tight
temporary buffer only when it differs. The rule matters if you later add direct pixel access:
advance by the reported pitch, not by an assumed row width.

`Window.Rect` appears in `fill_surface` because it is the SDL-compatible rectangle used for pixel
operations. It is not a gameplay collision type.

## Draw one frame from a sheet

Upload the whole sheet once. Select a frame at draw time by changing its horizontal UV range:

```jr
u0 := cast(float32, cast(float64, index) / cast(float64, FRAMES));
u1 := cast(float32, cast(float64, index + 1) / cast(float64, FRAMES));

Simp.set_shader_for_images(*sheet);
Simp.immediate_begin();
Simp.immediate_quad_corners(
    p0, p1, p2, p3,
    Simp.vector4(1.0, 1.0, 1.0, 1.0),
    Simp.vector2(u0, 0.0),
    Simp.vector2(u1, 0.0),
    Simp.vector2(u1, 1.0),
    Simp.vector2(u0, 1.0),
);
Simp.immediate_flush();
```

White leaves the sampled texture unchanged because the image shader multiplies the texel by the
vertex colour. Pass another colour to tint the sprite without creating another texture.

## Release resources in the reverse order

The texture belongs to the GL context, so destroy it before destroying the render target:

```jr
Image.destroy_texture(*sheet);
Simp.destroy_render_target();
Window.close(*window);
Window.stop();
```

Today `Image` handles BMP files, surface creation, fills, and GL upload. PNG decoding, public pixel
editing, and pre-upload cropping are not present; the compact inventory is in
[What a game cannot do yet](/games/not-implemented/).

Next: [Immediate-mode UI](/games/immediate-mode-ui/) — one button, one interaction state, and no
widget tree.
