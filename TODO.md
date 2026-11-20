# TODO

Deferred from the 2026-11-20 review. None of these are user-visible today;
each is written down with the shape of the fix so it can be picked up cold.

## Mixed-DPI reveal across monitors

`show()` now refreshes `max_height` for the target monitor and the `RevealNow`
fast path requires the anchor's monitor scale to match the window's current
`pixels_per_point`, so a cross-scale reveal goes through `Settling` instead.

What remains: `pending_reveal` stores a logical `Pos2` that `reveal_flyout_at`
scales by the *current* ppp. On a genuine scale change between monitors, prefer
storing the physical rect from `compute_placement_px` and passing it straight to
`SetWindowPos` (with `SWP_NOSIZE` when the parked size is already exact). Only
worth it with a real two-monitor, mixed-scale setup to test against.

## Poller: revert racing a newer edit

`finish_dispatch_error` can revert a parameter whose value the user changed
again while the request was in flight. Guard it: drop the in-flight marker
first, then skip `revert_param` when the key is still in `in_flight_params` or
`pending_params`. Error-path only, so it needs a deliberate test (server
rejects a set, user re-edits the same param inside the settle window).

## `draw()` split

`draw()` paints the card, handles header actions, sizes the canvas, updates
metrics, applies the region and runs the height grip. The duplicated sheet
blocks are already one helper and post-layout bookkeeping is keyed to
`laid_out_view`, so this is purely structural. If the file keeps churning,
extract the tail (metrics + region + grip) into `update_metrics` first; the paint
closure is already one cohesive pass and converting it to a `CardFrame` struct
has the largest regression surface of anything here. Behavior and visuals must
stay identical, so any split needs the 235 fps open/close capture rerun.

## Top-anchored taskbar

With a taskbar at the top of the screen the card can sit a canvas-height away
from the tray, because the card is always anchored to the window's bottom.
Placement lives in `tray::compute_flyout_pos`; uncommon configuration, no code
until someone uses it.
