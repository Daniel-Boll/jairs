---
title: Immediate-mode UI
description: modules/UI's buttons — a state machine over Input's queue, drawn as quads through Simp, with no widget tree and no allocator.
sidebar:
  order: 6
---

The outcome here is deliberately small: a Pause button that highlights under the mouse, stays
armed while held, and fires only when released inside its rectangle.

`modules/UI` is immediate mode. You declare the same button every frame; a compact `UI` value
keeps only the interaction state that must survive between frames.

## Feed events, then declare the button

At the start of a frame, clear the one-frame edges and pass the current events through the UI:

```jr
begin_frame(*ui);

_ = Input.update_window_events(*events, Input.MAX_EVENTS_PER_FRAME);
seen := Input.events_this_frame(*events);
i := 0;
while i < seen.count {
    event := seen[i];
    feed(*ui, *event);
    i = i + 1;
}
```

Then draw and handle the button in one call:

```jr
if draw_button(*ui, ID_PAUSE, 12, 12, 90, 28) {
    paused = !paused;
}
```

Do not wrap that call in an outer `Simp.immediate_begin`/`immediate_flush` pair.
`draw_button` selects the colour shader and owns its own batch.

Each widget needs a stable, non-zero id. Calling both `button` and `draw_button` for the same id
in one frame is also a mistake: `draw_button` already runs the interaction state machine.

## A click happens on release

The state transition is:

1. Press inside: the button becomes `active`.
2. Hold or drag: it remains active.
3. Release inside: it fires.
4. Release outside: it clears without firing.

This lets a player cancel a click by dragging away before release. On the firing frame,
`button` clears `active` before `draw_button` chooses its shade. Because the pointer is still
inside, the button draws **hot**, not active.

The caller owns the `UI` value so the interaction logic is explicit, testable, and usable with
more than one UI state. That is a design choice, not a language limitation: Jairs supports
file-scope mutable state.

## Keep input and drawing in the same coordinates

Mouse events use a top-left origin with y increasing downward. Set the render target to the same
convention:

```jr
Simp.set_render_target(*window, Simp.LEFT_HANDED);
```

Without that choice, a button can be drawn in one place and hit-tested in another. The UI module
cannot perform the conversion itself because it does not own the window height.

The current module provides a button, not a complete toolkit. Text labels, layout, focus, sliders,
and text fields remain future work; see [What a game cannot do yet](/games/not-implemented/).

Next: [Maths for games](/games/math-for-games/) — useful vector operations and the small helpers a
2D game still writes itself.
