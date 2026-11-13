//! The panel's windows as CupertinoKit draws them: the module tiles, each its own glass shape on a transparent window,
//! the open detail view grown out of its tile, and the transparent catcher under the panel that takes a press outside
//! it.
//!
//! The window paints no panel of its own: every tile asks the compositor for glass under its outline, and the tiles
//! join one glass container so the compositor can blend them where they come close. With Reduce Transparency the
//! tiles are opaque and sit on one opaque panel. The panel keeps the dark appearance in a light session; there the
//! tiles lay a dark tint over their glass so the white text reads on it.

use std::cell::RefCell;
use std::rc::Rc;

use cupertino_kit::controls::{Icon, IconDirection};
use cupertino_kit::core::{
    AccessibleStates, AccessibleValue, Action, ActionData, Change, Checked, CommandRegistry,
    Element, Environment, Event, EventCx, Glass, Key, MeasureCx, PaintCx, Phase, Propagation, Role,
    SemanticsCx, Shortcut, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutDirection, LayoutSpec, Measured,
    Overflow, SizeSpec,
};
use cupertino_kit::platform::{
    DecorationMode, Keysym, Mods, MouseButton, PointerEvent, WindowSpec,
};
use cupertino_kit::render::{FillRule, Paint, PaintOp, PathVerb, Shape};
use cupertino_kit::text::{
    AttributedText, FontWeight, ParagraphLayout, ParagraphStyle, TextAlign, TextStyle, Wrap,
};
use cupertino_kit::theme::{Appearance, Color, ColorRole, StateVariant};

use crate::layout::{
    self, detail as rows, Outline, BAR_MARGIN, GRID_ORIGIN, GRID_WIDTH, INSET, LARGE_RADIUS,
    SHADOW_REACH, SHADOW_TOP, SURFACE, WINDOW, WINDOW_ORIGIN,
};
use crate::model::{Controls, Detail, MediaCommand, Module, Reading, Slider, TileState};
use crate::motion::{visibility, Visibility};

/// The `app_id` of the panel's window, which gives it its layer surface.
pub const APP_ID: &str = "cupertino-controlcenter";
/// The `app_id` of the catcher's window.
pub const CATCHER_APP_ID: &str = "cupertino-controlcenter.catcher";
/// What assistive technology calls the panel.
pub const TITLE: &str = "Control Center";

const ESCAPE: Keysym = Keysym(0xff1b);
const SPACE: Keysym = Keysym(0x20);
const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);
const LEFT: Keysym = Keysym(0xff51);
const UP: Keysym = Keysym(0xff52);
const RIGHT: Keysym = Keysym(0xff53);
const DOWN: Keysym = Keysym(0xff54);
const HOME: Keysym = Keysym(0xff50);
const END: Keysym = Keysym(0xff57);

/// The glass container the tiles join, and how close two of them come before their outlines blend: less than the gap
/// between tiles, so tiles at rest stay apart.
const CONTAINER: u32 = 1;
const BLEND: f64 = 8.0;

/// The fills a tile paints itself. `RT_*`: the opaque panel and tiles with Reduce Transparency. `FALLBACK`: a tile the window cannot show glass under.
const RT_GROUP: Color = Color::new(0.302, 0.302, 0.302, 1.0);
const RT_TILE: Color = Color::new(0.114, 0.114, 0.114, 1.0);
/// The tiles with Increase Contrast: opaque mid grey inside a darker rim half a point wide, so no backdrop shows through
/// behind the white text.
const IC_TILE: Color = Color::new(0.463, 0.463, 0.463, 1.0);
const IC_RIM_COLOR: Color = Color::new(0.341, 0.341, 0.341, 1.0);
const IC_RIM: f64 = 0.5;
const FALLBACK: Color = Color::new(0.16, 0.16, 0.17, 0.92);
/// A switch that is on: a white disc behind its symbol, drawn in the accent colour.
const ON_FILL: Color = Color::new(1.0, 1.0, 1.0, 1.0);
/// A switch badge that is off: a light veil.
const OFF_BADGE: Color = Color::new(1.0, 1.0, 1.0, 0.16);
/// Laid over a control while it is pressed: darker on a white disc, lighter on glass.
const PRESSED_ON: Color = Color::new(0.0, 0.0, 0.0, 0.12);
const PRESSED_OFF: Color = Color::new(1.0, 1.0, 1.0, 0.12);
/// Under a list row the pointer is over.
const HOVER_ROW: Color = Color::new(1.0, 1.0, 1.0, 0.1);
/// The track of a level: the level in white over a faint white track.
const TRACK_FILL: Color = Color::new(1.0, 1.0, 1.0, 1.0);
const TRACK_EMPTY: Color = Color::new(1.0, 1.0, 1.0, 0.2);
const TRACK_HEIGHT: f64 = 4.0;
/// A disabled control's content is drawn at this opacity.
const DISABLED_ALPHA: f32 = 0.4;
/// The media tile's artwork placeholder and the separator of a detail view.
const ARTWORK: Color = Color::new(1.0, 1.0, 1.0, 0.12);
const ARTWORK_RADIUS: f64 = 10.0;
const SEPARATOR: Color = Color::new(1.0, 1.0, 1.0, 0.16);

/// Type of the panel. Sizes come from the theme's type scale: titles in its body size (13) semibold, second lines in
/// its caption size (11), the Edit Controls label in the body size medium, a detail's heading in the caption size
/// semibold, its rows in the body size.
const TITLE_WEIGHT: u16 = 600;
const LINE_WEIGHT: u16 = 500;
const EDIT_WEIGHT: u16 = 500;
const ROW_WEIGHT: u16 = 400;

/// How strongly the labels of a tile are drawn over its glass, as the opacity of the label colour: a title somewhat
/// below full white, its second line in full white at a medium weight, so the state reads as the brighter line under
/// the name. The slider
/// tiles' titles and the Edit Controls label are drawn in full white. Increase Contrast draws every label in the
/// theme's own colours.
const TITLE_STRENGTH: f32 = 0.85;
const LINE_STRENGTH: f32 = 1.0;

/// A capsule's badge: its diameter, its inset from the capsule's leading and top edges, and where the text starts.
const BADGE: f64 = 36.0;
const BADGE_INSET: f64 = 14.0;
const TEXT_START: f64 = 56.0;
const TEXT_END_INSET: f64 = 6.0;
/// Symbol sizes: in a badge, on a circular tile, in a list row.
const BADGE_SYMBOL: f64 = 18.0;
const CIRCLE_SYMBOL: f64 = 22.0;
const ROW_SYMBOL: f64 = 16.0;
/// The slider tiles: the title's inset, the track's centre line and the hit band around it.
const SLIDER_TITLE_INSET: f64 = 16.0;
const SLIDER_TITLE_TOP: f64 = 9.0;
const TRACK_CENTRE: f64 = 42.0;
const SLIDER_BAND: f64 = 20.0;
/// How far past each end of its track a level takes the pointer.
const LEVEL_REACH: f64 = 4.0;
/// The display tile's track and its two suns; the sound tile's track, speakers and output button.
const DISPLAY_TRACK: (f64, f64) = (36.0, 254.0);
const SOUND_TRACK: (f64, f64) = (33.0, 213.0);
const OUTPUT_BUTTON: f64 = 26.0;
const OUTPUT_CENTRE_X: f64 = 265.0;
/// A level step of the arrow keys.
const KEY_STEP: f64 = 1.0 / 16.0;
/// The media tile: artwork square, text block and the three buttons on a centre line.
const ART: f64 = 40.0;
const MEDIA_TEXT_TOP: f64 = 66.0;
const MEDIA_BUTTONS_Y: f64 = 111.0;
const MEDIA_BUTTONS_X: [f64; 3] = [30.0, 70.0, 110.0];
const MEDIA_BUTTON: f64 = 32.0;
/// Corner radius of a detail view's row highlight.
const ROW_RADIUS: f64 = 8.0;

/// What the user did on the panel.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// A module's switch or button was pressed.
    Press(Module),
    /// A module's detail view is asked for.
    Expand(Detail, Via),
    /// The open detail view is to close: Escape or a press outside it.
    Collapse(Via),
    /// A button of the media tile.
    Media(MediaCommand),
    /// A network of the Wi-Fi view, by name.
    Join(String),
    /// A device of the Bluetooth view, by identifier: it connects or disconnects.
    Connect(String),
    /// A sound output, by identifier.
    Output(u32),
    /// The settings row of a detail view.
    Settings(Detail),
    /// The Edit Controls capsule.
    EditControls,
    /// An arrow key on a control: the focus goes to the control of this key.
    Focus(String),
    /// A slider moved to a fraction, 0 to 1.
    Slide(Slider, f64),
    /// The slider's drag or key step ended at a fraction.
    Release(Slider, f64),
    /// Escape, or a press outside the panel.
    Dismiss,
    /// The server closed the window: its output went away.
    Closed,
}

/// How a control was taken. The focus follows a detail view in and out only when the keyboard or assistive
/// technology opened or closed it, so a click leaves no focus ring behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Pointer,
    Keyboard,
}

impl Input {
    /// What the input is when the keyboard or assistive technology takes the control instead of the pointer.
    fn by_key(self) -> Input {
        match self {
            Input::Expand(detail, _) => Input::Expand(detail, Via::Keyboard),
            Input::Collapse(_) => Input::Collapse(Via::Keyboard),
            input => input,
        }
    }
}

/// How the panel is shown besides its readings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Presentation {
    /// The panel's opacity while it fades out.
    pub alpha: f64,
    /// The session is in the light appearance: the panel keeps its dark glass.
    pub light: bool,
    /// The panel takes input: false while it fades out.
    pub input: bool,
    /// A detail view opening, open or closing.
    pub detail: Option<Opened>,
}

impl Presentation {
    /// The panel at rest in a dark session.
    pub const SHOWN: Presentation = Presentation {
        alpha: 1.0,
        light: false,
        input: true,
        detail: None,
    };
}

/// A detail view and how far it has opened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Opened {
    pub detail: Detail,
    /// 0 (the tile in the grid) to 1 (the open view).
    pub progress: f64,
    /// The tile grows into the view; with Reduce Motion the two cross-fade in place instead.
    pub morph: bool,
}

/// The key of the control that holds the focus for `module` on the grid.
pub fn focus_key(module: Module) -> String {
    format!("module.{}", slug(module))
}

/// The key of the first control of an open detail view.
pub const DETAIL_FOCUS: &str = "detail.header";

fn slug(module: Module) -> &'static str {
    match module {
        Module::WiFi => "wifi",
        Module::NowPlaying => "now-playing",
        Module::Bluetooth => "bluetooth",
        Module::Microphone => "microphone",
        Module::DockHiding => "dock-hiding",
        Module::LockScreen => "lock-screen",
        Module::Mute => "mute",
        Module::KeyboardBrightness => "keyboard-brightness",
        Module::FocusMode => "focus",
        Module::Display => "display",
        Module::Sound => "sound",
    }
}

/// The symbol of a module.
fn symbol(controls: &Controls, module: Module) -> &'static str {
    let on = controls.tile(module) == TileState::On;
    match module {
        Module::WiFi => "symbol.wifi",
        Module::Bluetooth => "symbol.bluetooth",
        Module::Microphone if on => "symbol.mic",
        Module::Microphone => "symbol.mic.slash",
        Module::DockHiding => "symbol.dock.hide",
        Module::LockScreen => "symbol.lock-screen",
        Module::Mute => "symbol.speaker.slash",
        Module::KeyboardBrightness if on => "symbol.keyboard-brightness",
        Module::KeyboardBrightness => "symbol.keyboard-brightness.off",
        Module::FocusMode => "symbol.moon.fill",
        Module::Display => "symbol.sun",
        Module::Sound => "symbol.speaker.high",
        Module::NowPlaying => "symbol.music",
    }
}

/// Which size of the theme's type scale a label takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TypeSize {
    Body,
    Caption,
}

/// How a label is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    /// A tile's title: the label colour at [`TITLE_STRENGTH`].
    Title,
    /// A tile's second line: the label colour at [`LINE_STRENGTH`].
    Line,
    /// The label colour at its full strength.
    Primary,
    /// The secondary label colour of the theme.
    Secondary,
}

/// One label of the panel, set in its own size and weight and drawn in its tone. The kit's static text sets every
/// label in the size of its role; the panel's second lines and headings are smaller than its titles.
#[derive(Clone, Debug, PartialEq)]
struct Label {
    content: String,
    size: TypeSize,
    weight: u16,
    tone: Tone,
    /// Wraps onto more lines where its block is too narrow; else one line that ends in an ellipsis.
    wrap: bool,
}

impl Label {
    fn laid_out(&self, env: &Environment, width: f64) -> Option<ParagraphLayout> {
        let scale = &env.theme.type_scale;
        let size = match self.size {
            TypeSize::Body => scale.body.size,
            TypeSize::Caption => scale.caption.size,
        };
        let content = AttributedText::plain(
            &self.content,
            TextStyle {
                weight: FontWeight(self.weight),
                ..TextStyle::new(size)
            },
        );
        let style = ParagraphStyle {
            align: match env.direction {
                LayoutDirection::Ltr => TextAlign::Left,
                LayoutDirection::Rtl => TextAlign::Right,
            },
            wrap: if self.wrap { Wrap::Word } else { Wrap::None },
            ellipsis: !self.wrap,
            ..ParagraphStyle::default()
        };
        let paragraph = env.text.paragraph(&content, &style).ok()?;
        let width = width.is_finite().then_some(width as f32);
        if self.wrap {
            return Some(paragraph.layout(width));
        }
        let line = paragraph.layout(None);
        Some(match width {
            Some(width) if line.size().width > f64::from(width) => {
                let cut = paragraph.layout(Some(width));
                paragraph.layout(Some(cut.size().width as f32))
            }
            _ => line,
        })
    }

    /// The colour of the label where it is painted.
    fn color(&self, cx: &PaintCx<'_>) -> Color {
        let colors = &cx.env().theme.colors;
        let white = |a: f32| Color::new(1.0, 1.0, 1.0, a);
        let label = colors
            .get(ColorRole::Text, StateVariant::NORMAL)
            .unwrap_or(white(1.0));
        let secondary = colors
            .get(ColorRole::SecondaryText, StateVariant::NORMAL)
            .unwrap_or(white(0.55));
        let at = |strength: f32| Color {
            a: label.a.min(strength),
            ..label
        };
        match (self.tone, increase_contrast(cx)) {
            (Tone::Title, false) => at(TITLE_STRENGTH),
            (Tone::Line, false) => at(LINE_STRENGTH),
            (Tone::Title | Tone::Primary, _) => label,
            (Tone::Line | Tone::Secondary, _) => secondary,
        }
    }
}

impl<M: 'static> Widget<M> for Label {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.content != old.content
            || self.size != old.size
            || self.weight != old.weight
            || self.wrap != old.wrap
        {
            Change::Layout
        } else if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), constraints: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        match self.laid_out(cx.env(), constraints.width.max()) {
            Some(layout) => Measured {
                size: layout.size(),
                baseline: layout.lines().first().map(|line| line.baseline),
            },
            None => Measured {
                size: Size::ZERO,
                baseline: None,
            },
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let width = cx.size().width;
        let Some(layout) = self.laid_out(cx.env(), width) else {
            return;
        };
        let origin = match cx.env().direction {
            LayoutDirection::Ltr => Point::ZERO,
            LayoutDirection::Rtl => Point::new(
                width - layout.lines().first().map_or(0.0, |line| line.rect.width()),
                0.0,
            ),
        };
        let color = self.color(cx);
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
        cx.name(self.content.clone());
    }
}

/// A label on one line.
fn text<M: 'static>(content: &str, size: TypeSize, weight: u16, tone: Tone) -> Element<M> {
    Element::new(Label {
        content: content.to_owned(),
        size,
        weight,
        tone,
        wrap: false,
    })
}

/// A label that wraps onto more lines where its block is too narrow, as a capsule's title and state do.
fn wrapping<M: 'static>(content: &str, size: TypeSize, weight: u16, tone: Tone) -> Element<M> {
    Element::new(Label {
        content: content.to_owned(),
        size,
        weight,
        tone,
        wrap: true,
    })
}

fn icon<M: 'static>(name: &str, size: f64, tint: Option<ColorRole>) -> Element<M> {
    Element::new(Icon {
        name: name.to_owned(),
        size,
        direction: IconDirection::Fixed,
        label: None,
        tint,
    })
}

fn fill(cx: &mut PaintCx<'_>, rect: Rect, radius: f64, color: Color) {
    cx.push(PaintOp::Fill {
        shape: Shape::RoundedRect { rect, radius },
        paint: Paint::Solid(color),
    });
}

/// `rect` with its corners rounded by `radius`, as a path of four quarter circles.
fn rounded_path(rect: Rect, radius: f64) -> Vec<PathVerb> {
    // a quarter circle's control points sit this far along its tangents, as a fraction of the radius
    const KAPPA: f64 = 0.552_284_75;
    let (l, t, r, b) = (rect.min_x(), rect.min_y(), rect.max_x(), rect.max_y());
    let radius = radius
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0)
        .max(0.0);
    let k = radius * KAPPA;
    vec![
        PathVerb::MoveTo(Point::new(l + radius, t)),
        PathVerb::LineTo(Point::new(r - radius, t)),
        PathVerb::CubicTo(
            Point::new(r - radius + k, t),
            Point::new(r, t + radius - k),
            Point::new(r, t + radius),
        ),
        PathVerb::LineTo(Point::new(r, b - radius)),
        PathVerb::CubicTo(
            Point::new(r, b - radius + k),
            Point::new(r - radius + k, b),
            Point::new(r - radius, b),
        ),
        PathVerb::LineTo(Point::new(l + radius, b)),
        PathVerb::CubicTo(
            Point::new(l + radius - k, b),
            Point::new(l, b - radius + k),
            Point::new(l, b - radius),
        ),
        PathVerb::LineTo(Point::new(l, t + radius)),
        PathVerb::CubicTo(
            Point::new(l, t + radius - k),
            Point::new(l + radius - k, t),
            Point::new(l + radius, t),
        ),
        PathVerb::Close,
    ]
}

/// Darkening at a distance from the modules' edge, in points: the share of black laid over the backdrop. It starts
/// at about 9 % at the edge and fades out over [`SHADOW_REACH`].
const SHADOW_PROFILE: [(f64, f64); 5] = [
    (0.0, 0.090),
    (30.0, 0.067),
    (60.0, 0.027),
    (90.0, 0.011),
    (SHADOW_REACH, 0.0),
];
/// The shadow is drawn as bands this wide, each a ring around the modules' outline.
const SHADOW_BAND: f64 = 5.0;

/// The darkening `distance` points from the edge.
fn shadow_at(distance: f64) -> f64 {
    let d = distance.max(0.0);
    for pair in SHADOW_PROFILE.windows(2) {
        let ((d0, a0), (d1, a1)) = (pair[0], pair[1]);
        if d <= d1 {
            return a0 + (a1 - a0) * ((d - d0) / (d1 - d0)).clamp(0.0, 1.0);
        }
    }
    0.0
}

/// A soft shadow around `rect` (rounded at [`LARGE_RADIUS`]): concentric bands outside the outline, so the modules
/// themselves are not darkened.
///
/// The `holes` are the tiles' outlines (a rectangle and its corner radius): the shadow is also laid between them, where
/// the glass of the tiles does not cover it, at the strength it has at the edge, and is clipped away over the tiles,
/// whose glass lies under everything the window paints.
fn shadow(cx: &mut PaintCx<'_>, rect: Rect, holes: &[(Rect, f64)]) {
    if !holes.is_empty() {
        let mut path = rounded_path(rect.outset(SHADOW_REACH + 1.0), 0.0);
        for (hole, radius) in holes {
            path.extend(rounded_path(*hole, *radius));
        }
        cx.push(PaintOp::PushClip(Shape::Path {
            verbs: path,
            fill_rule: FillRule::EvenOdd,
        }));
        fill(
            cx,
            rect,
            LARGE_RADIUS,
            Color::new(0.0, 0.0, 0.0, shadow_at(0.0) as f32),
        );
    }
    let bands = (SHADOW_REACH / SHADOW_BAND).ceil() as usize;
    for i in (0..bands).rev() {
        let inner = i as f64 * SHADOW_BAND;
        let middle = inner + SHADOW_BAND / 2.0;
        let alpha = shadow_at(middle) as f32;
        if alpha <= 0.0 {
            continue;
        }
        let centre = rect.outset(middle);
        cx.push(PaintOp::Stroke {
            shape: Shape::RoundedRect {
                rect: centre,
                radius: LARGE_RADIUS + middle,
            },
            width: SHADOW_BAND,
            paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, alpha)),
        });
    }
    if !holes.is_empty() {
        cx.push(PaintOp::PopClip);
    }
}

fn reduce_transparency(cx: &PaintCx<'_>) -> bool {
    cx.env().theme.preferences.reduce_transparency
}

fn increase_contrast(cx: &PaintCx<'_>) -> bool {
    matches!(
        cx.env().theme.preferences.appearance,
        Appearance::LightHighContrast | Appearance::DarkHighContrast
    )
}

fn finite_alpha(alpha: f64) -> f64 {
    if alpha.is_finite() {
        alpha.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// A box at a place in its parent that lays its children out as `kind` says, clipping them.
struct Place<M> {
    frame: Rect,
    kind: ContainerKind,
    /// What the box is to assistive technology, when it is something: a list with its name and its number of rows.
    list: Option<(String, usize)>,
    children: Vec<Element<M>>,
}

impl<M: 'static> Place<M> {
    /// `children` stacked on `axis` in `frame`, centred across it; `centre` also centres them along it.
    fn stack(frame: Rect, axis: Axis, centre: bool, children: Vec<Element<M>>) -> Element<M> {
        let justify = if centre || axis == Axis::Vertical {
            Justify::Center
        } else {
            Justify::Start
        };
        let align = if axis == Axis::Vertical && !centre {
            Align::Start
        } else {
            Align::Center
        };
        Element::new(Place {
            frame,
            kind: ContainerKind::Stack {
                axis,
                spacing: 0.0,
                align,
                justify,
            },
            list: None,
            children,
        })
    }

    /// `child` centred in `frame`.
    fn centred(frame: Rect, child: Element<M>) -> Element<M> {
        Self::stack(frame, Axis::Horizontal, true, vec![child])
    }

    /// `children` at their own positions in `frame`.
    fn absolute(frame: Rect, children: Vec<Element<M>>) -> Element<M> {
        Element::new(Place {
            frame,
            kind: ContainerKind::Absolute,
            list: None,
            children,
        })
    }

    /// The rows of a list named `name`, at their own positions in `frame`.
    fn list(frame: Rect, name: &str, children: Vec<Element<M>>) -> Element<M> {
        Element::new(Place {
            frame,
            kind: ContainerKind::Absolute,
            list: Some((name.to_owned(), children.len())),
            children,
        })
    }
}

fn spec(kind: ContainerKind, frame: Rect) -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fixed(frame.width()),
        height: SizeSpec::Fixed(frame.height()),
        overflow: Overflow::Clip,
        position: frame.origin,
        ..LayoutSpec::new(kind)
    }
}

impl<M: 'static> Widget<M> for Place<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(self.kind.clone(), self.frame)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.frame != old.frame || self.kind != old.kind {
            Change::Layout
        } else if self.list != old.list {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.frame.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if let Some((name, count)) = &self.list {
            cx.role(Role::List);
            cx.name(name.clone());
            cx.item_count(*count);
        }
    }
}

/// The whole window: the tiles at their places. With Reduce Transparency it draws the one opaque panel under the
/// tiles. A left or right press on it, between or around the tiles, collapses the open detail view or dismisses the
/// panel.
struct Board<M> {
    alpha: f64,
    /// Height of the tiles the opaque panel of Reduce Transparency goes around: the grid and Edit Controls, or an open
    /// detail view.
    height: f64,
    /// The outline of what the soft shadow goes around, relative to the grid's corner.
    shadow: Rect,
    /// The tiles' outlines, relative to the grid's corner, that the shadow is cut away over.
    holes: Vec<(Rect, f64)>,
    on_outside: M,
    children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for Board<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            overflow: Overflow::Visible,
            ..spec(
                ContainerKind::Absolute,
                Rect::from_origin_size(Point::ZERO, SURFACE),
            )
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.alpha != old.alpha
            || self.height != old.height
            || self.shadow != old.shadow
            || self.holes != old.holes
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: SURFACE,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left | MouseButton::Right,
                    pressed: true,
                    ..
                },
                _,
            ) if cx.phase() != Phase::Capture => {
                cx.emit(self.on_outside.clone());
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let alpha = finite_alpha(self.alpha);
        if alpha < 1.0 {
            cx.set_group(alpha as f32, Vec::new());
        }
        if reduce_transparency(cx) {
            let group = Rect::new(
                WINDOW_ORIGIN.x,
                WINDOW_ORIGIN.y,
                WINDOW.width,
                self.height + 2.0 * INSET,
            );
            fill(cx, group, layout::GROUP_RADIUS, RT_GROUP);
        } else if !increase_contrast(cx) {
            let holes: Vec<(Rect, f64)> = self
                .holes
                .iter()
                .map(|(rect, radius)| (rect.translate(GRID_ORIGIN.x, GRID_ORIGIN.y), *radius))
                .collect();
            // the menu bar lies over the panel's shadow: none of it darkens the bar
            let bar_edge = SHADOW_TOP - f64::from(BAR_MARGIN);
            cx.push(PaintOp::PushClip(Shape::Rect(Rect::new(
                -1.0e4, bar_edge, 2.0e4, 2.0e4,
            ))));
            shadow(
                cx,
                self.shadow.translate(GRID_ORIGIN.x, GRID_ORIGIN.y),
                &holes,
            );
            cx.push(PaintOp::PopClip);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
        cx.name(TITLE);
    }
}

/// One tile: glass under its outline, its content over it. A press on the tile outside its controls does nothing.
struct GlassTile<M> {
    label: String,
    frame: Rect,
    radius: f64,
    capsule: bool,
    /// The panel's opacity: it reaches the glass but not the content, which the board fades.
    panel: f64,
    /// The tile's own opacity, glass and content.
    alpha: f64,
    light: bool,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for GlassTile<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Absolute, self.frame)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.frame != old.frame {
            Change::Layout
        } else if self.radius != old.radius
            || self.capsule != old.capsule
            || self.panel != old.panel
            || self.alpha != old.alpha
            || self.light != old.light
            || self.label != old.label
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.frame.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Pointer(PointerEvent::Button { pressed: true, .. }, _)
                if cx.phase() != Phase::Capture =>
            {
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let alpha = finite_alpha(self.alpha);
        if alpha < 1.0 {
            cx.set_group(alpha as f32, Vec::new());
        }
        let size = cx.size();
        let rect = Rect::from_origin_size(Point::ZERO, size);
        let radius = if self.capsule {
            size.width.min(size.height) / 2.0
        } else {
            self.radius
        };
        if reduce_transparency(cx) {
            fill(cx, rect, radius, RT_TILE);
            return;
        }
        if increase_contrast(cx) {
            fill(cx, rect, radius, IC_TILE);
            let rim = Rect::new(
                IC_RIM / 2.0,
                IC_RIM / 2.0,
                size.width - IC_RIM,
                size.height - IC_RIM,
            );
            cx.push(PaintOp::Stroke {
                shape: Shape::RoundedRect {
                    rect: rim,
                    radius: (radius - IC_RIM / 2.0).max(0.0),
                },
                width: IC_RIM,
                paint: Paint::Solid(IC_RIM_COLOR),
            });
            return;
        }
        let shape = if self.capsule {
            Glass::module().capsule()
        } else {
            Glass::module().radius(radius)
        };
        let glass = shape
            .container(CONTAINER, BLEND)
            .alpha(finite_alpha(self.panel) * alpha);
        if !cx.glass(glass) {
            fill(cx, rect, radius, FALLBACK);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
        cx.name(self.label.clone());
    }
}

/// How a pressable control looks.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Face {
    /// A disc: white when on, a light veil when off.
    Badge { on: bool },
    /// A whole circular tile: a white disc when on, the glass alone when off.
    Circle { on: bool },
    /// Nothing at rest; a highlight under the pointer when `hover` is set, a veil while pressed.
    Plain { radius: f64, hover: bool },
}

impl Face {
    fn on(self) -> bool {
        matches!(self, Face::Badge { on: true } | Face::Circle { on: true })
    }
}

/// What a pressable control is to assistive technology.
#[derive(Clone, Debug, PartialEq)]
enum Kind {
    /// A switch with its state.
    Switch(bool),
    Button,
    /// A button that opens a view; `Some(open)` once it can tell.
    Expander(bool),
    /// The row at `index` of a list, `selected` when it is the current one.
    Row {
        index: usize,
        selected: bool,
    },
}

/// Where the arrow keys lead from a control: the controls of the panel with their rects in the window.
type Map = Rc<RefCell<Vec<(String, Rect)>>>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Press {
    hovered: bool,
    pointer: bool,
    key: bool,
    inside: bool,
}

impl Press {
    fn pressed(&self) -> bool {
        self.key || (self.pointer && self.inside)
    }
}

/// A control taken by a press, Space or Return, or assistive technology; the arrow keys move the focus to the
/// nearest control that way.
struct Pressable<M> {
    key: String,
    label: String,
    description: Option<String>,
    kind: Kind,
    face: Face,
    frame: Rect,
    enabled: bool,
    on_press: Option<Input>,
    map: Map,
    to: fn(Input) -> M,
    children: Vec<Element<M>>,
}

impl<M: 'static> Pressable<M> {
    fn radius(&self, size: Size) -> f64 {
        match self.face {
            Face::Badge { .. } | Face::Circle { .. } => size.width.min(size.height) / 2.0,
            Face::Plain { radius, .. } => radius,
        }
    }

    fn arrow(&self, keysym: Keysym, cx: &mut EventCx<'_, M>) -> bool {
        let Some(from) = cx.bounds() else {
            return false;
        };
        let direction = match keysym {
            LEFT => (-1.0, 0.0),
            RIGHT => (1.0, 0.0),
            UP => (0.0, -1.0),
            DOWN => (0.0, 1.0),
            _ => return false,
        };
        let map = self.map.borrow();
        if let Some(key) = nearest(&map, &self.key, from, direction) {
            cx.emit((self.to)(Input::Focus(key)));
        }
        true
    }
}

/// The control of `map` nearest to `from` in `direction`, other than `own`: among those whose centre lies that way,
/// the one with the least distance along the direction plus twice the distance across it.
fn nearest(map: &[(String, Rect)], own: &str, from: Rect, direction: (f64, f64)) -> Option<String> {
    let start = from.center();
    map.iter()
        .filter(|(key, _)| key != own)
        .filter_map(|(key, rect)| {
            let c = rect.center();
            let (dx, dy) = (c.x - start.x, c.y - start.y);
            let along = dx * direction.0 + dy * direction.1;
            let across = (dx * direction.1 - dy * direction.0).abs();
            (along > 0.5).then_some((along + 2.0 * across, key))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, key)| key.clone())
}

impl<M: 'static> Widget<M> for Pressable<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        spec(
            ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
                align: Align::Center,
                justify: Justify::Center,
            },
            self.frame,
        )
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.frame != old.frame {
            Change::Layout
        } else if self.face != old.face
            || self.kind != old.kind
            || self.enabled != old.enabled
            || self.label != old.label
            || self.description != old.description
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.frame.size,
            baseline: None,
        }
    }

    fn event(&self, state: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        let enabled = self.enabled && self.on_press.is_some();
        let activate = |cx: &mut EventCx<'_, M>, via: Via| {
            if let Some(input) = self.on_press.clone() {
                let input = match via {
                    Via::Pointer => input,
                    Via::Keyboard => input.by_key(),
                };
                cx.emit((self.to)(input));
            }
        };
        match *event {
            Event::Pointer(PointerEvent::Enter { .. }, _) if cx.node() == cx.target() => {
                state.hovered = true;
                state.inside |= state.pointer;
                cx.request_paint();
                Propagation::Continue
            }
            Event::Pointer(PointerEvent::Leave, _) if cx.node() == cx.target() => {
                state.hovered = false;
                state.inside = false;
                cx.request_paint();
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
                if enabled && !state.pointer && !state.key {
                    state.pointer = true;
                    state.inside = true;
                    cx.capture_pointer();
                    cx.request_paint();
                }
                Propagation::Stop
            }
            Event::Pointer(PointerEvent::Motion { position }, _) if state.pointer => {
                let inside = cx.hit(position);
                if inside != state.inside {
                    state.inside = inside;
                    cx.request_paint();
                }
                Propagation::Stop
            }
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) if state.pointer => {
                cx.release_pointer();
                let inside = cx.hit(position);
                *state = Press {
                    hovered: state.hovered,
                    ..Press::default()
                };
                cx.request_paint();
                if enabled && inside {
                    activate(cx, Via::Pointer);
                }
                Propagation::Stop
            }
            Event::Cancel => {
                *state = Press {
                    hovered: state.hovered,
                    ..Press::default()
                };
                cx.request_paint();
                Propagation::Continue
            }
            Event::Focus { gained: false, .. } if state.key => {
                state.key = false;
                cx.request_paint();
                Propagation::Continue
            }
            Event::Key(ref key, _) if cx.node() == cx.target() => match key.keysym {
                SPACE if key.pressed && !key.repeat && enabled && !state.pointer => {
                    state.key = true;
                    cx.request_paint();
                    Propagation::Stop
                }
                SPACE if !key.pressed && state.key => {
                    state.key = false;
                    cx.request_paint();
                    activate(cx, Via::Keyboard);
                    Propagation::Stop
                }
                RETURN | KP_ENTER if key.pressed && !key.repeat && enabled => {
                    activate(cx, Via::Keyboard);
                    Propagation::Stop
                }
                LEFT | RIGHT | UP | DOWN if key.pressed && self.arrow(key.keysym, cx) => {
                    Propagation::Stop
                }
                _ => Propagation::Continue,
            },
            Event::Action(ref request) => match request.action {
                Action::Click | Action::Expand if enabled => {
                    activate(cx, Via::Keyboard);
                    Propagation::Stop
                }
                Action::Focus => {
                    cx.request_focus();
                    Propagation::Stop
                }
                _ => Propagation::Continue,
            },
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, state: &Press, cx: &mut PaintCx<'_>) {
        if !self.enabled {
            cx.set_group(DISABLED_ALPHA, Vec::new());
        }
        let size = cx.size();
        let rect = Rect::from_origin_size(Point::ZERO, size);
        let radius = self.radius(size);
        match self.face {
            Face::Badge { on: true } | Face::Circle { on: true } => fill(cx, rect, radius, ON_FILL),
            Face::Badge { on: false } => fill(cx, rect, radius, OFF_BADGE),
            Face::Plain { hover: true, .. } if state.hovered && self.enabled => {
                fill(cx, rect, radius, HOVER_ROW)
            }
            _ => {}
        }
        if state.pressed() {
            let veil = if self.face.on() {
                PRESSED_ON
            } else {
                PRESSED_OFF
            };
            fill(cx, rect, radius, veil);
        }
        if cx.focus_visible() && cx.window_active() {
            let theme = cx.env().theme.clone();
            let ring = &theme.focus;
            let color = theme
                .colors
                .get(ColorRole::KeyboardFocusIndicator, StateVariant::NORMAL);
            if let Some(color) = color {
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: rect.outset(ring.outset + ring.width / 2.0),
                        radius: radius + ring.outset + ring.width / 2.0,
                    },
                    width: ring.width,
                    paint: Paint::Solid(color),
                });
            }
        }
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        let mut states = AccessibleStates {
            disabled: !self.enabled,
            ..AccessibleStates::default()
        };
        match self.kind {
            Kind::Switch(on) => {
                cx.role(Role::Switch);
                cx.value(AccessibleValue::Toggle(if on {
                    Checked::True
                } else {
                    Checked::False
                }));
            }
            Kind::Button => cx.role(Role::Button),
            Kind::Expander(open) => {
                cx.role(Role::Button);
                states.expanded = Some(open);
            }
            Kind::Row { index, selected } => {
                cx.role(Role::ListItem);
                cx.item_index(index);
                states.selected = selected;
            }
        }
        cx.name(self.label.clone());
        if let Some(description) = &self.description {
            cx.description(description.clone());
        }
        cx.states(states);
        if self.enabled && self.on_press.is_some() {
            cx.focusable();
            cx.action(Action::Click);
            if matches!(self.kind, Kind::Expander(false)) {
                cx.action(Action::Expand);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Drag {
    dragging: bool,
}

/// A level: a thin white track over a faint one, with no knob. A press or drag puts the level under the pointer;
/// Left and Right, Home and End step it.
struct Level<M> {
    key: String,
    label: String,
    slider: Slider,
    frame: Rect,
    value: Option<f64>,
    map: Map,
    to: fn(Input) -> M,
}

impl<M: 'static> Level<M> {
    fn at(&self, x: f64, bounds: Rect) -> f64 {
        let width = (bounds.width() - 2.0 * LEVEL_REACH).max(1.0);
        ((x - bounds.x() - LEVEL_REACH) / width).clamp(0.0, 1.0)
    }

    fn step(&self, value: f64, cx: &mut EventCx<'_, M>) {
        let value = value.clamp(0.0, 1.0);
        cx.emit((self.to)(Input::Slide(self.slider, value)));
        cx.emit((self.to)(Input::Release(self.slider, value)));
    }
}

impl<M: 'static> Widget<M> for Level<M> {
    type State = Drag;

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Leaf, self.frame)
    }

    fn state(&self) -> Drag {
        Drag::default()
    }

    fn update(&self, old: &Self, _: &mut Drag) -> Change {
        if self.frame != old.frame {
            Change::Layout
        } else if self.value != old.value || self.label != old.label {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Drag, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.frame.size,
            baseline: None,
        }
    }

    fn event(&self, state: &mut Drag, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        let Some(value) = self.value else {
            return match event {
                Event::Pointer(PointerEvent::Button { .. }, _) => Propagation::Stop,
                _ => Propagation::Continue,
            };
        };
        match *event {
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed,
                    ..
                },
                _,
            ) => {
                if let Some(bounds) = cx.bounds() {
                    if pressed && !state.dragging {
                        state.dragging = true;
                        cx.capture_pointer();
                        cx.emit((self.to)(Input::Slide(
                            self.slider,
                            self.at(position.x, bounds),
                        )));
                    } else if !pressed && state.dragging {
                        state.dragging = false;
                        cx.release_pointer();
                        cx.emit((self.to)(Input::Release(self.slider, value)));
                    }
                }
                Propagation::Stop
            }
            Event::Pointer(PointerEvent::Motion { position }, _) if state.dragging => {
                if let Some(bounds) = cx.bounds() {
                    cx.emit((self.to)(Input::Slide(
                        self.slider,
                        self.at(position.x, bounds),
                    )));
                }
                Propagation::Stop
            }
            Event::Cancel if state.dragging => {
                state.dragging = false;
                cx.emit((self.to)(Input::Release(self.slider, value)));
                Propagation::Continue
            }
            Event::Key(ref key, _) if key.pressed && cx.node() == cx.target() => {
                let next = match key.keysym {
                    RIGHT => value + KEY_STEP,
                    LEFT => value - KEY_STEP,
                    HOME => 0.0,
                    END => 1.0,
                    UP | DOWN => {
                        let Some(from) = cx.bounds() else {
                            return Propagation::Continue;
                        };
                        let direction = if key.keysym == UP { -1.0 } else { 1.0 };
                        let found = nearest(&self.map.borrow(), &self.key, from, (0.0, direction));
                        if let Some(key) = found {
                            cx.emit((self.to)(Input::Focus(key)));
                        }
                        return Propagation::Stop;
                    }
                    _ => return Propagation::Continue,
                };
                self.step(next, cx);
                Propagation::Stop
            }
            Event::Action(ref request) => {
                let next = match (request.action, &request.data) {
                    (Action::Increment, _) => value + KEY_STEP,
                    (Action::Decrement, _) => value - KEY_STEP,
                    (Action::SetValue, Some(ActionData::Number(v))) => *v,
                    (Action::Focus, _) => {
                        cx.request_focus();
                        return Propagation::Stop;
                    }
                    _ => return Propagation::Continue,
                };
                self.step(next, cx);
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &Drag, cx: &mut PaintCx<'_>) {
        if self.value.is_none() {
            cx.set_group(DISABLED_ALPHA, Vec::new());
        }
        let size = cx.size();
        let track = Rect::new(
            LEVEL_REACH,
            (size.height - TRACK_HEIGHT) / 2.0,
            size.width - 2.0 * LEVEL_REACH,
            TRACK_HEIGHT,
        );
        let radius = TRACK_HEIGHT / 2.0;
        fill(cx, track, radius, TRACK_EMPTY);
        let level = self.value.unwrap_or(0.0).clamp(0.0, 1.0);
        if level > 0.0 {
            let filled = Rect::new(
                track.x(),
                track.y(),
                (track.width() * level).max(TRACK_HEIGHT),
                track.height(),
            );
            fill(cx, filled, radius, TRACK_FILL);
        }
        if cx.focus_visible() && cx.window_active() {
            let theme = cx.env().theme.clone();
            if let Some(color) = theme
                .colors
                .get(ColorRole::KeyboardFocusIndicator, StateVariant::NORMAL)
            {
                let ring = &theme.focus;
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: track.outset(ring.outset + ring.width / 2.0 + 2.0),
                        radius: radius + ring.outset + 2.0,
                    },
                    width: ring.width,
                    paint: Paint::Solid(color),
                });
            }
        }
    }

    fn semantics(&self, _: &Drag, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Slider);
        cx.name(self.label.clone());
        cx.value(AccessibleValue::Number {
            value: self.value.unwrap_or(0.0),
            min: 0.0,
            max: 1.0,
            step: Some(KEY_STEP),
        });
        cx.states(AccessibleStates {
            disabled: self.value.is_none(),
            ..AccessibleStates::default()
        });
        if self.value.is_some() {
            cx.focusable();
            cx.action(Action::Increment);
            cx.action(Action::Decrement);
            cx.action(Action::SetValue);
        }
    }
}

/// Builds the panel's elements and records where each control lies, for the arrow keys.
struct Builder<'a, M> {
    controls: &'a Controls,
    presentation: Presentation,
    to: fn(Input) -> M,
    map: Map,
}

impl<M: Clone + 'static> Builder<'_, M> {
    /// Records a control of `key` at `frame` within a tile at `origin` in the window.
    fn record(&self, key: &str, origin: Point, frame: Rect) {
        self.map
            .borrow_mut()
            .push((key.to_owned(), frame.translate(origin.x, origin.y)));
    }

    #[allow(clippy::too_many_arguments)]
    fn pressable(
        &self,
        origin: Point,
        key: &str,
        label: &str,
        kind: Kind,
        face: Face,
        frame: Rect,
        enabled: bool,
        on_press: Option<Input>,
        children: Vec<Element<M>>,
    ) -> Element<M> {
        let interactive = enabled && on_press.is_some();
        if interactive {
            self.record(key, origin, frame);
        }
        Element::new(Pressable {
            key: key.to_owned(),
            label: label.to_owned(),
            description: None,
            kind,
            face,
            frame,
            enabled,
            on_press,
            map: Rc::clone(&self.map),
            to: self.to,
            children,
        })
        .key(key.to_owned())
    }

    fn level(&self, origin: Point, key: &str, slider: Slider, frame: Rect) -> Element<M> {
        let value = self.controls.fraction(slider);
        if value.is_some() {
            self.record(key, origin, frame);
        }
        Element::new(Level {
            key: key.to_owned(),
            label: slider.module().title().to_owned(),
            slider,
            frame,
            value,
            map: Rc::clone(&self.map),
            to: self.to,
        })
        .key(key.to_owned())
    }

    /// A glass tile at `frame` in the grid.
    fn tile(
        &self,
        label: &str,
        frame: Rect,
        outline: Outline,
        alpha: f64,
        children: Vec<Element<M>>,
    ) -> Element<M> {
        let frame = frame.translate(GRID_ORIGIN.x, GRID_ORIGIN.y);
        Element::new(GlassTile {
            label: label.to_owned(),
            frame,
            radius: outline.radius(frame.size),
            capsule: outline == Outline::Capsule,
            panel: self.presentation.alpha,
            alpha,
            light: self.presentation.light,
            children,
        })
    }

    /// A title over an optional second line, in a block from `x` to the tile's end, centred down the tile.
    fn lines(
        &self,
        title: &str,
        line: Option<&str>,
        x: f64,
        width: f64,
        height: f64,
    ) -> Element<M> {
        let mut children = vec![wrapping(title, TypeSize::Body, TITLE_WEIGHT, Tone::Title)];
        if let Some(line) = line {
            children.push(wrapping(line, TypeSize::Caption, LINE_WEIGHT, Tone::Line));
        }
        Place::stack(
            Rect::new(x, 0.0, (width - x - TEXT_END_INSET).max(0.0), height),
            Axis::Vertical,
            false,
            children,
        )
    }

    /// A capsule's content: its badge switch and its title and state. A capsule with a detail view opens it from its
    /// text; the others switch from either.
    fn capsule(&self, module: Module, origin: Point, key: &str, header: bool) -> Vec<Element<M>> {
        let controls = self.controls;
        let state = controls.tile(module);
        let on = state == TileState::On;
        let enabled = !matches!(state, TileState::Disabled(_));
        let size = layout::frame(module).size;
        let tint = on.then_some(ColorRole::DefaultButtonFill);
        let badge = self.pressable(
            origin,
            key,
            module.title(),
            Kind::Switch(on),
            Face::Badge { on },
            Rect::new(BADGE_INSET, BADGE_INSET, BADGE, BADGE),
            enabled,
            Some(Input::Press(module)),
            vec![icon(symbol(controls, module), BADGE_SYMBOL, tint)],
        );
        match module.detail().filter(|_| !header) {
            Some(detail) => {
                let label = format!("{} {}", module.title(), detail.heading());
                let more = self.pressable(
                    origin,
                    &format!("{key}.more"),
                    &label,
                    Kind::Expander(false),
                    Face::Plain {
                        radius: 0.0,
                        hover: false,
                    },
                    Rect::new(
                        TEXT_START - 4.0,
                        0.0,
                        size.width - TEXT_START + 4.0,
                        size.height,
                    ),
                    true,
                    Some(Input::Expand(detail, Via::Pointer)),
                    vec![Place::absolute(
                        Rect::new(0.0, 0.0, size.width - TEXT_START + 4.0, size.height),
                        vec![self.lines(
                            controls.title(module),
                            controls.subtitle(module),
                            4.0,
                            size.width - TEXT_START + 4.0,
                            size.height,
                        )],
                    )],
                );
                vec![badge, more]
            }
            None => {
                let width = if header {
                    layout::GRID_WIDTH
                } else {
                    size.width
                };
                let lines = self.lines(
                    controls.title(module),
                    controls.subtitle(module),
                    TEXT_START,
                    width,
                    size.height,
                );
                vec![badge, lines]
            }
        }
    }

    /// A circular tile: the whole tile is the switch or button.
    fn circle(&self, module: Module, origin: Point) -> Vec<Element<M>> {
        let controls = self.controls;
        let state = controls.tile(module);
        let on = state == TileState::On;
        let enabled = !matches!(state, TileState::Disabled(_));
        let kind = match module {
            Module::LockScreen => Kind::Button,
            _ => Kind::Switch(on),
        };
        let tint = on.then_some(ColorRole::DefaultButtonFill);
        let size = layout::frame(module).size;
        vec![self.pressable(
            origin,
            &focus_key(module),
            module.title(),
            kind,
            Face::Circle { on },
            Rect::from_origin_size(Point::ZERO, size),
            enabled,
            Some(Input::Press(module)),
            vec![icon(symbol(controls, module), CIRCLE_SYMBOL, tint)],
        )]
    }

    /// The media tile: artwork, what plays, and the previous, play / pause and next buttons.
    fn media(&self, origin: Point) -> Vec<Element<M>> {
        let size = layout::frame(Module::NowPlaying).size;
        let media = match &self.controls.media {
            Reading::Ready(media) => media.as_ref(),
            Reading::Unavailable(_) => None,
        };
        let (title, artist) = match media {
            Some(media) => (media.title.as_str(), media.artist.as_deref()),
            None => ("Not Playing", None),
        };
        let artwork = Element::new(Artwork {
            frame: Rect::new(BADGE_INSET, BADGE_INSET, ART, ART),
        });
        let block = Place::stack(
            Rect::new(
                BADGE_INSET + 4.0,
                MEDIA_TEXT_TOP,
                size.width - 2.0 * BADGE_INSET - 4.0,
                MEDIA_BUTTONS_Y - MEDIA_BUTTON / 2.0 - MEDIA_TEXT_TOP,
            ),
            Axis::Vertical,
            false,
            [
                Some(text(title, TypeSize::Body, TITLE_WEIGHT, Tone::Title)),
                artist.map(|a| text(a, TypeSize::Caption, LINE_WEIGHT, Tone::Line)),
            ]
            .into_iter()
            .flatten()
            .collect(),
        );
        let playing = media.is_some_and(|m| m.playing);
        let buttons = [
            (
                MediaCommand::Previous,
                "Previous",
                "symbol.backward",
                media.is_some_and(|m| m.can_previous),
            ),
            (
                MediaCommand::PlayPause,
                if playing { "Pause" } else { "Play" },
                if playing {
                    "symbol.pause"
                } else {
                    "symbol.play"
                },
                media.is_some_and(|m| m.can_play_pause),
            ),
            (
                MediaCommand::Next,
                "Next",
                "symbol.forward",
                media.is_some_and(|m| m.can_next),
            ),
        ];
        let mut children = vec![artwork, block];
        for ((command, label, name, enabled), x) in buttons.into_iter().zip(MEDIA_BUTTONS_X) {
            let key = match command {
                MediaCommand::Previous => format!("{}.previous", focus_key(Module::NowPlaying)),
                MediaCommand::PlayPause => focus_key(Module::NowPlaying),
                MediaCommand::Next => format!("{}.next", focus_key(Module::NowPlaying)),
            };
            children.push(self.pressable(
                origin,
                &key,
                label,
                Kind::Button,
                Face::Plain {
                    radius: MEDIA_BUTTON / 2.0,
                    hover: false,
                },
                Rect::new(
                    x - MEDIA_BUTTON / 2.0,
                    MEDIA_BUTTONS_Y - MEDIA_BUTTON / 2.0,
                    MEDIA_BUTTON,
                    MEDIA_BUTTON,
                ),
                enabled,
                Some(Input::Media(command)),
                vec![icon(
                    name,
                    if command == MediaCommand::PlayPause {
                        22.0
                    } else {
                        18.0
                    },
                    None,
                )],
            ));
        }
        children
    }

    /// A slider tile's content: its title (which opens the detail view on the grid) and the level between its
    /// symbols, with the output button on the sound tile.
    fn slider(&self, slider: Slider, origin: Point, header: bool) -> Vec<Element<M>> {
        let module = slider.module();
        let detail = module.detail().expect("both sliders have a detail view");
        let title = text(module.title(), TypeSize::Body, TITLE_WEIGHT, Tone::Primary);
        let title_frame = Rect::new(
            SLIDER_TITLE_INSET - 8.0,
            SLIDER_TITLE_TOP - 4.0,
            120.0,
            24.0,
        );
        let title = if header {
            Place::stack(
                title_frame,
                Axis::Horizontal,
                false,
                vec![Element::new(Spacer8), title],
            )
        } else {
            self.pressable(
                origin,
                &format!("{}.more", focus_key(module)),
                &format!("{} {}", module.title(), detail.heading()),
                Kind::Expander(false),
                Face::Plain {
                    radius: 8.0,
                    hover: false,
                },
                title_frame,
                true,
                Some(Input::Expand(detail, Via::Pointer)),
                vec![Place::stack(
                    Rect::from_origin_size(Point::ZERO, title_frame.size),
                    Axis::Horizontal,
                    false,
                    vec![Element::new(Spacer8), title],
                )],
            )
        };
        let (start, end) = match slider {
            Slider::Display => DISPLAY_TRACK,
            Slider::Sound => SOUND_TRACK,
        };
        let key = if header {
            DETAIL_FOCUS.to_owned()
        } else {
            focus_key(module)
        };
        let level = self.level(
            origin,
            &key,
            slider,
            Rect::new(
                start - LEVEL_REACH,
                TRACK_CENTRE - SLIDER_BAND / 2.0,
                end - start + 2.0 * LEVEL_REACH,
                SLIDER_BAND,
            ),
        );
        let symbol_at = |x: f64, size: f64, name: &str| {
            Place::centred(
                Rect::new(x, TRACK_CENTRE - size / 2.0, size, size),
                icon(name, size, None),
            )
        };
        let mut children = vec![title, level];
        match slider {
            Slider::Display => {
                children.push(symbol_at(15.0, 14.0, "symbol.sun"));
                children.push(symbol_at(266.0, 18.0, "symbol.sun"));
            }
            Slider::Sound => {
                children.push(symbol_at(14.0, 14.0, "symbol.speaker.low"));
                children.push(symbol_at(219.0, 20.0, "symbol.speaker.high"));
                let output = Rect::new(
                    OUTPUT_CENTRE_X - OUTPUT_BUTTON / 2.0,
                    TRACK_CENTRE - OUTPUT_BUTTON / 2.0,
                    OUTPUT_BUTTON,
                    OUTPUT_BUTTON,
                );
                children.push(self.pressable(
                    origin,
                    &format!("{}.output", focus_key(module)),
                    "Output",
                    Kind::Expander(header),
                    Face::Badge { on: false },
                    output,
                    true,
                    Some(if header {
                        Input::Collapse(Via::Pointer)
                    } else {
                        Input::Expand(Detail::Sound, Via::Pointer)
                    }),
                    vec![icon("symbol.speaker", 14.0, None)],
                ));
            }
        }
        children
    }

    /// The content of `module`'s tile at the tile's origin in the window.
    fn content(&self, module: Module, origin: Point) -> Vec<Element<M>> {
        match module {
            Module::WiFi | Module::Bluetooth | Module::Microphone | Module::FocusMode => {
                self.capsule(module, origin, &focus_key(module), false)
            }
            Module::DockHiding | Module::LockScreen | Module::Mute | Module::KeyboardBrightness => {
                self.circle(module, origin)
            }
            Module::NowPlaying => self.media(origin),
            Module::Display => self.slider(Slider::Display, origin, false),
            Module::Sound => self.slider(Slider::Sound, origin, false),
        }
    }

    /// The rows of `detail`'s list: label, symbol, whether it is the current one, and what a press does.
    fn rows(&self, detail: Detail) -> Result<Vec<Row>, String> {
        let controls = self.controls;
        let rows = match detail {
            Detail::WiFi => match &controls.networks {
                Reading::Ready(networks) => networks
                    .iter()
                    .map(|n| Row {
                        label: n.ssid.clone(),
                        symbol: "symbol.wifi",
                        trailing: if n.connected {
                            Some("symbol.checkmark")
                        } else if n.secured {
                            Some("symbol.lock")
                        } else {
                            None
                        },
                        current: n.connected,
                        on_press: n.joinable().then(|| Input::Join(n.ssid.clone())),
                    })
                    .collect(),
                Reading::Unavailable(reason) => return Err(reason.clone()),
            },
            Detail::Bluetooth => match &controls.devices {
                Reading::Ready(devices) => devices
                    .iter()
                    .map(|d| Row {
                        label: d.name.clone(),
                        symbol: "symbol.bluetooth",
                        trailing: d.connected.then_some("symbol.checkmark"),
                        current: d.connected,
                        on_press: Some(Input::Connect(d.id.clone())),
                    })
                    .collect(),
                Reading::Unavailable(reason) => return Err(reason.clone()),
            },
            Detail::Sound => match &controls.outputs {
                Reading::Ready(outputs) => outputs
                    .iter()
                    .map(|o| Row {
                        label: o.name.clone(),
                        symbol: "symbol.speaker",
                        trailing: o.current.then_some("symbol.checkmark"),
                        current: o.current,
                        on_press: (!o.current).then_some(Input::Output(o.id)),
                    })
                    .collect(),
                Reading::Unavailable(reason) => return Err(reason.clone()),
            },
            Detail::Display => {
                let module = Module::KeyboardBrightness;
                match controls.tile(module) {
                    TileState::Disabled(reason) => return Err(reason),
                    state => vec![Row {
                        label: module.title().to_owned(),
                        symbol: "symbol.keyboard-brightness",
                        trailing: (state == TileState::On).then_some("symbol.checkmark"),
                        current: state == TileState::On,
                        on_press: Some(Input::Press(module)),
                    }],
                }
            }
        };
        Ok(rows)
    }

    /// The number of list rows `detail` shows.
    fn row_count(&self, detail: Detail) -> usize {
        self.rows(detail)
            .map_or(1, |rows| rows.len())
            .clamp(1, layout::detail_rows())
    }

    /// The open view's content at the view's origin in the window.
    fn detail(&self, detail: Detail, origin: Point) -> Vec<Element<M>> {
        let module = detail.module();
        let mut children = match detail {
            Detail::WiFi | Detail::Bluetooth => self.capsule(module, origin, DETAIL_FOCUS, true),
            Detail::Display => self.slider(Slider::Display, origin, true),
            Detail::Sound => self.slider(Slider::Sound, origin, true),
        };
        let width = layout::GRID_WIDTH;
        let row_width = width - 2.0 * rows::ROW_INSET;
        children.push(Place::stack(
            Rect::new(
                rows::ROW_INSET + rows::TEXT_INSET,
                rows::HEADER,
                row_width - 2.0 * rows::TEXT_INSET,
                rows::HEADING,
            ),
            Axis::Horizontal,
            false,
            vec![text(
                detail.heading(),
                TypeSize::Caption,
                TITLE_WEIGHT,
                Tone::Secondary,
            )],
        ));
        let mut y = rows::HEADER + rows::HEADING;
        match self.rows(detail) {
            Ok(list) if !list.is_empty() => {
                let list: Vec<_> = list.into_iter().take(layout::detail_rows()).collect();
                let at = Point::new(origin.x, origin.y + y);
                let items = list
                    .into_iter()
                    .enumerate()
                    .map(|(i, row)| {
                        let frame = Rect::new(
                            rows::ROW_INSET,
                            i as f64 * rows::ITEM,
                            row_width,
                            rows::ITEM,
                        );
                        self.row(at, i, frame, row)
                    })
                    .collect::<Vec<_>>();
                let height = items.len() as f64 * rows::ITEM;
                children.push(Place::list(
                    Rect::new(0.0, y, width, height),
                    detail.heading(),
                    items,
                ));
                y += height;
            }
            outcome => {
                let message = match outcome {
                    Err(reason) => format!("Unavailable: {reason}"),
                    Ok(_) => "None found".to_owned(),
                };
                children.push(Place::stack(
                    Rect::new(
                        rows::ROW_INSET + rows::TEXT_INSET,
                        y,
                        row_width - 2.0 * rows::TEXT_INSET,
                        rows::ITEM,
                    ),
                    Axis::Horizontal,
                    false,
                    vec![text(&message, TypeSize::Body, ROW_WEIGHT, Tone::Secondary)],
                ));
                y += rows::ITEM;
            }
        }
        children.push(Element::new(Line {
            frame: Rect::new(
                rows::ROW_INSET + rows::TEXT_INSET,
                y + (rows::SEPARATOR - 1.0) / 2.0,
                row_width - 2.0 * rows::TEXT_INSET,
                1.0,
            ),
        }));
        y += rows::SEPARATOR;
        children.push(self.pressable(
            origin,
            "detail.settings",
            detail.settings_label(),
            Kind::Button,
            Face::Plain {
                radius: ROW_RADIUS,
                hover: true,
            },
            Rect::new(rows::ROW_INSET, y, row_width, rows::ITEM),
            true,
            Some(Input::Settings(detail)),
            vec![Place::stack(
                Rect::new(0.0, 0.0, row_width, rows::ITEM),
                Axis::Horizontal,
                false,
                vec![
                    Element::new(Spacer8),
                    text(
                        detail.settings_label(),
                        TypeSize::Body,
                        ROW_WEIGHT,
                        Tone::Primary,
                    ),
                ],
            )],
        ));
        children
    }

    fn row(&self, origin: Point, index: usize, frame: Rect, row: Row) -> Element<M> {
        let key = &format!("detail.row.{index}");
        let width = frame.width();
        let mut parts = vec![
            Place::centred(
                Rect::new(
                    rows::TEXT_INSET,
                    (rows::ITEM - ROW_SYMBOL) / 2.0,
                    ROW_SYMBOL,
                    ROW_SYMBOL,
                ),
                icon(row.symbol, ROW_SYMBOL, None),
            ),
            Place::stack(
                Rect::new(
                    rows::TEXT_INSET + ROW_SYMBOL + 10.0,
                    0.0,
                    width - 2.0 * rows::TEXT_INSET - 2.0 * ROW_SYMBOL - 20.0,
                    rows::ITEM,
                ),
                Axis::Horizontal,
                false,
                vec![text(&row.label, TypeSize::Body, ROW_WEIGHT, Tone::Primary)],
            ),
        ];
        if let Some(trailing) = row.trailing {
            parts.push(Place::centred(
                Rect::new(
                    width - rows::TEXT_INSET - ROW_SYMBOL,
                    (rows::ITEM - ROW_SYMBOL) / 2.0,
                    ROW_SYMBOL,
                    ROW_SYMBOL,
                ),
                icon(trailing, ROW_SYMBOL - 2.0, None),
            ));
        }
        let enabled = row.on_press.is_some() || row.current;
        self.pressable(
            origin,
            key,
            &row.label,
            Kind::Row {
                index,
                selected: row.current,
            },
            Face::Plain {
                radius: ROW_RADIUS,
                hover: true,
            },
            frame,
            enabled,
            row.on_press,
            vec![Place::absolute(
                Rect::from_origin_size(Point::ZERO, frame.size),
                parts,
            )],
        )
    }

    /// The Edit Controls capsule under the grid.
    fn edit(&self, alpha: f64) -> Element<M> {
        let frame = layout::edit_frame();
        let origin = frame.origin;
        let origin = Point::new(origin.x + GRID_ORIGIN.x, origin.y + GRID_ORIGIN.y);
        let size = frame.size;
        self.tile(
            "Edit Controls",
            frame,
            Outline::Capsule,
            alpha,
            vec![self.pressable(
                origin,
                "edit-controls",
                "Edit Controls",
                Kind::Button,
                Face::Plain {
                    radius: size.height / 2.0,
                    hover: false,
                },
                Rect::from_origin_size(Point::ZERO, size),
                true,
                Some(Input::EditControls),
                vec![text(
                    "Edit Controls",
                    TypeSize::Body,
                    EDIT_WEIGHT,
                    Tone::Primary,
                )],
            )],
        )
    }

    /// Height of what the tiles cover: the grid with the Edit Controls capsule under it, going to the open detail
    /// view's height as it opens.
    fn height(&self) -> f64 {
        let grid = layout::edit_frame().max_y();
        self.presentation.detail.map_or(grid, |o| {
            let open = layout::detail_frame(self.row_count(o.detail)).height();
            grid + (open - grid) * o.progress.clamp(0.0, 1.0)
        })
    }

    /// The outlines of the tiles at rest, from the grid's corner: what the soft shadow is cut away over. None while a
    /// detail view opens or closes, when the tiles move.
    fn holes(&self) -> Vec<(Rect, f64)> {
        if self.presentation.detail.is_some() {
            return Vec::new();
        }
        let mut holes: Vec<(Rect, f64)> = Module::ALL
            .into_iter()
            .map(|m| {
                let frame = layout::frame(m);
                (frame, layout::outline(m).radius(frame.size))
            })
            .collect();
        let edit = layout::edit_frame();
        holes.push((edit, edit.height() / 2.0));
        holes
    }

    /// What the soft shadow goes around: the grid with the Edit Controls capsule, going to the open detail view's
    /// outline as it opens. It fades with the panel.
    fn shadow(&self) -> Rect {
        Rect::new(0.0, 0.0, GRID_WIDTH, self.height())
    }

    /// Every tile, with the detail view opening, open or closing.
    fn board(&self) -> Vec<Element<M>> {
        let opened = self.presentation.detail;
        let shown = opened.map_or(
            Visibility {
                others: 1.0,
                tile_content: 1.0,
                detail_content: 0.0,
            },
            |o| visibility(o.progress),
        );
        let source = opened.map(|o| o.detail.module());
        let mut children = Vec::new();
        for module in Module::ALL {
            if Some(module) == source {
                continue;
            }
            if shown.others <= 0.0 {
                continue;
            }
            let frame = layout::frame(module);
            let origin = Point::new(frame.x() + GRID_ORIGIN.x, frame.y() + GRID_ORIGIN.y);
            let content = self.content(module, origin);
            children.push(
                self.tile(
                    module.title(),
                    frame,
                    layout::outline(module),
                    shown.others,
                    content,
                )
                .key(format!("tile.{}", slug(module))),
            );
        }
        if shown.others > 0.0 {
            children.push(self.edit(shown.others).key("tile.edit"));
        }
        if let Some(opened) = opened {
            children.extend(self.opening(opened, shown));
        }
        children
    }

    /// The tile of the detail view: grown from its place with its content giving way to the view's, or with Reduce
    /// Motion the tile fading in place and the view fading in at its own place.
    fn opening(&self, opened: Opened, shown: Visibility) -> Vec<Element<M>> {
        let detail = opened.detail;
        let module = detail.module();
        let rows = self.row_count(detail);
        let open = layout::detail_frame(rows);
        let tile_frame = layout::frame(module);
        let at = |frame: Rect| Point::new(frame.x() + GRID_ORIGIN.x, frame.y() + GRID_ORIGIN.y);
        let faded = |alpha: f64, children: Vec<Element<M>>| {
            if alpha >= 1.0 {
                children
            } else {
                vec![Element::new(Fade {
                    frame: Rect::from_origin_size(
                        Point::ZERO,
                        Size::new(open.width(), open.height()),
                    ),
                    alpha,
                    children,
                })]
            }
        };
        let label = module.title();
        if !opened.morph {
            let mut tiles = Vec::new();
            let p = opened.progress.clamp(0.0, 1.0);
            if p < 1.0 {
                let content = self.content(module, at(tile_frame));
                tiles.push(
                    self.tile(label, tile_frame, layout::outline(module), 1.0 - p, content)
                        .key(format!("tile.{}", slug(module))),
                );
            }
            if p > 0.0 {
                let content = self.detail(detail, at(open));
                tiles.push(
                    self.tile(label, open, Outline::Rounded(LARGE_RADIUS), p, content)
                        .key("tile.detail"),
                );
            }
            return tiles;
        }
        let frame = layout::morph(detail, rows, opened.progress);
        let p = opened.progress.clamp(0.0, 1.0);
        let from = layout::outline(module).radius(tile_frame.size);
        let radius = from + (LARGE_RADIUS - from) * p;
        let mut content = Vec::new();
        if shown.tile_content > 0.0 {
            content.extend(faded(shown.tile_content, self.content(module, at(frame))));
        }
        if shown.detail_content > 0.0 {
            content.extend(faded(shown.detail_content, self.detail(detail, at(frame))));
        }
        vec![self
            .tile(label, frame, Outline::Rounded(radius), 1.0, content)
            .key(format!("tile.{}", slug(module)))]
    }
}

/// A row of a detail view's list.
struct Row {
    label: String,
    symbol: &'static str,
    trailing: Option<&'static str>,
    current: bool,
    on_press: Option<Input>,
}

/// An 8 point gap before a label.
struct Spacer8;

impl<M: 'static> Widget<M> for Spacer8 {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(8.0),
            height: SizeSpec::Fixed(1.0),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(8.0, 1.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A drawn shape with no input: the media tile's artwork placeholder, or a detail view's separator.
macro_rules! shape_widget {
    ($name:ident, $color:expr, $radius:expr) => {
        struct $name {
            frame: Rect,
        }

        impl<M: 'static> Widget<M> for $name {
            type State = ();

            fn layout(&self) -> LayoutSpec {
                spec(ContainerKind::Leaf, self.frame)
            }

            fn state(&self) {}

            fn update(&self, old: &Self, _: &mut ()) -> Change {
                if self.frame != old.frame {
                    Change::Layout
                } else {
                    Change::None
                }
            }

            fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
                Measured {
                    size: self.frame.size,
                    baseline: None,
                }
            }

            fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
                Propagation::Continue
            }

            fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
                let rect = Rect::from_origin_size(Point::ZERO, cx.size());
                fill(cx, rect, $radius, $color);
            }

            fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
        }
    };
}

shape_widget!(Artwork, ARTWORK, ARTWORK_RADIUS);
shape_widget!(Line, SEPARATOR, 0.0);

/// Content faded as one layer.
struct Fade<M> {
    frame: Rect,
    alpha: f64,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Fade<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Absolute, self.frame)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.frame != old.frame {
            Change::Layout
        } else if self.alpha != old.alpha {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.frame.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        cx.set_group(finite_alpha(self.alpha) as f32, Vec::new());
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The panel's window: the tiles at `presentation`. Escape collapses an open detail view, else dismisses the panel.
pub fn panel<M: Clone + 'static>(
    controls: &Controls,
    presentation: Presentation,
    to: fn(Input) -> M,
) -> Window<M> {
    let builder = Builder {
        controls,
        presentation,
        to,
        map: Rc::new(RefCell::new(Vec::new())),
    };
    // Escape and a press outside the open view close it; with no view open they close the panel.
    let away = |via| {
        if presentation.detail.is_some() {
            Input::Collapse(via)
        } else {
            Input::Dismiss
        }
    };
    Window {
        key: WindowKey(Key::from("panel")),
        spec: WindowSpec {
            title: TITLE.to_owned(),
            app_id: APP_ID.to_owned(),
            size: SURFACE,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            // only the modules take input: not the shadow's margin, and nothing while the panel fades out
            input_region: Some(if presentation.input {
                vec![Rect::from_origin_size(WINDOW_ORIGIN, WINDOW)]
            } else {
                Vec::new()
            }),
        },
        shortcuts: CommandRegistry::new().with(
            Shortcut {
                mods: Mods::NONE,
                keysym: ESCAPE,
            },
            to(away(Via::Keyboard)),
        ),
        on_close: Some(to(Input::Closed)),
        background: WindowBackground::None,
        content: Element::new(Board {
            alpha: presentation.alpha,
            height: builder.height(),
            shadow: builder.shadow(),
            holes: builder.holes(),
            on_outside: to(away(Via::Pointer)),
            children: builder.board(),
        }),
        popup: None,
        menus: None,
    }
}

/// Takes a left or right press anywhere on it and draws nothing.
struct Catcher<M> {
    on_press: M,
}

impl<M: Clone + 'static> Widget<M> for Catcher<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            // a left or right press outside the panel dismisses it
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left | MouseButton::Right,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    /// Not something assistive technology can reach: Escape dismisses the panel from the keyboard.
    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The transparent window under the panel, over the whole output while the panel is shown.
pub fn catcher<M: Clone + 'static>(to: fn(Input) -> M) -> Window<M> {
    Window {
        key: WindowKey(Key::from("catcher")),
        spec: WindowSpec {
            title: TITLE.to_owned(),
            app_id: CATCHER_APP_ID.to_owned(),
            size: Size::ZERO,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: Some(to(Input::Closed)),
        // shown without a frame of the output's size: it draws nothing
        background: WindowBackground::INPUT_ONLY,
        content: Element::new(Catcher {
            on_press: to(Input::Dismiss),
        }),
        popup: None,
        menus: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arrows_go_to_the_nearest_control_that_way() {
        let map = vec![
            ("a".to_owned(), Rect::new(0.0, 0.0, 140.0, 64.0)),
            ("b".to_owned(), Rect::new(152.0, 0.0, 140.0, 140.0)),
            ("c".to_owned(), Rect::new(0.0, 76.0, 140.0, 64.0)),
            ("d".to_owned(), Rect::new(0.0, 304.0, 292.0, 64.0)),
        ];
        let from = map[0].1;
        assert_eq!(nearest(&map, "a", from, (1.0, 0.0)).as_deref(), Some("b"));
        assert_eq!(nearest(&map, "a", from, (0.0, 1.0)).as_deref(), Some("c"));
        assert_eq!(nearest(&map, "a", from, (-1.0, 0.0)), None);
        assert_eq!(
            nearest(&map, "c", map[2].1, (0.0, 1.0)).as_deref(),
            Some("d")
        );
    }
}
