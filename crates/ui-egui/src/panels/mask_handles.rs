//! Radial gradient handles on the photo: where they sit and what dragging one does to the shape.
//!
//! Positions are normalized photo coordinates; radii are long-edge units, like the shape itself.
//! `l` converts long-edge units to normalized x and y (see `detail::frame_long_norm`).

use lightcraft_develop::MaskShape;
use lightcraft_geom::Point;

/// Handle ids of a radial gradient (0 is a component's pin, 1 and 2 a linear gradient's ends).
pub const RIGHT: u8 = 3;
pub const LEFT: u8 = 4;
pub const BOTTOM: u8 = 5;
pub const TOP: u8 = 6;
pub const ROTATE: u8 = 7;
pub const FEATHER: u8 = 8;

/// The smallest radius a drag leaves (long-edge units), so the ellipse stays visible and grabbable.
pub const MIN_RADIUS: f64 = 0.005;

/// Shift-rotation snaps to this many degrees.
const SNAP_DEG: f64 = 15.0;

/// `(x, y)` in the gradient's own rotated frame (long-edge units) → normalized photo coordinates.
pub fn local_to_norm(center: Point, angle: f64, x: f64, y: f64, l: (f64, f64)) -> Point {
    let (s, co) = angle.to_radians().sin_cos();
    Point::new(center.x + (x * co - y * s) * l.0, center.y + (x * s + y * co) * l.1)
}

/// Normalized `p` → the gradient's rotated frame (long-edge units); the inverse of [`local_to_norm`].
pub fn norm_to_local(center: Point, angle: f64, p: Point, l: (f64, f64)) -> (f64, f64) {
    let (dx, dy) = ((p.x - center.x) / l.0, (p.y - center.y) / l.1);
    let (s, co) = angle.to_radians().sin_cos();
    (dx * co + dy * s, -dx * s + dy * co)
}

/// The resize handles (right, left, bottom, top) and the feather handle of a radial gradient, with
/// their normalized positions. The feather handle sits on the inner ellipse where the fade starts.
/// Other shapes have none. The rotate handle is placed by the caller in screen space, a fixed
/// distance beyond the top handle.
pub fn handles(shape: &MaskShape, l: (f64, f64)) -> Vec<(u8, Point)> {
    let MaskShape::Radial { center, rx, ry, angle, feather, .. } = *shape else { return Vec::new() };
    let at = |x: f64, y: f64| local_to_norm(center, angle, x, y, l);
    let inner = 1.0 - (feather / 100.0).clamp(0.0, 1.0);
    vec![(RIGHT, at(rx, 0.0)), (LEFT, at(-rx, 0.0)), (BOTTOM, at(0.0, ry)), (TOP, at(0.0, -ry)), (FEATHER, at(rx * inner, 0.0))]
}

/// `shape` after dragging radial `handle` to `at` (normalized): a side handle sets that radius to
/// the pointer's distance from the centre (`proportional`, i.e. Shift, scales the other radius by
/// the same factor), the rotate handle points the top of the ellipse at the pointer
/// (`proportional` snaps to 15°), the feather handle sets where the fade starts. Anything else
/// comes back unchanged.
pub fn dragged(shape: &MaskShape, handle: u8, at: Point, l: (f64, f64), proportional: bool) -> MaskShape {
    let MaskShape::Radial { center, rx, ry, angle, feather, invert } = shape.clone() else { return shape.clone() };
    let (x, y) = norm_to_local(center, angle, at, l);
    let (mut rx, mut ry, mut angle, mut feather) = (rx, ry, angle, feather);
    match handle {
        RIGHT | LEFT => {
            let r = x.abs().max(MIN_RADIUS);
            if proportional {
                ry = (ry * r / rx.max(MIN_RADIUS)).max(MIN_RADIUS);
            }
            rx = r;
        }
        BOTTOM | TOP => {
            let r = y.abs().max(MIN_RADIUS);
            if proportional {
                rx = (rx * r / ry.max(MIN_RADIUS)).max(MIN_RADIUS);
            }
            ry = r;
        }
        ROTATE => {
            // the angle that puts the ellipse's top (local (0, -r)) under the pointer
            let (dx, dy) = ((at.x - center.x) / l.0, (at.y - center.y) / l.1);
            if dx.hypot(dy) > 1e-9 {
                let mut a = dx.atan2(-dy).to_degrees();
                if proportional {
                    a = (a / SNAP_DEG).round() * SNAP_DEG;
                }
                // keep it in (-180, 180]
                angle = if a <= -180.0 { a + 360.0 } else { a };
            }
        }
        FEATHER => {
            let inner = (x.abs() / rx.max(MIN_RADIUS)).clamp(0.0, 1.0);
            feather = ((1.0 - inner) * 100.0).clamp(0.0, 100.0);
        }
        _ => {}
    }
    MaskShape::Radial { center, rx, ry, angle, feather, invert }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3:2 landscape photo: one long-edge unit is 1 in x and 1.5 in y (normalized).
    const L: (f64, f64) = (1.0, 1.5);

    fn radial(rx: f64, ry: f64, angle: f64, feather: f64) -> MaskShape {
        MaskShape::Radial { center: Point::new(0.5, 0.5), rx, ry, angle, feather, invert: true }
    }

    fn parts(s: &MaskShape) -> (Point, f64, f64, f64, f64, bool) {
        let MaskShape::Radial { center, rx, ry, angle, feather, invert } = *s else { panic!("radial expected, got {s:?}") };
        (center, rx, ry, angle, feather, invert)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn local_and_normalized_round_trip() {
        let c = Point::new(0.4, 0.6);
        for angle in [0.0, 30.0, -75.0, 180.0] {
            let p = local_to_norm(c, angle, 0.12, -0.05, L);
            let (x, y) = norm_to_local(c, angle, p, L);
            assert!(close(x, 0.12) && close(y, -0.05), "{angle}: {x} {y}");
        }
    }

    #[test]
    fn handles_sit_on_the_ellipse_and_feather_ring() {
        let h = handles(&radial(0.2, 0.1, 0.0, 50.0), L);
        let at = |id: u8| h.iter().find(|(i, _)| *i == id).map(|(_, p)| *p).unwrap();
        assert!(close(at(RIGHT).x, 0.7) && close(at(RIGHT).y, 0.5));
        assert!(close(at(LEFT).x, 0.3));
        // 0.1 long-edge units down is 0.15 normalized on a 3:2 photo
        assert!(close(at(BOTTOM).y, 0.65) && close(at(TOP).y, 0.35));
        // feather 50: the fade starts halfway out
        assert!(close(at(FEATHER).x, 0.6));
        // a quarter turn puts the right handle below the centre
        let r = handles(&radial(0.2, 0.1, 90.0, 0.0), L);
        let right = r.iter().find(|(i, _)| *i == RIGHT).unwrap().1;
        assert!(close(right.x, 0.5) && close(right.y, 0.5 + 0.2 * 1.5), "{right:?}");
        // other shapes have no radial handles
        assert!(handles(&MaskShape::Sky, L).is_empty());
    }

    #[test]
    fn side_handles_set_one_radius() {
        // dragging the right handle out to x = 0.8 makes rx 0.3; ry, centre, angle, feather, invert stay
        let (c, rx, ry, a, f, inv) = parts(&dragged(&radial(0.2, 0.1, 0.0, 40.0), RIGHT, Point::new(0.8, 0.52), L, false));
        assert!(close(rx, 0.3) && close(ry, 0.1) && close(c.x, 0.5) && close(c.y, 0.5) && close(a, 0.0) && close(f, 40.0) && inv);
        // the left handle works the same from the other side
        assert!(close(parts(&dragged(&radial(0.2, 0.1, 0.0, 40.0), LEFT, Point::new(0.45, 0.5), L, false)).1, 0.05));
        // the top handle sets ry (0.15 normalized up = 0.1 long-edge units)
        let (_, rx, ry, ..) = parts(&dragged(&radial(0.2, 0.1, 0.0, 40.0), TOP, Point::new(0.5, 0.2), L, false));
        assert!(close(rx, 0.2) && close(ry, 0.2), "{rx} {ry}");
        // rotated 90°: the right handle lies along the photo's y axis
        let (_, rx, ..) = parts(&dragged(&radial(0.2, 0.1, 90.0, 0.0), RIGHT, Point::new(0.5, 0.5 + 0.25 * 1.5), L, false));
        assert!(close(rx, 0.25), "{rx}");
    }

    #[test]
    fn shift_resizes_proportionally() {
        let (_, rx, ry, ..) = parts(&dragged(&radial(0.2, 0.1, 0.0, 0.0), RIGHT, Point::new(0.9, 0.5), L, true));
        assert!(close(rx, 0.4) && close(ry, 0.2), "{rx} {ry}");
        let (_, rx, ry, ..) = parts(&dragged(&radial(0.2, 0.1, 0.0, 0.0), BOTTOM, Point::new(0.5, 0.5 + 0.05 * 1.5), L, true));
        assert!(close(rx, 0.1) && close(ry, 0.05), "{rx} {ry}");
    }

    #[test]
    fn radii_never_collapse() {
        // dragging a handle onto (or past) the centre leaves the minimum radius, never zero or negative
        let (_, rx, ry, ..) = parts(&dragged(&radial(0.2, 0.1, 0.0, 0.0), RIGHT, Point::new(0.5, 0.5), L, true));
        assert!(rx >= MIN_RADIUS && ry >= MIN_RADIUS && rx.is_finite() && ry.is_finite(), "{rx} {ry}");
        let (_, rx, ..) = parts(&dragged(&radial(0.2, 0.1, 0.0, 0.0), RIGHT, Point::new(0.1, 0.5), L, false));
        assert!(close(rx, 0.4), "past the centre the radius is the distance: {rx}");
        // a degenerate ellipse can still be grown back, also proportionally
        let (_, rx, ry, ..) = parts(&dragged(&radial(0.0, 0.0, 0.0, 0.0), RIGHT, Point::new(0.6, 0.5), L, true));
        assert!(close(rx, 0.1) && ry >= MIN_RADIUS && ry.is_finite(), "{rx} {ry}");
    }

    #[test]
    fn rotate_handle_points_the_top_at_the_pointer() {
        let rot = |at: Point, snap: bool| parts(&dragged(&radial(0.2, 0.1, 10.0, 0.0), ROTATE, at, L, snap)).3;
        assert!(close(rot(Point::new(0.5, 0.2), false), 0.0), "straight up");
        assert!(close(rot(Point::new(0.8, 0.5), false), 90.0), "right is a quarter turn clockwise");
        assert!(close(rot(Point::new(0.2, 0.5), false), -90.0));
        assert!(close(rot(Point::new(0.5, 0.9), false), 180.0), "straight down stays in (-180, 180]");
        // 30° from vertical, measured in long-edge units (x 0.1, y -0.1·√3 → normalized ×1.5)
        let at = Point::new(0.5 + 0.1, 0.5 - 0.1 * 3f64.sqrt() * 1.5);
        assert!((rot(at, false) - 30.0).abs() < 1e-6);
        // Shift snaps to 15°
        let at = Point::new(0.5 + 0.2 * 0.38f64.sin(), 0.5 - 0.2 * 0.38f64.cos() * 1.5);
        assert!(close(rot(at, true), 15.0), "21.8° snaps to 15°: {}", rot(at, true));
        // the pointer on the centre keeps the angle
        assert!(close(rot(Point::new(0.5, 0.5), false), 10.0));
    }

    #[test]
    fn feather_handle_sets_where_the_fade_starts() {
        let fe = |x: f64| parts(&dragged(&radial(0.2, 0.1, 0.0, 50.0), FEATHER, Point::new(x, 0.5), L, false)).4;
        assert!(close(fe(0.5), 100.0), "at the centre: fully feathered");
        assert!(close(fe(0.6), 50.0));
        assert!(close(fe(0.65), 25.0));
        assert!(close(fe(0.7), 0.0), "on the edge: hard");
        assert!(close(fe(0.95), 0.0), "beyond the edge clamps");
        assert!(close(fe(0.35), 25.0), "either side of the centre");
    }

    #[test]
    fn unknown_handles_and_other_shapes_are_unchanged() {
        let r = radial(0.2, 0.1, 5.0, 30.0);
        assert_eq!(dragged(&r, 0, Point::new(0.9, 0.9), L, true), r);
        assert_eq!(dragged(&r, 99, Point::new(0.9, 0.9), L, false), r);
        let lin = MaskShape::Linear { start: Point::new(0.1, 0.1), end: Point::new(0.2, 0.2) };
        assert_eq!(dragged(&lin, RIGHT, Point::new(0.9, 0.9), L, false), lin);
    }
}
