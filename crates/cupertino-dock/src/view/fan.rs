//! A folder's stack as a fan: a popup of the bar standing over the folder's tile with a column of rows rising from it in
//! an arc, each a small icon with its name on a pill to its left and turned a little more the higher it stands, and a
//! round button at the top that opens the folder. The rows rise out of the tile as it opens and fall back into it as it
//! closes. It takes the keyboard and closes on a press outside it or Escape; the model moves the selection with the
//! arrows. Where the parts go is [`FanLayout`]'s.

use cupertino_kit::core::{
    Action, Change, CommandRegistry, Element, Event, EventCx, Key, MeasureCx, PaintCx, Popup,
    PopupAnchor, Propagation, Role, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{Constraints, LayoutSpec, Measured};
use cupertino_kit::platform::{DecorationMode, WindowSpec};
use cupertino_kit::render::{FillRule, Paint, PaintOp, PathVerb, Shape};
use cupertino_kit::theme::Color;

use super::bar::FAN_ANCHOR;
use super::paint::{self, Root};
use super::{IconSet, Input};
use crate::fan::{
    FanLayout, ICON, PILL_FONT_PT, PILL_GAP, PILL_HEIGHT, PILL_MAX_WIDTH, PILL_PADDING, PITCH,
};
use crate::stack::{StackEntry, OPEN_FOLDER};

/// A pill's fill in the light and the dark appearance, and with the pointer or the keyboard on its row.
const PILL: [Color; 2] = [
    Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 0.55,
    },
    Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 0.18,
    },
];
const PILL_HIGHLIGHT: [Color; 2] = [
    Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 0.85,
    },
    Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 0.34,
    },
];
/// A pill's fine rim, and its text, in the light and the dark appearance.
const RIM: [Color; 2] = [
    Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 0.5,
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
const RIM_WIDTH: f64 = 0.5;
/// The button's arrow: how far its ends are from the button's middle, and its stroke.
const ARROW_REACH: f64 = 8.0;
const ARROW_STROKE: f64 = 3.0;
/// How far a pill's background has grown when the fan starts to open, as a share of its width.
const PILL_START: f64 = 0.4;

pub fn key() -> WindowKey {
    WindowKey(Key::from("fan"))
}

/// What the fan shows this frame.
pub struct FanView<'a> {
    /// The bar's window, which the fan is a popup of.
    pub parent: WindowKey,
    pub name: &'a str,
    pub entries: &'a [StackEntry],
    /// Icons by their theme name.
    pub icons: &'a dyn Fn(&str) -> IconSet,
    pub layout: FanLayout,
    /// How far the fan has come out, from 0 to 1.
    pub progress: f64,
    /// The row the pointer or the keyboard is on.
    pub highlighted: Option<usize>,
    /// The fan moves by itself: its window sends [`Input::Frame`] at every frame.
    pub moving: bool,
}

pub fn window<M: Clone + 'static>(view: FanView<'_>, input: fn(Input) -> M) -> Window<M> {
    let layout = view.layout;
    let names: Vec<&str> = view.entries.iter().map(|e| e.name.as_str()).collect();
    let rows = layout.rows();
    let mut children = Vec::with_capacity(rows);
    for row in 0..rows {
        let entry = view.entries.get(row);
        let label = entry.map_or(OPEN_FOLDER, |e| e.name.as_str());
        children.push(
            Element::new(RowNode {
                index: row,
                label: label.to_owned(),
                button: entry.is_none(),
                image: entry.map_or_else(Vec::new, |e| (view.icons)(&e.icon)),
                centre: {
                    let c = layout.centre(row, view.progress);
                    Point::new(c.x, c.y)
                },
                tilt: layout.tilt(row, view.progress),
                progress: view.progress,
                highlighted: view.highlighted == Some(row),
                width: layout.width,
                input,
            })
            .key(Key::from(format!("row-{row}"))),
        );
    }
    // the part of the window that takes the pointer: the rows at rest, once the fan is out
    let region = (view.progress >= 1.0).then(|| {
        (0..rows)
            .map(|row| {
                let r = layout.hit_rect(row, names.get(row).copied().unwrap_or(OPEN_FOLDER));
                Rect::new(r.x, r.y, r.w, r.h)
            })
            .collect()
    });
    Window {
        key: key(),
        spec: WindowSpec {
            title: view.name.to_owned(),
            app_id: String::new(),
            size: Size::new(layout.width, layout.height),
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: region,
        },
        shortcuts: CommandRegistry::new(),
        on_close: Some(input(Input::StackClosed)),
        background: WindowBackground::None,
        content: Element::new(Root {
            role: Role::Dialog,
            name: view.name.to_owned(),
            pointer: Some(Box::new(move |p| input(Input::Stack(p)))),
            key: Some(Box::new(move |k| Some(input(Input::StackKey(k))))),
            roving: true,
            on_blur: None,
            alpha: 1.0,
            revision: (view.progress * 1000.0) as u64,
            under: None,
            frame: view.moving.then(|| input(Input::Frame)),
            children,
        }),
        popup: Some(Popup {
            parent: view.parent,
            anchor: PopupAnchor::Below {
                key: Key::from(FAN_ANCHOR),
                gap: 0.0,
            },
            grab: true,
            on_dismiss: input(Input::StackClosed),
        }),
        menus: None,
    }
}

/// `r` turned clockwise by `sin_cos` of an angle about `pivot`.
fn turned(p: Point, pivot: Point, (sin, cos): (f64, f64)) -> Point {
    let (x, y) = (p.x - pivot.x, p.y - pivot.y);
    Point::new(pivot.x + x * cos - y * sin, pivot.y + x * sin + y * cos)
}

/// The outline of the rounded rectangle `r` with round ends, turned clockwise by `angle` about `pivot`.
fn pill_path(r: Rect, pivot: Point, angle: f64) -> Vec<PathVerb> {
    let sc = angle.sin_cos();
    let radius = r.height().min(r.width()) / 2.0;
    let k = 0.552_284_75 * radius;
    let (l, t, rt, b) = (r.min_x(), r.min_y(), r.max_x(), r.max_y());
    let at = |x: f64, y: f64| turned(Point::new(x, y), pivot, sc);
    vec![
        PathVerb::MoveTo(at(l + radius, t)),
        PathVerb::LineTo(at(rt - radius, t)),
        PathVerb::CubicTo(
            at(rt - radius + k, t),
            at(rt, t + radius - k),
            at(rt, t + radius),
        ),
        PathVerb::LineTo(at(rt, b - radius)),
        PathVerb::CubicTo(
            at(rt, b - radius + k),
            at(rt - radius + k, b),
            at(rt - radius, b),
        ),
        PathVerb::LineTo(at(l + radius, b)),
        PathVerb::CubicTo(
            at(l + radius - k, b),
            at(l, b - radius + k),
            at(l, b - radius),
        ),
        PathVerb::LineTo(at(l, t + radius)),
        PathVerb::CubicTo(
            at(l, t + radius - k),
            at(l + radius - k, t),
            at(l + radius, t),
        ),
        PathVerb::Close,
    ]
}

/// One row: its icon (or the button) and the pill with its name.
struct RowNode<M> {
    index: usize,
    label: String,
    /// The open-the-folder button rather than an entry.
    button: bool,
    image: IconSet,
    /// Where the icon's middle is in the window, and how far the row is turned.
    centre: Point,
    tilt: f64,
    progress: f64,
    highlighted: bool,
    /// The window's width: the row's band is as wide.
    width: f64,
    input: fn(Input) -> M,
}

impl<M> RowNode<M> {
    fn band(&self) -> Rect {
        Rect::new(0.0, self.centre.y - PITCH / 2.0, self.width, PITCH)
    }
}

impl<M: Clone + 'static> Widget<M> for RowNode<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.band())
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.band() != old.band() {
            Change::Layout
        } else if self.label != old.label
            || self.image != old.image
            || self.centre != old.centre
            || self.tilt != old.tilt
            || self.progress != old.progress
            || self.highlighted != old.highlighted
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.band().size,
            baseline: None,
        }
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Action(request) if request.action == Action::Click => {
                cx.emit((self.input)(if self.button {
                    Input::StackOpenFolder
                } else {
                    Input::StackEntry(self.index)
                }));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let mode = usize::from(paint::is_dark(&env));
        let p = self.progress.clamp(0.0, 1.0);
        let icon_alpha = (p * 3.0).min(1.0);
        if icon_alpha < 1.0 {
            cx.set_group(icon_alpha as f32, Vec::new());
        }
        // the row's own coordinates: the node's top edge is the band's
        let c = Point::new(self.centre.x, PITCH / 2.0);
        let icon = Rect::new(c.x - ICON / 2.0, c.y - ICON / 2.0, ICON, ICON);
        let fade = |color: Color, share: f64| Color {
            a: color.a * share as f32,
            ..color
        };
        let veil = if self.highlighted {
            PILL_HIGHLIGHT[mode]
        } else {
            PILL[mode]
        };
        if self.button {
            cx.push(PaintOp::Fill {
                shape: Shape::RoundedRect {
                    rect: icon,
                    radius: ICON / 2.0,
                },
                paint: Paint::Solid(fade(PILL_HIGHLIGHT[mode], 0.8)),
            });
            let reach = ARROW_REACH;
            let at = |dx: f64, dy: f64| Point::new(c.x + dx, c.y + dy);
            cx.push(PaintOp::Stroke {
                shape: Shape::Path {
                    verbs: vec![
                        PathVerb::MoveTo(at(-reach, reach)),
                        PathVerb::LineTo(at(reach, -reach)),
                        PathVerb::MoveTo(at(-1.0, -reach)),
                        PathVerb::LineTo(at(reach, -reach)),
                        PathVerb::LineTo(at(reach, 1.0)),
                    ],
                    fill_rule: FillRule::NonZero,
                },
                width: ARROW_STROKE,
                paint: Paint::Solid(TEXT[mode]),
            });
        } else {
            paint::image(cx, &self.image, icon);
        }
        // the pill, to the left of the icon, turned with the row
        let Some(text) = paint::line(
            &env,
            &self.label,
            PILL_FONT_PT,
            400,
            Some(PILL_MAX_WIDTH - 2.0 * PILL_PADDING),
        ) else {
            return;
        };
        let (tw, th) = (text.size().width, text.size().height);
        let full = tw + 2.0 * PILL_PADDING;
        let shown = full * (PILL_START + (1.0 - PILL_START) * p);
        let right = c.x - ICON / 2.0 - PILL_GAP;
        let pill = Rect::new(right - shown, c.y - PILL_HEIGHT / 2.0, shown, PILL_HEIGHT);
        let shape = Shape::Path {
            verbs: pill_path(pill, c, self.tilt),
            fill_rule: FillRule::NonZero,
        };
        let appear = (p * 1.6).min(1.0);
        cx.push(PaintOp::Fill {
            shape: shape.clone(),
            paint: Paint::Solid(fade(veil, appear)),
        });
        cx.push(PaintOp::Stroke {
            shape,
            width: RIM_WIDTH,
            paint: Paint::Solid(fade(RIM[mode], appear)),
        });
        let text_alpha = ((p - 0.45) / 0.55).clamp(0.0, 1.0);
        if text_alpha > 0.0 {
            let origin = Point::new(right - full + PILL_PADDING, c.y - th / 2.0);
            paint::glyphs_turned(
                cx,
                &text,
                origin,
                c,
                self.tilt,
                fade(TEXT[mode], text_alpha),
            );
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.focusable();
        cx.role(Role::Button);
        cx.name(self.label.clone());
        cx.action(Action::Click);
    }
}
