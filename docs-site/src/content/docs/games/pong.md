---
title: Pong
description: A full game of Pong, split so the rules run headless in the compile-time VM and only the window needs SDL2.
sidebar:
  order: 8
---

Pong is the first complete vertical slice in this book: rules, input, timing and drawing, with fewer
than a dozen moving values.

Run the game without a display first:

```sh
cargo run -q -p jr-cli -- run examples/games/pong/sim.jr \
    -I modules -I examples/games/pong/modules
```

```text
steps=15153 walls=95 paddle hits=219 points=9
final score 4-5
```

Then build the version you can play:

```sh
cargo run -q -p jr-cli -- build examples/games/pong/main.jr -o /tmp/pong \
    -I modules -I examples/games/pong/modules -L /opt/homebrew/lib
/tmp/pong
```

Use `W` and `S`; the right paddle follows the ball.

## One rulebook, two drivers

`examples/games/pong/modules/Pong/module.jr` owns all game behavior. Its central procedure has no
idea where input came from or where output will go:

```jr
event := Pong.update(*state, dt, left, right);
```

`sim.jr` supplies a fixed `dt` and lets `Pong.follow` control both paddles. `main.jr` supplies measured
time, human keys for the left paddle, and rendering. Because both drivers call the same rulebook, the
headless result is a real check of the playable game rather than a second implementation.

The simulation also works in world units. The field is `200 × 150`; the graphical driver alone
converts those positions to `float32` pixels.

## Three details make the bounce feel intentional

First, a wall collision fixes position as well as velocity:

```jr
if state.ball.y - BALL_RADIUS < 0.0 {
    state.ball.y = BALL_RADIUS;
    state.ball_velocity.y = Math.fabs(state.ball_velocity.y);
}
```

Reflecting a ball that is already outside leaves it outside, where the next frame can collide again.
Putting it back on the boundary prevents that sticky-wall behavior.

Second, a paddle is tested only while the ball approaches it. An overlap test alone cannot tell the
paddle's front from its back:

```jr
if state.ball_velocity.x < 0.0 {
    if hits_paddle(state.ball, left_x, state.left_y) {
        bounce(state, state.left_y, 1.0);
    }
}
```

Third, the bounce angle comes from where the ball hit:

```jr
offset := (state.ball.y - paddle_y) / (PADDLE_HEIGHT / 2.0);
steer := Math.vec2(direction * 0.85, offset * 0.75);
state.ball_velocity = Math.normalize2(steer) * speed;
```

That offset is the player's control over the return. The speed increases slightly after a hit but is
capped before one frame can carry the ball through an entire paddle.

## Determinism is a feature here

The serve alternates between two vertical directions instead of using random input. That makes the
headless match reproducible: the same fixed steps produce the same rally and score.

Random serves would make a fine game, but they would weaken this particular example. When rules are
deterministic, a changed score points directly at changed behavior.

The graphical driver can still be smoke-tested without somebody watching forever:

```sh
PONG_FRAMES=120 /tmp/pong
```

It reports the frame count and score on exit. The driver also checks `GL.error_code()` after drawing;
a clean run means no GL error was observed, not that every pixel was proven correct.

## Read the complete example

The full source is intentionally the reference:

- `examples/games/pong/modules/Pong/module.jr`: state, collision and scoring;
- `examples/games/pong/sim.jr`: deterministic headless match;
- `examples/games/pong/main.jr`: held keys, clamped time and `Simp` drawing.

Notice what is absent from the rulebook: SDL constants, keycodes, OpenGL types and pixel sizes. That
absence is what makes the same game easy to test and easy to render.

Next: [Snake](/games/snake/) — the same seam applied to a fixed-tick grid game.
