# Vendored: iced_tiny_skia 0.14.1 (+ shadow clipping fix)

Verbatim copy of [`iced_tiny_skia` 0.14.1](https://crates.io/crates/iced_tiny_skia)
(from https://github.com/iced-rs/iced, `tiny_skia/` workspace member) with a
single upstream fix applied. **Keep this copy minimal** — every local delta is
a merge conflict when a fixed upstream release lands.

Deltas against the pristine 0.14.1:

1. `src/engine.rs` — the upstream fix described below;
2. this file.

The regression test lives in the app: `app/tests/quad_shadow_clipping.rs`
(wired through the `[dev-dependencies]` entry + `[patch.crates-io]` in
`app/Cargo.toml`), so it runs under plain `cargo test` without making the
vendored crate a workspace member.

## Why

Quad shadows were rendered as an unclipped pixmap: any partial redraw
(iced damage tracking) that touched a shadowed popup re-composited the whole
translucent shadow over stale pixels. Hovering items in a menu visibly
stacked its shadow — darkening rows below the cursor and the halo around the
card (see `src/engine.rs`, `Engine::draw_quad`).

## The fix

Upstream commit [`05535598`](https://github.com/iced-rs/iced/commit/05535598)
—"Clip quad shadows in `tiny-skia` renderer" — applied to `src/engine.rs`:

1. the quad's shadow `draw_pixmap` call now passes `Some(clip_mask)` instead
   of `None`, so the shadow is clipped to the damaged region;
2. the conditional `clip_mask` binding used by `fill_path`/`stroke_path` is
   computed after the shadow draw (the shadow itself always uses the full
   mask, since it intentionally extends beyond the quad bounds).

## When to remove

No released iced version ships the fix yet (0.14.1 was published from a
maintenance branch that predates it). Once iced releases a version containing
05535598 (0.14.2+ or 0.15), delete this directory, the `[patch.crates-io]`
and `[dev-dependencies]` entries for `iced_tiny_skia` in `app/Cargo.toml`,
and `app/tests/quad_shadow_clipping.rs`, and bump the dependency.
