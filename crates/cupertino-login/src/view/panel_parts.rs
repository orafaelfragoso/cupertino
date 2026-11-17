//! What the session panels have besides the standard controls: the area that moves a panel when its background is
//! dragged, and the picture at its top (power, restart, log out), drawn from lines and arcs.

use cupertino_kit::core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Phase, Propagation, SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Point, Rect};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_kit::platform::{MouseButton, PointerEvent};
use cupertino_kit::render::{FillRule, GradientStop, Paint, PaintOp, PathVerb, Shape};
use cupertino_kit::theme::Color;

use crate::panel::Icon;

/// The panel's content over an area that moves the panel: a left press that no control inside took, and the pointer
/// moved with the button held, asks for the panel to move by how far the pointer went from the press (`on_move`).
/// The panel's surface follows, so the pointer stays over the point it pressed and each motion asks for the rest.
pub(crate) struct Movable<M> {
    pub on_move: Box<dyn Fn(Point) -> M>,
    pub children: Vec<Element<M>>,
}

/// Where the held press went down, in the panel's points.
#[derive(Debug, Default)]
pub(crate) struct Held(Option<Point>);

impl<M: 'static> Widget<M> for Movable<M> {
    type State = Held;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> Held {
        Held::default()
    }

    fn update(&self, _: &Self, _: &mut Held) -> Change {
        Change::None
    }

    fn measure(&self, _: &Held, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: cupertino_kit::geometry::Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, held: &mut Held, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        // controls inside take their presses on the way down; what reaches here on the way up is the background's
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match *event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    position,
                    ..
                },
                _,
            ) => {
                held.0 = Some(position);
                cx.capture_pointer();
                Propagation::Stop
            }
            Event::Pointer(PointerEvent::Motion { position }, _) => match held.0 {
                Some(press) => {
                    let by = drag_step(press, position);
                    if by != Point::ZERO {
                        cx.emit((self.on_move)(by));
                    }
                    Propagation::Stop
                }
                None => Propagation::Continue,
            },
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) if held.0.is_some() => {
                held.0 = None;
                cx.release_pointer();
                Propagation::Stop
            }
            Event::Cancel => {
                held.0 = None;
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &Held, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &Held, _: &mut SemanticsCx<'_>) {}
}

/// The controls of one question of a panel over the whole panel, as a focus scope: a new question (a new key) gives
/// the focus to its control marked as the first, and when it goes the focus goes back to where it was before it.
pub(crate) struct Question<M> {
    pub children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Question<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: cupertino_kit::geometry::Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.focus_scope();
    }
}

/// How far the panel moves for a pointer at `position` after a press at `press` (both in the panel's points): the
/// whole way, in whole points, so the surface lands on the point grid and the pointer stays over what it pressed.
pub(crate) fn drag_step(press: Point, position: Point) -> Point {
    Point::new(
        (position.x - press.x).round(),
        (position.y - press.y).round(),
    )
}

/// The picture's disc: a grey that darkens towards its foot, under a white glyph.
const DISC_TOP: Color = Color::new(0.62, 0.64, 0.68, 1.0);
const DISC_BOTTOM: Color = Color::new(0.45, 0.47, 0.51, 1.0);
const GLYPH: Color = Color::new(1.0, 1.0, 1.0, 1.0);
/// The glyph's line width and the radius of its ring, as shares of the disc's side.
const GLYPH_LINE: f64 = 0.07;
const GLYPH_RING: f64 = 0.26;
/// How far the ring of the power glyph opens each side of its top, in radians; and where the restart glyph's ring
/// starts right of the top and ends left of it, its opening at the upper right.
const POWER_OPENING: f64 = 0.62;
const RESTART_START: f64 = 0.95;
const RESTART_END: f64 = 0.1;
/// The restart glyph's arrow head: its length along the ring's end, as a share of the side.
const ARROW_HEAD: f64 = 0.12;

/// The ops of the picture `icon` in a square `side` points wide from the origin: the disc, then the glyph's strokes.
pub(crate) fn icon_ops(icon: Icon, side: f64) -> Vec<PaintOp> {
    let mut ops = vec![PaintOp::Fill {
        shape: Shape::RoundedRect {
            rect: Rect::new(0.0, 0.0, side, side),
            radius: side / 2.0,
        },
        paint: Paint::Linear {
            start: Point::new(0.0, 0.0),
            end: Point::new(0.0, side),
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: DISC_TOP,
                },
                GradientStop {
                    offset: 1.0,
                    color: DISC_BOTTOM,
                },
            ],
        },
    }];
    let width = side * GLYPH_LINE;
    ops.extend(glyph(icon, side).into_iter().map(|shape| PaintOp::Stroke {
        shape,
        width,
        paint: Paint::Solid(GLYPH),
    }));
    ops
}

/// The strokes of the glyph of `icon`, centred in a square `side` wide.
pub(crate) fn glyph(icon: Icon, side: f64) -> Vec<Shape> {
    let c = Point::new(side / 2.0, side / 2.0);
    let r = side * GLYPH_RING;
    let at = |angle: f64, radius: f64| {
        Point::new(c.x + radius * angle.cos(), c.y + radius * angle.sin())
    };
    // angles in radians from the positive x axis, y down: the top of the ring is -pi/2
    let top = -std::f64::consts::FRAC_PI_2;
    match icon {
        Icon::Power => vec![
            arc(
                c,
                r,
                top + POWER_OPENING,
                top + std::f64::consts::TAU - POWER_OPENING,
            ),
            line(
                Point::new(c.x, c.y - r * 1.25),
                Point::new(c.x, c.y - r * 0.15),
            ),
        ],
        Icon::Restart => {
            let start = top + RESTART_START;
            let end = top + std::f64::consts::TAU - RESTART_END;
            let tip = at(end, r);
            // the head points along the ring at its end, clockwise from it
            let along = Point::new(-(end.sin()), end.cos());
            let out = Point::new(end.cos(), end.sin());
            let h = side * ARROW_HEAD;
            vec![
                arc(c, r, start, end),
                polyline(&[
                    Point::new(
                        tip.x - along.x * h * 0.2 + out.x * h * 0.7,
                        tip.y - along.y * h * 0.2 + out.y * h * 0.7,
                    ),
                    Point::new(tip.x + along.x * h, tip.y + along.y * h),
                    Point::new(
                        tip.x - along.x * h * 0.2 - out.x * h * 0.7,
                        tip.y - along.y * h * 0.2 - out.y * h * 0.7,
                    ),
                ]),
            ]
        }
        Icon::LogOut => {
            // a door frame open on its right, and an arrow leaving it to the right
            let (x0, x1) = (c.x - r * 1.05, c.x + r * 0.15);
            let (y0, y1) = (c.y - r * 1.05, c.y + r * 1.05);
            let tip = Point::new(c.x + r * 1.2, c.y);
            let h = side * ARROW_HEAD * 0.9;
            vec![
                polyline(&[
                    Point::new(x1, y0 + r * 0.35),
                    Point::new(x1, y0),
                    Point::new(x0, y0),
                    Point::new(x0, y1),
                    Point::new(x1, y1),
                    Point::new(x1, y1 - r * 0.35),
                ]),
                line(Point::new(c.x - r * 0.35, c.y), tip),
                polyline(&[
                    Point::new(tip.x - h, tip.y - h),
                    tip,
                    Point::new(tip.x - h, tip.y + h),
                ]),
            ]
        }
    }
}

fn line(from: Point, to: Point) -> Shape {
    polyline(&[from, to])
}

fn polyline(points: &[Point]) -> Shape {
    let mut verbs = vec![PathVerb::MoveTo(points[0])];
    verbs.extend(points[1..].iter().map(|&p| PathVerb::LineTo(p)));
    Shape::Path {
        verbs,
        fill_rule: FillRule::NonZero,
    }
}

/// A circular arc about `c` of radius `r` from angle `from` to `to` (radians, clockwise on screen as they grow), as
/// cubic segments of at most a quarter turn each.
fn arc(c: Point, r: f64, from: f64, to: f64) -> Shape {
    let segments = ((to - from).abs() / std::f64::consts::FRAC_PI_2)
        .ceil()
        .max(1.0) as usize;
    let step = (to - from) / segments as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan() * r;
    let at = |a: f64| Point::new(c.x + r * a.cos(), c.y + r * a.sin());
    let mut verbs = vec![PathVerb::MoveTo(at(from))];
    for i in 0..segments {
        let (a0, a1) = (from + step * i as f64, from + step * (i + 1) as f64);
        let (p0, p1) = (at(a0), at(a1));
        verbs.push(PathVerb::CubicTo(
            Point::new(p0.x - k * a0.sin(), p0.y + k * a0.cos()),
            Point::new(p1.x + k * a1.sin(), p1.y - k * a1.cos()),
            p1,
        ));
    }
    Shape::Path {
        verbs,
        fill_rule: FillRule::NonZero,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(shape: &Shape) -> Vec<Point> {
        let Shape::Path { verbs, .. } = shape else {
            panic!("a glyph is drawn of paths")
        };
        verbs
            .iter()
            .flat_map(|v| match *v {
                PathVerb::MoveTo(p) | PathVerb::LineTo(p) => vec![p],
                PathVerb::QuadTo(a, p) => vec![a, p],
                PathVerb::CubicTo(a, b, p) => vec![a, b, p],
                PathVerb::Close => vec![],
            })
            .collect()
    }

    #[test]
    fn each_glyph_stays_inside_its_disc_with_room_for_its_line() {
        let side = 64.0;
        let c = Point::new(32.0, 32.0);
        for icon in [Icon::Power, Icon::Restart, Icon::LogOut] {
            let shapes = glyph(icon, side);
            assert!(shapes.len() >= 2, "{icon:?}");
            for p in shapes.iter().flat_map(points) {
                let d = ((p.x - c.x).powi(2) + (p.y - c.y).powi(2)).sqrt();
                assert!(
                    d + side * GLYPH_LINE <= side / 2.0,
                    "{icon:?}: {p:?} reaches the disc's edge"
                );
            }
            assert_eq!(
                icon_ops(icon, side).len(),
                shapes.len() + 1,
                "the disc, then the strokes"
            );
        }
    }

    #[test]
    fn the_power_ring_opens_at_the_top_where_its_bar_stands() {
        let shapes = glyph(Icon::Power, 64.0);
        let ring = points(&shapes[0]);
        let (first, last) = (ring[0], *ring.last().unwrap());
        assert!(
            (first.x - (64.0 - last.x)).abs() < 1e-9
                && first.y < 32.0
                && (first.y - last.y).abs() < 1e-9,
            "the ends mirror each other above the middle: {first:?} {last:?}"
        );
        let bar = points(&shapes[1]);
        assert!(
            bar.iter().all(|p| p.x == 32.0),
            "the bar is upright in the middle"
        );
        // a point of the arc halfway lies on the ring
        let mid = ring[ring.len() / 2];
        let d = ((mid.x - 32.0).powi(2) + (mid.y - 32.0).powi(2)).sqrt();
        assert!((d - 64.0 * GLYPH_RING).abs() < 0.2, "{d}");
    }

    #[test]
    fn a_drag_moves_the_whole_way_from_the_press_in_whole_points() {
        let press = Point::new(40.0, 12.0);
        assert_eq!(drag_step(press, Point::new(40.2, 11.9)), Point::ZERO);
        assert_eq!(
            drag_step(press, Point::new(52.6, 2.4)),
            Point::new(13.0, -10.0)
        );
    }
}
