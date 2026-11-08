//! The name of the tile under the pointer, over it: a popup of the bar shown as soon as the pointer is on a tile, a
//! capsule with the name and a pointer under it towards the tile. It takes neither the pointer nor the keyboard.

use cupertino_kit::core::{
    Change, CommandRegistry, Element, Event, EventCx, Key, MeasureCx, PaintCx, Popup, PopupAnchor,
    Propagation, Role, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Point, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_kit::platform::{DecorationMode, WindowSpec};
use cupertino_kit::render::{FillRule, Paint, PaintOp, PathVerb, Shape};
use cupertino_kit::theme::Color;

use super::paint;
use super::Input;
use crate::config::Orientation;

/// The capsule's height, the space either side of the name, and the name's size and weight.
pub const HEIGHT: f64 = 25.0;
pub const PADDING: f64 = 13.0;
pub const FONT_PT: f64 = 13.0;
const WEIGHT: u16 = 400;
/// The pointer under the capsule: its width at the capsule and its depth.
pub const POINTER_WIDTH: f64 = 16.0;
pub const POINTER_DEPTH: f64 = 6.5;
/// How far the pointer's tip stands off the tile.
pub const GAP: f64 = 3.0;
/// The capsule's fill and its fine outline, and the name's colour, in the light and the dark appearance.
const FILL: [Color; 2] = [
    Color {
        r: 0.96,
        g: 0.96,
        b: 0.96,
        a: 0.88,
    },
    Color {
        r: 0.16,
        g: 0.16,
        b: 0.17,
        a: 0.86,
    },
];
const OUTLINE: [Color; 2] = [
    Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.12,
    },
    Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 0.22,
    },
];
const TEXT: [Color; 2] = [
    Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.85,
    },
    Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 0.92,
    },
];
const OUTLINE_WIDTH: f64 = 0.5;

pub fn key() -> WindowKey {
    WindowKey(Key::from("tile-label"))
}

/// The label of tile `tile` (`TileId.0`), named `name`, over it in the bar's window `parent`.
pub fn window<M: Clone + 'static>(
    parent: WindowKey,
    tile: u64,
    name: &str,
    orientation: Orientation,
    input: fn(Input) -> M,
) -> Window<M> {
    let anchor = Key::from(format!("tile-{tile}"));
    let anchor = match orientation {
        // over the tile: below has no room at the bottom of the output, so the popup flips
        Orientation::Bottom => PopupAnchor::Below {
            key: anchor,
            gap: GAP,
        },
        Orientation::Left | Orientation::Right => PopupAnchor::Beside {
            key: anchor,
            offset: Point::new(GAP, 0.0),
        },
    };
    Window {
        key: key(),
        spec: WindowSpec {
            title: name.to_owned(),
            app_id: String::new(),
            size: Size::new(600.0, HEIGHT + POINTER_DEPTH),
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background: WindowBackground::None,
        content: Element::new(LabelNode {
            name: name.to_owned(),
            pointer: orientation == Orientation::Bottom,
        }),
        popup: Some(Popup {
            parent,
            anchor,
            grab: false,
            on_dismiss: input(Input::LabelGone),
        }),
        menus: None,
    }
}

/// The outline of the capsule `w` × [`HEIGHT`] with, when `pointer`, the pointer under its middle.
fn outline(w: f64, pointer: bool) -> Vec<PathVerb> {
    let r = HEIGHT / 2.0;
    let k = 0.552_284_75 * r;
    let (l, t, b) = (0.0, 0.0, HEIGHT);
    let right = w;
    let mid = w / 2.0;
    let half = POINTER_WIDTH / 2.0;
    let mut v = vec![
        PathVerb::MoveTo(Point::new(l + r, t)),
        PathVerb::LineTo(Point::new(right - r, t)),
        PathVerb::CubicTo(
            Point::new(right - r + k, t),
            Point::new(right, b - r - k),
            Point::new(right, t + r),
        ),
        PathVerb::CubicTo(
            Point::new(right, t + r + k),
            Point::new(right - r + k, b),
            Point::new(right - r, b),
        ),
    ];
    if pointer {
        // a soft pointer: its sides curve in to a rounded tip
        let tip = b + POINTER_DEPTH;
        v.extend([
            PathVerb::LineTo(Point::new(mid + half, b)),
            PathVerb::CubicTo(
                Point::new(mid + half * 0.45, b),
                Point::new(mid + 1.5, tip),
                Point::new(mid, tip),
            ),
            PathVerb::CubicTo(
                Point::new(mid - 1.5, tip),
                Point::new(mid - half * 0.45, b),
                Point::new(mid - half, b),
            ),
        ]);
    }
    v.extend([
        PathVerb::LineTo(Point::new(l + r, b)),
        PathVerb::CubicTo(
            Point::new(l + r - k, b),
            Point::new(l, t + r + k),
            Point::new(l, t + r),
        ),
        PathVerb::CubicTo(
            Point::new(l, t + r - k),
            Point::new(l + r - k, t),
            Point::new(l + r, t),
        ),
        PathVerb::Close,
    ]);
    v
}

struct LabelNode {
    name: String,
    pointer: bool,
}

impl<M: Clone + 'static> Widget<M> for LabelNode {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.name != old.name || self.pointer != old.pointer {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let width = paint::width(cx.env(), &self.name, FONT_PT, WEIGHT);
        Measured {
            size: Size::new(width.ceil() + 2.0 * PADDING, HEIGHT + POINTER_DEPTH),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let mode = usize::from(paint::is_dark(&env));
        let w = cx.size().width;
        let shape = Shape::Path {
            verbs: outline(w, self.pointer),
            fill_rule: FillRule::NonZero,
        };
        cx.push(PaintOp::Fill {
            shape: shape.clone(),
            paint: Paint::Solid(FILL[mode]),
        });
        cx.push(PaintOp::Stroke {
            shape,
            width: OUTLINE_WIDTH,
            paint: Paint::Solid(OUTLINE[mode]),
        });
        let Some(text) = paint::line(&env, &self.name, FONT_PT, WEIGHT, Some(w)) else {
            return;
        };
        let (tw, th) = (text.size().width, text.size().height);
        let origin = Point::new((w - tw) / 2.0, (HEIGHT - th) / 2.0);
        paint::glyphs(cx, &text, origin, TEXT[mode]);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Tooltip);
        cx.name(self.name.clone());
    }
}
