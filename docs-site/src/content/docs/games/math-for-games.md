---
title: Maths for games
description: The small part of modules/Math Pong uses, followed by the helpers a practical 2D game still lacks.
sidebar:
  order: 7
---

`modules/Math` gives Pong enough vocabulary to keep its simulation readable: positions and
velocities are `Vector2`, speed is a vector length, and a paddle hit chooses a new normalized
direction.

Snake is different. Its grid module imports `Random`, not `Math`; integer cells are simpler than
floating-point vectors for that game.

## A useful 2D slice

Pong's bounce is the best small tour of the module:

```jr
speed := Math.length2(state.ball_velocity) * BALL_SPEEDUP;
offset := (state.ball.y - paddle_y) / (PADDLE_HEIGHT / 2.0);
steer := Math.vec2(direction * 0.85, offset * 0.75);
state.ball_velocity = Math.normalize2(steer) * speed;
```

The game keeps its simulation in `float64`. Rendering converts to Simp's `float32` values only at
the drawing boundary.

For ordinary gameplay, the most useful pieces are:

- `vec2`, vector addition and subtraction, scalar multiplication, and equality.
- `dot2`, `length2`, `distance2`, `normalize2`, and `lerp2`.
- `fabs`, `sqrt`, `sin`, and `cos`.
- `Matrix4` and quaternions when a project grows into 3D transforms.

The complete introductory API tour lives in [Math by example](/by-example/075-stdlib-math/).

## Write the missing helper where you need it

The integer `Math.clamp` cannot clamp a paddle's `float64` position, so Pong uses two comparisons:

```jr
if moved < half {
    moved = half;
}
if moved > FIELD_HEIGHT - half {
    moved = FIELD_HEIGHT - half;
}
```

That is often the right response to a small library gap: keep the helper near the gameplay rule
until more than one project needs the same abstraction.

The common 2D gaps today are:

- Float `clamp`, `min`, `max`, scalar `lerp`, and approximate equality.
- `PI`, `TAU`, degrees/radians conversion, `atan2`, and float modulo.
- `rotate2`, perpendicular vectors, reflection, and projection.
- A general matrix inverse and screen-to-world unprojection.
- Random floats, random directions, and shuffling.

## Rectangles: platform data is not gameplay geometry

`Window.Rect` does exist. It mirrors SDL's integer rectangle for window and surface operations.
It does not provide overlap, containment, movement, or collision response.

Pong therefore writes its paddle collision as four separating-axis checks. A future gameplay
geometry module could turn that into one `overlaps` call, but describing Jairs as having “no
Rect” would be inaccurate: the missing piece is a gameplay geometry API.

Keep the same distinction in mind elsewhere. `Math` is already a substantial scalar, vector,
matrix, and quaternion module; it is not yet a complete 2D game-math toolkit.

Next: [Pong](/games/pong/) — the complete rules module that puts this small vector slice to work.
