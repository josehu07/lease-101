//! Renderable frame geometry, interpolated from engine state at a given time.
//!
//! The engine produces discrete events; the frame layer turns the state at an
//! arbitrary global time into drawable primitives (node disks, in-flight message
//! dots, lease-timer bars). Both the live Canvas2D frontend and the native GIF
//! tool consume identical [`Frame`]s, so layout/interpolation lives here once.

use crate::clock::Time;
use crate::event::{LeaseStatus, MsgFate, MsgKind, NodeId};

/// A 2D point in an abstract unit canvas (`0.0..1.0` on both axes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Visual liveness of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeViz {
    Up,
    Down,
}

/// A node disk to draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeShape {
    pub id: NodeId,
    pub pos: Point,
    pub viz: NodeViz,
}

/// A message in flight, with `progress` in `0.0..1.0` from sender to receiver.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MsgShape {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: MsgKind,
    /// Eventual fate; lets the frontend foreshadow a drop (e.g. fade out).
    pub fate: MsgFate,
    pub progress: f64,
    pub pos: Point,
}

/// A lease-timer bar: `fill` in `0.0..1.0` is the fraction of life remaining.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeaseBar {
    pub grantor: NodeId,
    pub grantee: NodeId,
    pub status: LeaseStatus,
    /// Remaining fraction from the grantor's viewpoint.
    pub grantor_fill: f64,
    /// Remaining fraction from the grantee's viewpoint.
    pub grantee_fill: f64,
}

/// Everything needed to paint one frame at a given global time.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub at: Time,
    pub nodes: Vec<NodeShape>,
    pub messages: Vec<MsgShape>,
    pub leases: Vec<LeaseBar>,
}

/// Place `n` nodes evenly on a circle inscribed in the unit canvas.
pub fn ring_layout(n: usize) -> Vec<Point> {
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![Point { x: 0.5, y: 0.5 }];
    }
    let r = 0.38;
    (0..n)
        .map(|i| {
            // Start at the top and go clockwise.
            let theta =
                -core::f64::consts::FRAC_PI_2 + (i as f64) * core::f64::consts::TAU / (n as f64);
            Point {
                x: 0.5 + r * theta.cos(),
                y: 0.5 + r * theta.sin(),
            }
        })
        .collect()
}

/// Linear interpolation between two points.
pub fn lerp(a: Point, b: Point, t: f64) -> Point {
    let t = t.clamp(0.0, 1.0);
    Point {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_layout_counts_and_bounds() {
        for n in [0usize, 1, 2, 3, 5, 8] {
            let pts = ring_layout(n);
            assert_eq!(pts.len(), n);
            for p in pts {
                assert!((0.0..=1.0).contains(&p.x));
                assert!((0.0..=1.0).contains(&p.y));
            }
        }
    }

    #[test]
    fn single_node_centered() {
        assert_eq!(ring_layout(1)[0], Point { x: 0.5, y: 0.5 });
    }

    #[test]
    fn lerp_endpoints_and_midpoint() {
        let a = Point { x: 0.0, y: 0.0 };
        let b = Point { x: 1.0, y: 2.0 };
        assert_eq!(lerp(a, b, 0.0), a);
        assert_eq!(lerp(a, b, 1.0), b);
        assert_eq!(lerp(a, b, 0.5), Point { x: 0.5, y: 1.0 });
    }

    #[test]
    fn lerp_clamps_out_of_range() {
        let a = Point { x: 0.0, y: 0.0 };
        let b = Point { x: 1.0, y: 1.0 };
        assert_eq!(lerp(a, b, -1.0), a);
        assert_eq!(lerp(a, b, 2.0), b);
    }
}
