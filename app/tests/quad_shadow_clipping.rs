//! Regression test for the vendored `iced_tiny_skia` shadow-clipping fix
//! (upstream iced commit 05535598): quad shadows must be clipped to the
//! damaged region.
//!
//! The app renders its popup cards (context menu, palette, quick switcher,
//! quick open) with blurred shadows through iced's software backend, which
//! presents frames with damage tracking. Hovering a menu row damages only
//! that row's bounds; the unfixed backend then re-composited the popup's
//! whole translucent shadow over stale pixels on every hover — visibly
//! stacking the shadow.
//!
//! The test renders a two-frame menu — frame A opens a popup card with the
//! hover on row 0, frame B moves the hover to row 1 — through the exact
//! layer-diff -> damage -> draw path of `iced_tiny_skia`'s window
//! compositor, then asserts the incremental redraw is pixel-identical to a
//! clean redraw of the same scene.

use iced_tiny_skia::core::renderer::Quad;
use iced_tiny_skia::core::{
    Background, Border, Color, Font, Pixels, Rectangle, Renderer as _, Shadow, Size, Vector,
};
use iced_tiny_skia::graphics::{damage, Viewport};
use iced_tiny_skia::{Layer, Renderer};

const VIEWPORT: Size = Size::new(200.0, 300.0);
/// Dark backdrop, like the diff pane under a popup card.
const BACKGROUND: Color = Color::from_rgb(0.11, 0.12, 0.13);
/// Same values as the popup cards in `ui::context_menu` et al.
const SHADOW: Shadow = Shadow {
    color: Color::from_rgba(0.0, 0.0, 0.0, 0.2),
    offset: Vector::new(0.0, 4.0),
    blur_radius: 12.0,
};

/// One menu frame: a shadowed popup card plus one hover row whose `y`
/// changes as the mouse moves down the menu.
fn draw_scene(renderer: &mut Renderer, row_y: f32, shadow: Shadow) {
    renderer.reset(Rectangle::with_size(VIEWPORT));

    let card = Quad {
        bounds: Rectangle {
            x: 40.0,
            y: 40.0,
            width: 120.0,
            height: 200.0,
        },
        border: Border {
            color: Color::from_rgb(0.8, 0.8, 0.8),
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow,
        snap: true,
    };
    renderer.fill_quad(card, Background::Color(Color::from_rgb(1.0, 1.0, 1.0)));

    let row = Quad {
        bounds: Rectangle {
            x: 44.0,
            y: row_y,
            width: 112.0,
            height: 20.0,
        },
        border: Border::default(),
        shadow: Shadow::default(),
        snap: true,
    };
    renderer.fill_quad(row, Background::Color(Color::from_rgb(0.9, 0.9, 0.9)));
}

/// Mirrors `iced_tiny_skia`'s `window::compositor::present`: diff the
/// previous layers, derive the damage rects, redraw only the damage into
/// the same buffer.
fn present(
    renderer: &mut Renderer,
    buffer: &mut [u32],
    clip_mask: &mut tiny_skia::Mask,
    viewport: &Viewport,
    previous: &mut Option<Vec<Layer>>,
) {
    let layers = renderer.layers().to_vec();
    let full = Rectangle::with_size(viewport.logical_size());

    let damage = previous
        .as_ref()
        .map(|last_layers| {
            damage::diff(
                last_layers,
                &layers,
                |layer| vec![layer.bounds],
                Layer::damage,
            )
        })
        .unwrap_or_else(|| vec![full]);
    let damage = damage::group(damage, full);

    let mut pixels = tiny_skia::PixmapMut::from_bytes(
        bytemuck::cast_slice_mut(buffer),
        VIEWPORT.width as u32,
        VIEWPORT.height as u32,
    )
    .unwrap();
    renderer.draw(&mut pixels, clip_mask, viewport, &damage, BACKGROUND);
    *previous = Some(layers);
}

struct FrameBuf {
    buffer: Vec<u32>,
    clip_mask: tiny_skia::Mask,
    previous: Option<Vec<Layer>>,
}

impl FrameBuf {
    fn new() -> Self {
        Self {
            buffer: vec![0; VIEWPORT.width as usize * VIEWPORT.height as usize],
            clip_mask: tiny_skia::Mask::new(VIEWPORT.width as u32, VIEWPORT.height as u32).unwrap(),
            previous: None,
        }
    }

    fn present(&mut self, renderer: &mut Renderer) {
        let viewport = Viewport::with_physical_size(
            Size::new(VIEWPORT.width as u32, VIEWPORT.height as u32),
            1.0,
        );
        let FrameBuf {
            buffer,
            clip_mask,
            previous,
        } = self;
        present(renderer, buffer, clip_mask, &viewport, previous);
    }
}

fn differing_pixels(a: &FrameBuf, b: &FrameBuf) -> usize {
    a.buffer
        .iter()
        .zip(b.buffer.iter())
        .filter(|(x, y)| x != y)
        .count()
}

/// A hover change over a shadowed popup must leave the frame identical to
/// a clean redraw (no shadow stacking).
#[test]
fn hover_redraw_does_not_stack_quad_shadows() {
    // Frame A: menu open, hover on row 0.
    let mut incremental_renderer = Renderer::new(Font::default(), Pixels::from(13.0));
    let mut incremental = FrameBuf::new();
    draw_scene(&mut incremental_renderer, 60.0, SHADOW);
    incremental.present(&mut incremental_renderer);

    // Frame B: mouse moved down one row -> hover row 1.
    draw_scene(&mut incremental_renderer, 80.0, SHADOW);
    incremental.present(&mut incremental_renderer);

    // Reference: clean redraw of the exact frame-B scene.
    let mut reference_renderer = Renderer::new(Font::default(), Pixels::from(13.0));
    let mut reference = FrameBuf::new();
    draw_scene(&mut reference_renderer, 80.0, SHADOW);
    reference.present(&mut reference_renderer);

    assert_eq!(
        differing_pixels(&incremental, &reference),
        0,
        "incremental redraw diverged from a clean redraw: quad shadows are \
         being re-composited over stale pixels on partial redraws"
    );
}

/// Sanity check for the harness itself: the two-frame scenario is green
/// for a trivial reason when no shadow is involved, so a failure above can
/// only come from shadow handling.
#[test]
fn hover_redraw_is_stable_without_shadow() {
    let mut incremental_renderer = Renderer::new(Font::default(), Pixels::from(13.0));
    let mut incremental = FrameBuf::new();
    draw_scene(&mut incremental_renderer, 60.0, Shadow::default());
    incremental.present(&mut incremental_renderer);

    draw_scene(&mut incremental_renderer, 80.0, Shadow::default());
    incremental.present(&mut incremental_renderer);

    let mut reference_renderer = Renderer::new(Font::default(), Pixels::from(13.0));
    let mut reference = FrameBuf::new();
    draw_scene(&mut reference_renderer, 80.0, Shadow::default());
    reference.present(&mut reference_renderer);

    assert_eq!(differing_pixels(&incremental, &reference), 0);
}
