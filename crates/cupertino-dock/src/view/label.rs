//! The name of the tile under the pointer, over it: a popup of the bar shown as soon as the pointer is on a tile, a
//! capsule on a translucent veil with the name (and, for an application that runs without windows, a dimmer line under
//! it) and a pointer under it towards the tile. At the bottom of the output it stands over the bar, the pointer's tip a
//! little above the bar's top edge. It takes neither the pointer nor the keyboard.

use cupertino_kit::controls::CalloutPointer;
use cupertino_kit::core::{
    Change, CommandRegistry, Element, Event, EventCx, Glass, Key, MeasureCx, PaintCx, Popup,
    PopupAnchor, Propagation, Role, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Point, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured,
};
use cupertino_kit::platform::{DecorationMode, WindowSpec};
use cupertino_kit::render::{FillRule, Paint, PaintOp, PathVerb, Shape};
use cupertino_kit::theme::{Color, Surface};

use super::bar::LABEL_ANCHOR;
use super::paint;
use super::Input;
use crate::config::Orientation;

/// The capsule's height with one line and with two, the space either side of the text, and the name's size and weight.
pub const HEIGHT: f64 = 26.0;
pub const TWO_LINE_HEIGHT: f64 = 36.0;
pub const PADDING: f64 = 14.5;
pub const FONT_PT: f64 = 13.0;
const WEIGHT: u16 = 400;
/// The second line's size, and how far its centre line is below the first line's.
pub const STATUS_FONT_PT: f64 = 11.0;
const STATUS_DROP: f64 = 14.0;
/// The share of the name's colour the second line keeps.
const STATUS_SHARE: f32 = 0.6;
/// The capsule's corner radius: half a one-line capsule, and a two-line capsule keeps it.
pub const RADIUS: f64 = 13.0;
/// The pointer under the capsule: its width at the base, before its sides curve out into the capsule, and its depth.
pub const POINTER_WIDTH: f64 = 12.0;
pub const POINTER_DEPTH: f64 = 6.5;
/// How far the pointer's tip stands above the bar's top edge (the label's window ends at the tip).
pub const GAP: f64 = 3.0;
/// How far a label beside a bar at the side of the output stands off its tile.
const SIDE_GAP: f64 = 3.0;
/// The glass container the capsule and its pointer share, and how far their outlines blend into each other in points:
/// the pointer's sides curve out into the capsule's bottom edge as one piece of glass.
const GLASS_CONTAINER: u32 = 1;
const GLASS_BLEND: f64 = 4.0;
/// The capsule's fill where the session shows no material (Reduce Transparency, Increase Contrast, no desktop
/// effects), in the light and the dark appearance.
const FILL: [Color; 2] = [
    Color {
        r: 0.96,
        g: 0.96,
        b: 0.96,
        a: 0.96,
    },
    Color {
        r: 0.16,
        g: 0.16,
        b: 0.17,
        a: 0.96,
    },
];
/// The fine outline of the capsule, and the name's colour, in the light and the dark appearance.
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
        r: 0.976,
        g: 0.949,
        b: 0.945,
        a: 1.0,
    },
];
const OUTLINE_WIDTH: f64 = 0.5;

/// Asks for the label's glass under the node when the session shows materials; false where the label paints its own
/// opaque fill (Reduce Transparency, Increase Contrast, no desktop effects).
fn shown_on_glass(cx: &mut PaintCx<'_>, glass: Glass) -> bool {
    cx.env().theme.surface(Surface::Tooltip).material.is_some()
        && cx.glass(glass.container(GLASS_CONTAINER, GLASS_BLEND))
}

pub fn key() -> WindowKey {
    WindowKey(Key::from("tile-label"))
}

/// What the label shows.
pub struct LabelView<'a> {
    /// The bar's window, which the label is a popup of.
    pub parent: WindowKey,
    /// The tile it names (`TileId.0`).
    pub tile: u64,
    pub name: &'a str,
    /// A second, dimmer line under the name.
    pub status: Option<&'a str>,
    pub orientation: Orientation,
    /// How much of it shows: it fades with a bar that hides.
    pub opacity: f64,
}

/// The label of a tile.
pub fn window<M: Clone + 'static>(view: LabelView<'_>, input: fn(Input) -> M) -> Window<M> {
    let anchor = match view.orientation {
        // over the bar, the pointer's tip a little above its top edge
        Orientation::Bottom => PopupAnchor::CentredAbove {
            key: Key::from(LABEL_ANCHOR),
            gap: GAP,
        },
        Orientation::Left | Orientation::Right => PopupAnchor::Beside {
            key: Key::from(format!("tile-{}", view.tile)),
            offset: Point::new(SIDE_GAP, 0.0),
        },
    };
    let pointer = view.orientation == Orientation::Bottom;
    let height = if view.status.is_some() {
        TWO_LINE_HEIGHT
    } else {
        HEIGHT
    };
    Window {
        key: key(),
        spec: WindowSpec {
            title: view.name.to_owned(),
            app_id: String::new(),
            size: Size::new(600.0, height + POINTER_DEPTH),
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
            name: view.name.to_owned(),
            pointer,
            children: {
                let opacity = view.opacity.clamp(0.0, 1.0);
                let mut children = vec![Element::new(Body {
                    name: view.name.to_owned(),
                    status: view.status.map(str::to_owned),
                    pointer,
                    opacity,
                })
                .key("body")];
                if pointer {
                    children.push(
                        CalloutPointer {
                            width: POINTER_WIDTH,
                            depth: POINTER_DEPTH,
                            glass: Glass::popover()
                                .container(GLASS_CONTAINER, GLASS_BLEND)
                                .alpha(opacity),
                            surface: Surface::Tooltip,
                            fills_without_glass: false,
                            up: false,
                        }
                        .element()
                        .key("pointer"),
                    );
                }
                children
            },
        }),
        popup: Some(Popup {
            parent: view.parent,
            anchor,
            grab: false,
            on_dismiss: input(Input::LabelGone),
        }),
        menus: None,
    }
}

/// The outline of a capsule `w` × `h` with, when `pointer`, the pointer under its middle: the shape painted where
/// there is no material.
fn outline(w: f64, h: f64, pointer: bool) -> Vec<PathVerb> {
    let r = RADIUS;
    let k = 0.552_284_75 * r;
    let (l, t, b) = (0.0, 0.0, h);
    let right = w;
    let mid = w / 2.0;
    let half = POINTER_WIDTH / 2.0;
    let mut v = vec![
        PathVerb::MoveTo(Point::new(l + r, t)),
        PathVerb::LineTo(Point::new(right - r, t)),
        PathVerb::CubicTo(
            Point::new(right - r + k, t),
            Point::new(right, t + r - k),
            Point::new(right, t + r),
        ),
        PathVerb::LineTo(Point::new(right, b - r)),
        PathVerb::CubicTo(
            Point::new(right, b - r + k),
            Point::new(right - r + k, b),
            Point::new(right - r, b),
        ),
    ];
    if pointer {
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
            Point::new(l, b - r + k),
            Point::new(l, b - r),
        ),
        PathVerb::LineTo(Point::new(l, t + r)),
        PathVerb::CubicTo(
            Point::new(l, t + r - k),
            Point::new(l + r - k, t),
            Point::new(l + r, t),
        ),
        PathVerb::Close,
    ]);
    v
}

/// The popup's content: the capsule, with room under it for the pointer.
struct LabelNode<M> {
    /// What assistive technology calls the window: the tile's name, which the capsule under it reads.
    name: String,
    pointer: bool,
    children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for LabelNode<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Center,
            justify: Justify::Start,
        })
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.pointer != old.pointer {
            Change::Layout
        } else if self.name != old.name {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    // the window around the capsule, which assistive technology reads as the tooltip
    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.name(self.name.clone());
    }
}

/// The capsule: the label's glass under it (or its opaque fill), the pointer under it, and the text.
struct Body {
    name: String,
    status: Option<String>,
    pointer: bool,
    opacity: f64,
}

impl Body {
    fn height(&self) -> f64 {
        if self.status.is_some() {
            TWO_LINE_HEIGHT
        } else {
            HEIGHT
        }
    }
}

impl<M: Clone + 'static> Widget<M> for Body {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.name != old.name || self.status != old.status {
            Change::Layout
        } else if self.pointer != old.pointer || self.opacity != old.opacity {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let name = paint::width(cx.env(), &self.name, FONT_PT, WEIGHT);
        let status = self
            .status
            .as_deref()
            .map_or(0.0, |s| paint::width(cx.env(), s, STATUS_FONT_PT, WEIGHT));
        Measured {
            size: Size::new(name.max(status).ceil() + 2.0 * PADDING, self.height()),
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
        let (w, h) = (cx.size().width, cx.size().height);
        let shown = shown_on_glass(cx, Glass::popover().radius(RADIUS).alpha(self.opacity));
        if self.opacity < 1.0 {
            cx.set_group(self.opacity as f32, Vec::new());
        }
        // the glass lights its own rim, along the top and the bottom
        if !shown {
            let shape = Shape::Path {
                verbs: outline(w, h, self.pointer),
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
        }
        let colour = TEXT[mode];
        let (first, second) = if self.status.is_some() {
            ((h - STATUS_DROP) / 2.0, (h + STATUS_DROP) / 2.0)
        } else {
            (h / 2.0, 0.0)
        };
        let centred = |cx: &mut PaintCx<'_>, s: &str, size: f64, cy: f64, c: Color| {
            let Some(line) = paint::line(&env, s, size, WEIGHT, Some(w)) else {
                return;
            };
            let (tw, th) = (line.size().width, line.size().height);
            paint::glyphs(cx, &line, Point::new((w - tw) / 2.0, cy - th / 2.0), c);
        };
        centred(cx, &self.name, FONT_PT, first, colour);
        if let Some(status) = &self.status {
            let dim = Color {
                a: colour.a * STATUS_SHARE,
                ..colour
            };
            centred(cx, status, STATUS_FONT_PT, second, dim);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Tooltip);
        cx.name(self.name.clone());
        if let Some(status) = &self.status {
            cx.description(status.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_capsule_keeps_14_5_points_either_side_of_the_name_over_a_12_point_pointer() {
        // a five letter name about 34 points wide at 13 points makes a 63 point capsule
        assert_eq!(34.0 + 2.0 * PADDING, 63.0);
        assert_eq!((POINTER_WIDTH, POINTER_DEPTH), (12.0, 6.5));
    }

    #[test]
    fn the_pointer_and_capsule_blend_over_no_more_than_a_third_of_the_pointer_so_it_still_tapers() {
        // a wider blend melts the pointer's narrowing layers into a blunt round blob under the capsule
        assert!(GLASS_BLEND <= POINTER_WIDTH / 3.0, "{GLASS_BLEND}");
    }
}
