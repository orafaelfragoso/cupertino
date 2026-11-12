//! The widgets the screens are made of besides the standard controls: the backdrop with its wallpaper, the fading
//! interface, absolute placement, text at a size of its own, the avatar, the field's frame and its adornment, and the
//! power item.

use cupertino_kit::core::{
    AccessibleStates, Action, Change, Element, Event, EventCx, Live, MeasureCx, PaintCx, Phase,
    Propagation, Role, SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_kit::platform::{Keysym, MouseButton, PointerEvent};
use cupertino_kit::render::{FillRule, GradientStop, Paint, PaintOp, PathVerb, Shape};
use cupertino_kit::text::{
    AttributedText, FontWeight, ParagraphLayout, ParagraphStyle, TextAlign, TextStyle, TextSystem,
    Wrap,
};
use cupertino_kit::theme::{Color, ColorRole, StateVariant};

use crate::spinner;

const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);
const SPACE: Keysym = Keysym(0x20);
pub(crate) const ESCAPE: Keysym = Keysym(0xff1b);

/// Black laid over the wallpaper (alpha).
const DARKEN: f32 = 0.25;
/// Black laid over the darkened wallpaper (alpha).
const LIGHTEN: f32 = 0.3;
/// Black in the middle of the vignette, fading to nothing at the corners.
const VIGNETTE: f32 = 0.2;
/// Corner colours of the procedural wallpaper (approximate: the screens have no way to ask for the user's picture).
const WALLPAPER_BASE: [[f32; 3]; 2] = [[0.07, 0.09, 0.20], [0.20, 0.09, 0.27]];
/// The direction of the wallpaper's diagonal gradient, as weights of x and y in the unit square.
const WALLPAPER_DIAGONAL: (f32, f32) = (0.45, 0.55);
/// Centre in the unit square, falloff `k` of `exp(-k r^2)` with `r` in heights, and colour of each blob.
const WALLPAPER_BLOBS: [([f64; 2], f32, [f32; 3]); 2] = [
    ([0.22, 0.30], 9.0, [0.10, 0.30, 0.58]),
    ([0.80, 0.68], 7.0, [0.50, 0.16, 0.42]),
];
/// Strength of a blob at its centre.
const BLOB_STRENGTH: f32 = 0.85;
/// How many stops sample a blob's falloff.
const BLOB_STOPS: u32 = 6;
/// Side of the square tiles the wallpaper is drawn in, points: a change over it (a typed character) draws again only
/// the tiles under the change instead of every gradient over the whole screen.
const WALLPAPER_TILE: f64 = 128.0;

/// Avatar halo: how far the shadow reaches past the disc and its opacity at the disc.
const AVATAR_SHADOW_RADIUS: f64 = 15.0;
const AVATAR_SHADOW_OPACITY: f32 = 0.25;
/// Avatar ring: its width and white at this alpha.
const RING_WIDTH: f64 = 1.0;
const RING_ALPHA: f32 = 0.1;
/// Fill behind the initials (approximate).
const AVATAR_FILL: [f32; 3] = [0.60, 0.62, 0.66];
/// The mark on the avatar of a user who has a session: a disc of this fill with a white check, at its lower right.
const LOGGED_IN_FILL: [f32; 3] = [0.20, 0.72, 0.35];
const LOGGED_IN_RADIUS: f64 = 9.0;
const LOGGED_IN_CHECK_WIDTH: f64 = 1.8;

/// The outline of a password field after a wrong password: the dark appearance's red (approximate), and its width.
const FAILURE_OUTLINE: [f32; 3] = [1.0, 69.0 / 255.0, 58.0 / 255.0];
pub(crate) const FAILURE_OUTLINE_WIDTH: f64 = 2.0;
/// Corner radius of the outline around the field.
const FAILURE_OUTLINE_RADIUS: f64 = 7.0;

/// Caps Lock badge: a rounded square of corner radius 4 in 16-unit space, centred in the field's trailing slot.
const CAPS_BADGE_UNITS: f64 = 16.0;
const CAPS_BADGE_RADIUS: f64 = 4.0;
/// The up arrow over a bar that is cut out of the badge, in badge units.
const CAPS_GLYPH: [&[(f64, f64)]; 2] = [
    &[
        (8.0, 3.0),
        (12.5, 8.5),
        (10.0, 8.5),
        (10.0, 11.0),
        (6.0, 11.0),
        (6.0, 8.5),
        (3.5, 8.5),
    ],
    &[(5.5, 12.5), (10.5, 12.5), (10.5, 14.0), (5.5, 14.0)],
];
/// Edge of the spinner square in the field's trailing slot.
const SPINNER_SIZE: f64 = 16.0;

/// The power glyph: the ring's radius, stroke and the angle of its gap at the top, and how many segments draw the
/// ring.
const POWER_RADIUS: f64 = 5.5;
const POWER_STROKE: f64 = 1.6;
const POWER_GAP_DEGREES: f64 = 60.0;
const POWER_RING_STEPS: usize = 30;
/// The power item's fill while it has the keyboard focus or its menu is open (white at this alpha).
const POWER_HIGHLIGHT_ALPHA: f32 = 0.2;

/// The selected picker tile's rounded square behind the avatar: its edge, corner radius and white at this alpha.
const SELECTION_SIZE: f64 = 86.0;
const SELECTION_RADIUS: f64 = 20.0;
const SELECTION_ALPHA: f32 = 0.2;

pub(crate) const WHITE: Color = Color::new(1.0, 1.0, 1.0, 1.0);

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

fn rgba(c: [f32; 3], alpha: f32) -> Color {
    Color::new(c[0], c[1], c[2], alpha)
}

fn fill(cx: &mut PaintCx<'_>, shape: Shape, color: Color) {
    cx.push(PaintOp::Fill {
        shape,
        paint: Paint::Solid(color),
    });
}

fn circle(center: Point, radius: f64) -> Shape {
    Shape::RoundedRect {
        rect: Rect::new(
            center.x - radius,
            center.y - radius,
            2.0 * radius,
            2.0 * radius,
        ),
        radius,
    }
}

/// The part under `tile` (screen points) of the procedural wallpaper with the lock screen's treatment on top, over a
/// screen of `size`: a darkening, a lightening and a vignette, each drawn over the one before. The tile's edges are
/// put on device pixels, so neighbouring tiles meet without a seam at any scale.
fn paint_wallpaper(cx: &mut PaintCx<'_>, size: Size, tile: Rect) {
    let scale = cx.env().scale;
    let snap = |v: f64| (v * scale).round() / scale;
    let (x0, y0) = (snap(tile.min_x()), snap(tile.min_y()));
    let whole = Shape::Rect(Rect::new(
        x0 - tile.min_x(),
        y0 - tile.min_y(),
        snap(tile.max_x()) - x0,
        snap(tile.max_y()) - y0,
    ));
    // screen points to the tile's own
    let at = |x: f64, y: f64| Point::new(x - tile.min_x(), y - tile.min_y());
    let (w, h) = (size.width, size.height);
    let (a, b) = WALLPAPER_DIAGONAL;
    let norm = a * a + b * b;
    cx.push(PaintOp::Fill {
        shape: whole.clone(),
        paint: Paint::Linear {
            start: at(0.0, 0.0),
            end: at(f64::from(a / norm) * w, f64::from(b / norm) * h),
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: rgba(WALLPAPER_BASE[0], 1.0),
                },
                GradientStop {
                    offset: 1.0,
                    color: rgba(WALLPAPER_BASE[1], 1.0),
                },
            ],
        },
    });
    for (center, k, color) in WALLPAPER_BLOBS {
        // sample exp(-k r^2) until it is nearly gone; r is in heights
        let reach = (50.0f32.ln() / k).sqrt();
        let stops = (0..=BLOB_STOPS)
            .map(|i| {
                let t = i as f32 / BLOB_STOPS as f32;
                GradientStop {
                    offset: t,
                    color: rgba(color, BLOB_STRENGTH * (-k * (t * reach).powi(2)).exp()),
                }
            })
            .collect();
        cx.push(PaintOp::Fill {
            shape: whole.clone(),
            paint: Paint::Radial {
                center: at(center[0] * w, center[1] * h),
                radius: f64::from(reach) * h,
                stops,
            },
        });
    }
    for alpha in [DARKEN, LIGHTEN] {
        fill(cx, whole.clone(), Color::new(0.0, 0.0, 0.0, alpha));
    }
    cx.push(PaintOp::Fill {
        shape: whole,
        paint: Paint::Radial {
            center: at(w / 2.0, h / 2.0),
            radius: w.hypot(h) / 2.0,
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: Color::new(0.0, 0.0, 0.0, VIGNETTE),
                },
                GradientStop {
                    offset: 1.0,
                    color: Color::new(0.0, 0.0, 0.0, 0.0),
                },
            ],
        },
    });
}

/// The content of a full-screen window: the wallpaper under everything, and the first look at every key and press.
/// Any of them is activity; while the interface is hidden the key or press only shows it again and goes no further.
/// Escape and the keys `keys` maps go to the application instead of the focused control.
pub(crate) struct Backdrop<M> {
    /// The screen size to paint the treated wallpaper over (the lock screen and the greeter); a panel paints nothing.
    pub wallpaper: Option<Size>,
    pub hidden: bool,
    pub on_activity: Option<M>,
    /// What a key the application takes means; `None` passes the key on to the focus.
    pub keys: Box<dyn Fn(Keysym, Option<char>) -> Option<M>>,
    /// What assistive technology calls the screen, and says of it.
    pub label: String,
    pub description: Option<String>,
    pub role: Role,
    pub modal: bool,
    pub children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for Backdrop<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        let mut children: Vec<Element<M>> = self
            .wallpaper
            .map(|size| {
                tiles(size)
                    .map(|rect| Element::new(WallpaperTile { size, rect }))
                    .collect()
            })
            .unwrap_or_default();
        children.append(&mut self.children);
        children
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() != Phase::Capture {
            return Propagation::Continue;
        }
        let activity = |cx: &mut EventCx<'_, M>| {
            if let Some(message) = &self.on_activity {
                cx.emit(message.clone());
            }
        };
        match event {
            Event::Key(key, _) if key.pressed => {
                activity(cx);
                if self.hidden {
                    return Propagation::Stop;
                }
                match (self.keys)(key.keysym, key.text) {
                    Some(message) => {
                        cx.emit(message);
                        Propagation::Stop
                    }
                    None => Propagation::Continue,
                }
            }
            Event::Pointer(PointerEvent::Button { pressed: true, .. }, _) => {
                activity(cx);
                if self.hidden {
                    Propagation::Stop
                } else {
                    Propagation::Continue
                }
            }
            Event::Pointer(PointerEvent::Motion { .. }, _) => {
                activity(cx);
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(self.role);
        cx.name(self.label.clone());
        if let Some(description) = &self.description {
            cx.description(description.clone());
        }
        if self.modal {
            cx.states(AccessibleStates {
                modal: true,
                ..AccessibleStates::default()
            });
        }
    }
}

/// The squares of [`WALLPAPER_TILE`] points that cover a screen of `size`, row by row; those at the right and bottom
/// edges are cut to the screen.
fn tiles(size: Size) -> impl Iterator<Item = Rect> {
    let count = |side: f64| (side.max(0.0) / WALLPAPER_TILE).ceil() as usize;
    let (columns, rows) = (count(size.width), count(size.height));
    (0..rows).flat_map(move |row| {
        (0..columns).map(move |column| {
            let (x, y) = (column as f64 * WALLPAPER_TILE, row as f64 * WALLPAPER_TILE);
            Rect::new(
                x,
                y,
                WALLPAPER_TILE.min(size.width - x),
                WALLPAPER_TILE.min(size.height - y),
            )
        })
    })
}

/// One tile of the treated wallpaper of a screen of `size`, at `rect`.
struct WallpaperTile {
    size: Size,
    rect: Rect,
}

impl<M: 'static> Widget<M> for WallpaperTile {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.size != old.size {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        paint_wallpaper(cx, self.size, self.rect);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}

/// Everything of a screen but its wallpaper, drawn as one layer while it is partly faded; fully shown it is drawn
/// straight over the wallpaper, which looks the same, so a change in it repaints only what it covers.
pub(crate) struct Fade<M> {
    pub opacity: f64,
    pub children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Fade<M> {
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

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.opacity != old.opacity {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let opacity = self.opacity.clamp(0.0, 1.0);
        if opacity < 1.0 {
            cx.set_group(opacity as f32, Vec::new());
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// Its children at `rect` in an absolute parent, stacked along `axis` and placed across it by `align`. `initial` gives
/// its first control the focus when the window opens.
pub(crate) struct Place<M> {
    pub rect: Rect,
    pub axis: Axis,
    pub align: Align,
    pub justify: Justify,
    pub initial: bool,
    pub children: Vec<Element<M>>,
}

impl<M: 'static> Place<M> {
    /// Centred both ways.
    pub(crate) fn new(rect: Rect, children: Vec<Element<M>>) -> Self {
        Place {
            rect,
            axis: Axis::Vertical,
            align: Align::Center,
            justify: Justify::Center,
            initial: false,
            children,
        }
    }

    /// From the top, against the leading edge, as a paragraph.
    pub(crate) fn text(rect: Rect, children: Vec<Element<M>>) -> Self {
        Place {
            align: Align::Start,
            justify: Justify::Start,
            ..Place::new(rect, children)
        }
    }

    /// Filling the box: a button at its frame.
    pub(crate) fn filled(rect: Rect, children: Vec<Element<M>>) -> Self {
        Place {
            align: Align::Stretch,
            ..Place::new(rect, children)
        }
    }
}

impl<M: 'static> Widget<M> for Place<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: self.axis,
                spacing: 0.0,
                align: self.align,
                justify: self.justify,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect
            || self.axis != old.axis
            || self.align != old.align
            || self.justify != old.justify
        {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.initial {
            cx.initial_focus();
        }
    }
}

/// Text at a size and weight of its own, in `color` or the theme's text colour, centred in its box. Its first
/// baseline sits `ascent` below the box's top, so a caller places it by its baseline.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Label {
    pub text: String,
    pub size: f64,
    pub weight: FontWeight,
    pub color: Option<Color>,
    /// The box, in the parent's points.
    pub rect: Rect,
    /// From the box's top to the first baseline.
    pub ascent: f64,
    /// Wraps at the box's width; otherwise one line that ends in an ellipsis.
    pub wrap: bool,
    /// Read out as soon as it shows or changes.
    pub live: Option<Live>,
}

impl Label {
    fn laid_out(&self, cx: &PaintCx<'_>) -> Option<ParagraphLayout> {
        let content = AttributedText::plain(
            self.text.clone(),
            TextStyle {
                weight: self.weight,
                ..TextStyle::new(self.size as f32)
            },
        );
        let style = ParagraphStyle {
            align: TextAlign::Center,
            wrap: if self.wrap { Wrap::Word } else { Wrap::None },
            ellipsis: !self.wrap,
            ..ParagraphStyle::default()
        };
        let paragraph = cx.env().text.paragraph(&content, &style).ok()?;
        Some(paragraph.layout(Some(self.rect.width() as f32)))
    }
}

impl<M: 'static> Widget<M> for Label {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: Some(self.ascent),
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let Some(layout) = self.laid_out(cx) else {
            return;
        };
        let color = self.color.or_else(|| {
            cx.env()
                .theme
                .colors
                .get(ColorRole::Text, StateVariant::NORMAL)
        });
        let Some(color) = color else {
            return;
        };
        let first = layout.lines().first().map_or(0.0, |line| line.baseline);
        let origin = Point::new(0.0, self.ascent - first);
        for line in layout.lines() {
            for run in &line.runs {
                cx.push(PaintOp::Glyphs {
                    origin: origin + run.origin,
                    run: run.run.clone(),
                    color,
                });
            }
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.text.clone());
        if let Some(live) = self.live {
            cx.live(live);
        }
    }
}

/// The halo, disc and ring of an avatar at `rect`, with its picture or initials as children, and the mark of a user
/// with a session.
pub(crate) struct Avatar<M> {
    pub rect: Rect,
    /// No picture: the disc is filled behind the initials.
    pub filled: bool,
    pub logged_in: bool,
    pub children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Avatar<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            // the initials' line box sits at its own place in the disc
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.filled != old.filled || self.logged_in != old.logged_in {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let radius = self.rect.width() / 2.0;
        let center = Point::new(radius, radius);
        let outer = radius + AVATAR_SHADOW_RADIUS;
        let shadow = |alpha| Color::new(0.0, 0.0, 0.0, alpha);
        cx.push(PaintOp::Fill {
            shape: circle(center, outer),
            paint: Paint::Radial {
                center,
                radius: outer,
                stops: vec![
                    GradientStop {
                        offset: 0.0,
                        color: shadow(AVATAR_SHADOW_OPACITY),
                    },
                    GradientStop {
                        offset: (radius / outer) as f32,
                        color: shadow(AVATAR_SHADOW_OPACITY),
                    },
                    GradientStop {
                        offset: 1.0,
                        color: shadow(0.0),
                    },
                ],
            },
        });
        if self.filled {
            fill(cx, circle(center, radius), rgba(AVATAR_FILL, 1.0));
        }
        cx.push(PaintOp::Stroke {
            shape: circle(center, radius - RING_WIDTH / 2.0),
            width: RING_WIDTH,
            paint: Paint::Solid(Color::new(1.0, 1.0, 1.0, RING_ALPHA)),
        });
        if self.logged_in {
            let mark = Point::new(
                2.0 * radius - LOGGED_IN_RADIUS,
                2.0 * radius - LOGGED_IN_RADIUS,
            );
            fill(
                cx,
                circle(mark, LOGGED_IN_RADIUS),
                rgba(LOGGED_IN_FILL, 1.0),
            );
            let at = |dx: f64, dy: f64| {
                Point::new(
                    mark.x + LOGGED_IN_RADIUS * dx,
                    mark.y + LOGGED_IN_RADIUS * dy,
                )
            };
            cx.push(PaintOp::Stroke {
                shape: Shape::Path {
                    verbs: vec![
                        PathVerb::MoveTo(at(-0.45, 0.02)),
                        PathVerb::LineTo(at(-0.1, 0.38)),
                        PathVerb::LineTo(at(0.5, -0.35)),
                    ],
                    fill_rule: FillRule::NonZero,
                },
                width: LOGGED_IN_CHECK_WIDTH,
                paint: Paint::Solid(WHITE),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The box of a field at `rect`, its field centred in it: the outline after a wrong password, and a focus scope keyed
/// by the field's generation, so a new field takes the focus on every screen that shows it.
pub(crate) struct FieldFrame<M> {
    pub rect: Rect,
    pub outline: bool,
    pub children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for FieldFrame<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Center,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.outline != old.outline {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if !self.outline {
            return;
        }
        let half = FAILURE_OUTLINE_WIDTH / 2.0;
        let size = cx.size();
        cx.push(PaintOp::Stroke {
            shape: Shape::RoundedRect {
                rect: Rect::new(
                    -half,
                    -half,
                    size.width + 2.0 * half,
                    size.height + 2.0 * half,
                ),
                radius: FAILURE_OUTLINE_RADIUS,
            },
            width: FAILURE_OUTLINE_WIDTH,
            paint: Paint::Solid(rgba(FAILURE_OUTLINE, 1.0)),
        });
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.focus_scope();
        cx.initial_focus();
    }
}

/// What the field's trailing slot shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Adornment {
    /// The spinner at this frame: an attempt is being checked.
    Spinner(u32),
    CapsLock,
}

/// The field's trailing slot at `rect`, drawn over the field in the theme's secondary text colour.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Slot {
    pub rect: Rect,
    pub adornment: Adornment,
    /// What assistive technology hears of it.
    pub label: &'static str,
}

impl Slot {
    fn caps(&self, cx: &mut PaintCx<'_>, color: Color) {
        let size = cx.size();
        let side = CAPS_BADGE_UNITS;
        let (left, top) = ((size.width - side) / 2.0, (size.height - side) / 2.0);
        let r = CAPS_BADGE_RADIUS;
        let k = 0.552_284_75 * r;
        let (x0, y0, x1, y1) = (left, top, left + side, top + side);
        let mut verbs = vec![
            PathVerb::MoveTo(Point::new(x0 + r, y0)),
            PathVerb::LineTo(Point::new(x1 - r, y0)),
            PathVerb::CubicTo(
                Point::new(x1 - r + k, y0),
                Point::new(x1, y0 + r - k),
                Point::new(x1, y0 + r),
            ),
            PathVerb::LineTo(Point::new(x1, y1 - r)),
            PathVerb::CubicTo(
                Point::new(x1, y1 - r + k),
                Point::new(x1 - r + k, y1),
                Point::new(x1 - r, y1),
            ),
            PathVerb::LineTo(Point::new(x0 + r, y1)),
            PathVerb::CubicTo(
                Point::new(x0 + r - k, y1),
                Point::new(x0, y1 - r + k),
                Point::new(x0, y1 - r),
            ),
            PathVerb::LineTo(Point::new(x0, y0 + r)),
            PathVerb::CubicTo(
                Point::new(x0, y0 + r - k),
                Point::new(x0 + r - k, y0),
                Point::new(x0 + r, y0),
            ),
            PathVerb::Close,
        ];
        for polygon in CAPS_GLYPH {
            for (i, &(x, y)) in polygon.iter().enumerate() {
                let point = Point::new(left + x, top + y);
                verbs.push(if i == 0 {
                    PathVerb::MoveTo(point)
                } else {
                    PathVerb::LineTo(point)
                });
            }
            verbs.push(PathVerb::Close);
        }
        // even-odd: the glyph polygons lie inside the body, so they are holes
        fill(
            cx,
            Shape::Path {
                verbs,
                fill_rule: FillRule::EvenOdd,
            },
            color,
        );
    }

    fn spinner(&self, cx: &mut PaintCx<'_>, color: Color, frame: u32) {
        let size = cx.size();
        let center = Point::new(size.width / 2.0, size.height / 2.0);
        let unit = SPINNER_SIZE / spinner::CANVAS;
        let half_width = spinner::SPOKE_WIDTH / 2.0;
        for spoke in 0..spinner::SPOKES {
            // clockwise from 12 o'clock
            let angle = spoke as f64 * std::f64::consts::TAU / spinner::SPOKES as f64;
            let (sin, cos) = angle.sin_cos();
            let at = |radius: f64| {
                Point::new(
                    center.x + sin * radius * unit,
                    center.y - cos * radius * unit,
                )
            };
            cx.push(PaintOp::Stroke {
                shape: Shape::Path {
                    verbs: vec![
                        PathVerb::MoveTo(at(spinner::INNER_RADIUS + half_width)),
                        PathVerb::LineTo(at(spinner::OUTER_RADIUS - half_width)),
                    ],
                    fill_rule: FillRule::NonZero,
                },
                width: spinner::SPOKE_WIDTH * unit,
                paint: Paint::Solid(Color::new(
                    color.r,
                    color.g,
                    color.b,
                    color.a * spinner::opacity(frame, spoke),
                )),
            });
        }
    }
}

impl<M: 'static> Widget<M> for Slot {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let Some(color) = cx
            .env()
            .theme
            .colors
            .get(ColorRole::SecondaryText, StateVariant::NORMAL)
        else {
            return;
        };
        match self.adornment {
            Adornment::CapsLock => self.caps(cx, color),
            Adornment::Spinner(frame) => self.spinner(cx, color, frame),
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        match self.adornment {
            Adornment::CapsLock => cx.role(Role::StaticText),
            Adornment::Spinner(_) => {
                cx.role(Role::ProgressIndicator);
                cx.states(AccessibleStates {
                    busy: true,
                    ..AccessibleStates::default()
                });
            }
        }
        cx.name(self.label);
    }
}

/// A control drawn by the screen itself at `rect`: the power item (a glyph) or a picker tile (a selection square and
/// its children). A click, Return, Space or assistive technology's click send `on_press`; gaining the focus sends
/// `on_focus`; the pointer over it sends `on_hover`.
pub(crate) struct Pressable<M> {
    pub rect: Rect,
    pub look: Look,
    pub label: String,
    pub description: Option<String>,
    pub on_press: M,
    pub on_focus: Option<M>,
    pub on_hover: Option<M>,
    pub children: Vec<Element<M>>,
}

/// How a [`Pressable`] draws itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Look {
    /// The power glyph, highlighted while the menu is open.
    Power { open: bool },
    /// A picker tile: the rounded square behind the avatar while selected, centred on `avatar_center`.
    Tile {
        selected: bool,
        avatar_center: Point,
    },
}

#[derive(Default)]
pub(crate) struct PressState {
    focused: bool,
}

impl<M: Clone + 'static> Widget<M> for Pressable<M> {
    type State = PressState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> PressState {
        PressState::default()
    }

    fn update(&self, old: &Self, _: &mut PressState) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.look != old.look || self.label != old.label {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &PressState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: None,
        }
    }

    fn event(&self, state: &mut PressState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match event {
            Event::Focus { gained, .. } => {
                state.focused = *gained;
                cx.request_paint();
                if let (true, Some(message)) = (*gained, &self.on_focus) {
                    cx.emit(message.clone());
                }
                Propagation::Continue
            }
            Event::Pointer(PointerEvent::Enter { .. } | PointerEvent::Motion { .. }, _) => {
                if let Some(message) = &self.on_hover {
                    cx.emit(message.clone());
                }
                Propagation::Continue
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            Event::Key(key, _)
                if key.pressed
                    && cx.node() == cx.target()
                    && matches!(key.keysym, RETURN | KP_ENTER | SPACE) =>
            {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            Event::Action(request) if request.action == Action::Click => {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, state: &PressState, cx: &mut PaintCx<'_>) {
        let size = cx.size();
        match self.look {
            Look::Power { open } => {
                if open || (state.focused && cx.focus_visible()) {
                    fill(
                        cx,
                        Shape::RoundedRect {
                            rect: Rect::new(0.0, 0.0, size.width, size.height),
                            radius: size.height / 2.0,
                        },
                        Color::new(1.0, 1.0, 1.0, POWER_HIGHLIGHT_ALPHA),
                    );
                }
                power_glyph(cx, Point::new(size.width / 2.0, size.height / 2.0));
            }
            Look::Tile {
                selected,
                avatar_center,
            } => {
                if selected || (state.focused && cx.focus_visible()) {
                    let half = SELECTION_SIZE / 2.0;
                    fill(
                        cx,
                        Shape::RoundedRect {
                            rect: Rect::new(
                                avatar_center.x - half,
                                avatar_center.y - half,
                                SELECTION_SIZE,
                                SELECTION_SIZE,
                            ),
                            radius: SELECTION_RADIUS,
                        },
                        Color::new(1.0, 1.0, 1.0, SELECTION_ALPHA),
                    );
                }
            }
        }
    }

    fn semantics(&self, _: &PressState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.label.clone());
        if let Some(description) = &self.description {
            cx.description(description.clone());
        }
        if let Look::Tile { selected, .. } = self.look {
            cx.states(AccessibleStates {
                selected,
                ..AccessibleStates::default()
            });
        }
        if let Look::Power { open } = self.look {
            cx.states(AccessibleStates {
                expanded: Some(open),
                ..AccessibleStates::default()
            });
        }
        cx.focusable();
        cx.action(Action::Click);
    }
}

/// The power glyph centred on `center`: a ring open at the top with a bar through the gap, white, round caps.
fn power_glyph(cx: &mut PaintCx<'_>, center: Point) {
    let mut verbs = Vec::new();
    for step in 0..=POWER_RING_STEPS {
        let angle = (POWER_GAP_DEGREES / 2.0
            + step as f64 * (360.0 - POWER_GAP_DEGREES) / POWER_RING_STEPS as f64)
            .to_radians();
        let point = Point::new(
            center.x + POWER_RADIUS * angle.sin(),
            center.y - POWER_RADIUS * angle.cos(),
        );
        verbs.push(if step == 0 {
            PathVerb::MoveTo(point)
        } else {
            PathVerb::LineTo(point)
        });
    }
    verbs.push(PathVerb::MoveTo(Point::new(
        center.x,
        center.y - POWER_RADIUS - 1.0,
    )));
    verbs.push(PathVerb::LineTo(Point::new(center.x, center.y - 1.0)));
    cx.push(PaintOp::Stroke {
        shape: Shape::Path {
            verbs,
            fill_rule: FillRule::NonZero,
        },
        width: POWER_STROKE,
        paint: Paint::Solid(WHITE),
    });
}

/// A card's fill: white at this alpha over the treated wallpaper (approximate), and the alpha of its secondary text.
const NOTICE_FILL_ALPHA: f32 = 0.18;
const NOTICE_SECONDARY_ALPHA: f32 = 0.75;

/// A notification on the lock screen, in the column of cards: its application, its title and its body, left-aligned,
/// each cut with an ellipsis where it does not fit. It is as tall as its text, and the gap below it is its own. It is
/// one node for assistive technology, named by `speech` and said politely as it arrives; it takes no input, so the
/// password field keeps the focus.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NoticeCard {
    pub app: String,
    pub title: String,
    pub body: String,
    pub speech: String,
    pub ascent: f64,
    pub descent: f64,
}

/// A line of a card's text: its size, weight and how many lines it may take.
struct Line {
    size: f64,
    weight: FontWeight,
    lines: u32,
}

impl NoticeCard {
    fn laid_out(&self, text: &TextSystem, words: &str, line: &Line) -> Option<ParagraphLayout> {
        use crate::lock_notices::{CARD_PADDING, CARD_WIDTH};
        let content = AttributedText::plain(
            words.to_owned(),
            TextStyle {
                weight: line.weight,
                ..TextStyle::new(line.size as f32)
            },
        );
        let style = ParagraphStyle {
            align: TextAlign::Start,
            wrap: if line.lines > 1 {
                Wrap::Word
            } else {
                Wrap::None
            },
            max_lines: Some(line.lines),
            ellipsis: true,
            ..ParagraphStyle::default()
        };
        let paragraph = text.paragraph(&content, &style).ok()?;
        Some(paragraph.layout(Some((CARD_WIDTH - 2.0 * CARD_PADDING) as f32)))
    }

    const BODY: Line = Line {
        size: crate::lock_notices::BODY_SIZE,
        weight: FontWeight::REGULAR,
        lines: crate::lock_notices::BODY_LINES,
    };

    /// How many lines the body takes: none without one.
    fn body_lines(&self, text: &TextSystem) -> u32 {
        if self.body.trim().is_empty() {
            return 0;
        }
        self.laid_out(text, &self.body, &Self::BODY)
            .map_or(Self::BODY.lines, |l| l.lines().len() as u32)
            .clamp(1, Self::BODY.lines)
    }

    /// Paint `words` with its first line's top at `top`; returns the top of what follows.
    fn paint_text(
        &self,
        cx: &mut PaintCx<'_>,
        words: &str,
        line: &Line,
        top: f64,
        color: Color,
    ) -> f64 {
        use crate::lock_notices::CARD_PADDING;
        let Some(layout) = self.laid_out(&cx.env().text, words, line) else {
            return top;
        };
        let first = layout.lines().first().map_or(0.0, |l| l.baseline);
        let origin = Point::new(CARD_PADDING, top + self.ascent * line.size - first);
        let shown = layout.lines().len().min(line.lines as usize).max(1);
        for l in layout.lines().iter().take(shown) {
            for run in &l.runs {
                cx.push(PaintOp::Glyphs {
                    origin: origin + run.origin,
                    run: run.run.clone(),
                    color,
                });
            }
        }
        top + shown as f64 * (self.ascent + self.descent) * line.size
    }
}

impl<M: 'static> Widget<M> for NoticeCard {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(crate::lock_notices::CARD_WIDTH),
            height: SizeSpec::Fit,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self != old {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        use crate::lock_notices::{card_height, CARD_GAP, CARD_WIDTH};
        let lines = self.body_lines(&cx.env().text);
        Measured {
            size: Size::new(
                CARD_WIDTH,
                card_height(lines, self.ascent, self.descent) + CARD_GAP,
            ),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        use crate::lock_notices::{
            APP_SIZE, CARD_GAP, CARD_PADDING, CARD_RADIUS, LINE_GAP, TITLE_SIZE,
        };
        let size = cx.size();
        fill(
            cx,
            Shape::RoundedRect {
                rect: Rect::new(0.0, 0.0, size.width, (size.height - CARD_GAP).max(0.0)),
                radius: CARD_RADIUS,
            },
            Color::new(1.0, 1.0, 1.0, NOTICE_FILL_ALPHA),
        );
        let secondary = Color::new(1.0, 1.0, 1.0, NOTICE_SECONDARY_ALPHA);
        let app = Line {
            size: APP_SIZE,
            weight: FontWeight::REGULAR,
            lines: 1,
        };
        let title = Line {
            size: TITLE_SIZE,
            weight: FontWeight::SEMIBOLD,
            lines: 1,
        };
        let top = self.paint_text(cx, &self.app, &app, CARD_PADDING, secondary);
        let top = self.paint_text(cx, &self.title, &title, top + LINE_GAP, WHITE);
        if !self.body.trim().is_empty() {
            self.paint_text(cx, &self.body, &Self::BODY, top + LINE_GAP, secondary);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.speech.clone());
        cx.live(Live::Polite);
    }
}
