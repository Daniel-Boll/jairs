---
title: The game loop
description: modules/Time's two clocks, the two loop shapes Pong and Snake actually use, vsync through swap_buffers, and why a game with no test can still be checked.
sidebar:
  order: 4
---

Run the graphical Pong example for exactly 120 frames:

```sh
PONG_FRAMES=120 /tmp/pong
```

It opens, simulates, draws, then exits with a summary. The frame budget is only a test hook; the
actual loop has the familiar shape:

```text
poll input → measure time → update rules → draw → present
```

The choice that changes how a game feels is how time reaches the rules.

## Measure durations with the monotonic clock

Use `Time.monotonic()` for frame durations:

```jr
now := Time.monotonic();
dt := cast(float64, now - previous) /
    cast(float64, Time.NANOSECONDS_PER_SECOND);
previous = now;
```

`Time.wall()` is a timestamp clock. It can jump when the system time changes, so it is useful for a
random seed or a date, not for movement. A monotonic clock cannot turn one frame's `dt` negative.

The complete clock API and unit conversions live in `modules/Time/module.jr`.

## Continuous games: clamp the variable step

Pong moves by velocity multiplied by elapsed seconds. It therefore uses a variable step, but refuses
to simulate an arbitrarily long frame:

```jr
if dt > 1.0 / 30.0 {
    dt = 1.0 / 30.0;
}

_ = Pong.update(*state, dt, left, right);
```

After a breakpoint or window drag, the measured interval may be seconds. Applying it in one update
would teleport the ball through a paddle. Clamping deliberately loses wall-clock time so the
simulation remains playable.

For stricter physics, sub-step the long interval instead. Pong stays small enough to teach the
trade-off rather than hide it in a framework.

## Grid games: accumulate fixed ticks

Snake moves one whole cell at a time. Its simulation rate must not depend on how many frames the
display happens to draw:

```jr
pending = pending + dt;

while pending >= state.step_seconds && !Snake.is_over(*state) {
    pending = pending - state.step_seconds;
    _ = Snake.step(*state);
}
```

The `while` matters. If a slow frame owes two ticks, an `if` would pay only one and make the game
permanently lag behind. The accumulator lets rendering vary while the rules advance at a stable
rate.

Those are the two loop shapes used throughout the examples:

- continuous motion: measured and clamped `dt`;
- discrete rules: fixed steps paid from an accumulator.

## Presentation is not a clock

`Simp.swap_buffers(*window)` requests vsync by default. A visible, composited window will often block
until the next display refresh, but focus, display settings and platform behavior can change that.
Do not use frame count as elapsed time.

If a program needs an explicit yield, `Window.delay(milliseconds)` is available. A full target-FPS
limiter is not yet part of the standard library.

## Give the loop a machine-readable ending

The graphical examples accept `PONG_FRAMES`, `SNAKE_FRAMES` or `SPRITES_FRAMES`. A bounded run can
verify that a program builds, links, opens its rendering path and exits cleanly without requiring a
person to close it.

That is useful but incomplete. A clean exit and `GL.NO_ERROR` mean no GL error was observed; they do
not catch a wrong colour, misplaced sprite or unreadable frame. Keep headless assertions for rules,
bounded smoke runs for platform integration, and visual checks for pixels.

Read `examples/games/pong/main.jr` and `examples/games/snake/main.jr` for the complete loops. The
interesting parts are now small enough to recognize: one drain, one clock reading, one simulation
policy and one presentation.

Next: [Textures and images](/games/textures-and-images/) — put pixels from an asset into a `Simp`
texture.
