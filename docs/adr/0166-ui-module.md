# ADR-0166: `UI` — immediate-mode widgets, and the sentinel that answered `true`

- **Status:** Accepted
- **Date:** 2026-09-02
- **Deciders:** dboll
- **PLAN §8.5's immediate-mode UI item**, which ADR-0165's event loop unblocked. The module that proves the
  graphics stack **composes**, since it is the first thing here needing a window, an event queue and a renderer
  at once.
- No design fork was put to the decider; the recommended approach was taken and the rejected
  alternatives are recorded below, per the session directive.

## Context

`Window` (ADR-0164) draws and `Window`'s event loop (ADR-0165) reads input. Neither says anything about the
thing a caller actually writes, which is a button. This wave is that, and it is the first test in the project
that needs all three of a window, a queue and a renderer alive at the same time.

## Decision

### 1. Immediate mode, and no allocator

There is no widget tree and no retained object per widget. A caller re-declares every widget every frame, and
the state that must persist is exactly three things: where the mouse is, which widget the cursor is over, and
which widget is being pressed. So `UI` is a small struct a caller keeps on the stack.

**That is the property worth having here.** This language's allocator is a `context` field (ADR-0061), and a UI
that needs no allocator cannot get one wrong — no lifetime discipline for widget handles, no diffing pass, no
question about which allocator a widget's storage came from.

**Rejected: a retained widget tree.** It reads better for a form with forty fields, and it needs all three of
the things above. Immediate mode is what a language at this stage can carry honestly.

### 2. A widget's identity is a number the caller chooses, and the caller owns its uniqueness

`button(ui, id, …)` takes a stable, unique `id`. That is how a stateless call gets stateful behaviour: `active`
holds the id of the widget being pressed, so a press survives into the next frame although the *call* does not.

**Two widgets sharing an id is a bug this module cannot detect**, and the docs say so rather than implying
otherwise: they would highlight and fire together. **Rejected: deriving an id from the call site** — there is no
`#caller_location`-shaped mechanism here, and inventing one for this is a language change wearing a library's
clothes. **Rejected: deriving it from the rectangle**, which makes two same-sized buttons in a scrolling list
collide precisely when the list scrolls.

### 3. A click is a release inside after a press inside — never a press

The interesting decision, and the one a naive implementation gets wrong. Press inside sets `active`; release
with `active == id` **and the cursor still inside** is a click; release anywhere else clears `active` and fires
nothing; a press that begins outside cannot arm the widget.

**Returning `true` on press passes every positive test** and breaks the escape hatch every user expects: press a
button, think again, drag off, release. That must do nothing. Four of this wave's sixteen assertions are that
negative case, because it is the only part a passing test suite would otherwise not have checked.

**`is_active` stays true after the cursor leaves**, deliberately. The widget is still armed — moving back and
releasing still fires it — so showing it un-pressed would tell the user the opposite of the truth.

### 4. Hit-testing is half-open on the far edges

`x <= px < x + w`. Two rectangles sharing an edge must not both claim the pixel on it, or a two-button row has a
one-pixel column that highlights both. It is also the convention every fill here already uses, and disagreeing
with the drawing API about which pixels a rectangle covers would be its own bug.

### 5. Drawing is separate from the widget call

Immediate-mode libraries usually draw *inside* the widget call. Splitting them means the state machine can be
exercised with no display, which is how this module's tests run, and it lets a caller draw a button any way they
like while keeping the interaction. `draw_button` is a convenience over `is_hot`/`is_active`, not the only way.

### 6. The sentinel must not be askable about — a real bug this wave's tests caught

`is_hot` was `return ui.hot == id`. `begin_frame` sets `hot` to `NONE`. So **`is_hot(ui, NONE)` answered `true`
on every frame** — a widget that does not exist, reported as hovered.

`button` already refused a zero id. `is_hot` and `is_active` did not, and the inconsistency is exactly the shape
that survives review: the guard was written where the *obvious* misuse was, and the sentinel comparison is not
obviously a misuse until you notice that `hot` holds the sentinel most of the time.

**Both now refuse an invalid id.** The general rule is worth stating because this project will meet it again: a
sentinel meaning "nothing" must not be comparable through the same accessor as a real value, or every "is this
the one" question has an answer of yes for a thing that is not there.

Found by an assertion written because the zero id existed, not because a bug was suspected. That is the argument
for testing a sentinel's behaviour rather than only a value's.

### 7. What is absent, and why

**No text**, because a label needs a font — `SDL_ttf` (a second library) or a bitmap glyph table carried as
data. A button that draws its own frame is useful without one.

**No keyboard focus**, because tab-order needs an ordered widget list that immediate mode deliberately does not
keep, and the standard answer (a focus id plus a per-frame candidate scan) wants the text field that does not
exist yet to be worth having.

**No layout.** Every widget takes explicit pixels. A row/column stack is small *and* a policy decision — does a
stack own spacing, does it clip — so it waits for a caller with an opinion.

**Only the left button.** A right-click is a different interaction with different conventions per platform, and
guessing would be worse than not having it.

## Consequences

- **PLAN §8.5's immediate-mode UI item is done**, and W10's three named items with it.
- **`modules/UI` is the eighteenth module**, and the second that `jr run` cannot execute (ADR-0164 §6's reason).
- **1058 tests**, 253 corpus files.
- **The graphics stack is shown to compose**, which is a stronger claim than three modules working: one test
  holds a window, a queue and a renderer open together and drives a real interaction through them.
- **`#import` is flat and there is no `Window.Event` syntax** (probed). So a module building on another must not
  collide with its names — recorded here because this file reads as though it were qualified and is not.
- **§8.5's remaining items** are image decode and audio, both library work.
