//! Per-row commit-graph canvas. Each list row owns a small canvas that
//! draws the incoming continuation, the node and the outgoing edges of its
//! `GraphRow`, so the virtualized commit list can render lanes without one
//! giant canvas.

use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use iced::{mouse, Color, Point, Rectangle, Renderer, Theme};

use crate::graph::types::{GraphEdge, GraphRow};

use super::theme::Tokens;

pub const ROW_HEIGHT: f32 = 24.0;
pub const LANE_WIDTH: f32 = 14.0;
pub const PADDING: f32 = 10.0;
pub const NODE_RADIUS: f32 = 4.0;

/// Width of the lane column for a given lane count (centers at
/// `PADDING + lane * LANE_WIDTH`).
pub fn graph_width(max_lanes: u16) -> f32 {
    PADDING * 2.0 + (max_lanes.max(1) as f32 - 1.0) * LANE_WIDTH
}

fn lane_x(lane: u16) -> f32 {
    PADDING + lane as f32 * LANE_WIDTH
}

pub fn lane_color(tokens: Tokens, lane: u16) -> Color {
    match lane % 6 {
        0 => tokens.primary,
        1 => tokens.renamed,
        2 => tokens.modified,
        3 => tokens.added,
        4 => tokens.deleted,
        _ => tokens.untracked,
    }
}

/// One row's lane cell.
#[derive(Debug, Clone, Copy)]
pub struct GraphCell<'a> {
    pub row: &'a GraphRow,
    pub previous_edges: &'a [GraphEdge],
    pub tokens: Tokens,
}

impl canvas::Program<crate::ui::app::Message> for GraphCell<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let center_y = ROW_HEIGHT / 2.0;

        // Continuation from the previous row: its outgoing edge landed at
        // `to` on our top edge; run it down to this row's center.
        for edge in self.previous_edges {
            let color = lane_color(self.tokens, edge.from);
            frame.stroke(
                &Path::line(
                    Point::new(lane_x(edge.to), 0.0),
                    Point::new(lane_x(edge.to), center_y),
                ),
                Stroke::default().with_color(color).with_width(1.5),
            );
        }

        // Outgoing edges start at this row's center.
        for edge in &self.row.edges {
            let color = lane_color(self.tokens, edge.from);
            let start = Point::new(lane_x(edge.from), center_y);
            let end = Point::new(lane_x(edge.to), ROW_HEIGHT);
            let path = if edge.from == edge.to {
                Path::line(start, end)
            } else {
                let middle = center_y + (ROW_HEIGHT - center_y) / 2.0;
                Path::new(|builder| {
                    builder.move_to(start);
                    builder.bezier_curve_to(
                        Point::new(lane_x(edge.from), middle),
                        Point::new(lane_x(edge.to), middle),
                        end,
                    );
                })
            };
            frame.stroke(&path, Stroke::default().with_color(color).with_width(1.5));
        }

        // Node on top of its edges.
        let center = Point::new(lane_x(self.row.lane), center_y);
        let color = lane_color(self.tokens, self.row.lane);
        frame.fill(&Path::circle(center, NODE_RADIUS), color);
        frame.stroke(
            &Path::circle(center, NODE_RADIUS + 0.75),
            Stroke::default()
                .with_color(self.tokens.background)
                .with_width(1.5),
        );

        vec![frame.into_geometry()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_width_tracks_lane_count() {
        assert_eq!(graph_width(1), 20.0);
        assert_eq!(graph_width(3), 48.0);
        assert_eq!(graph_width(0), 20.0);
    }

    #[test]
    fn lane_colors_cycle() {
        let tokens = Tokens::LIGHT;
        assert_eq!(lane_color(tokens, 0), lane_color(tokens, 6));
        assert_eq!(lane_color(tokens, 1), lane_color(tokens, 7));
        assert_ne!(lane_color(tokens, 0), lane_color(tokens, 1));
    }
}
