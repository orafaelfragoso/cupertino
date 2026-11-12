//! The tool's surfaces as CupertinoKit windows: the overlay of each output (the dim with its clear selection, the
//! tint of the direct shortcut, the window pick's highlight, the size readout and the crosshair), the toolbar with its
//! Options menu, the floating thumbnail, and the transparent pixel that takes the keyboard while the capture is put on
//! the clipboard. Each window's `app_id` tells the process which layer surface it gets.
//!
//! The overlay and the thumbnail are widgets of their own: they draw from the model, and turn the pointer, the keys and
//! the actions of assistive technology into [`Input`]. The toolbar is built from the toolkit's buttons and segmented
//! control, and the Options menu is the toolkit's menu.

use cupertino_geometry::{Point, Rect, Size};
use cupertino_kit::controls::{Button, ButtonRole, ButtonStyle, Menu, MenuAnchor, MenuItem};
use cupertino_kit::controls::{SegmentedControl, Spacer, Stack};
use cupertino_kit::core::{
    AccessibleValue, Action, Change, CommandRegistry, Element, Event, EventCx, Key as NodeKey,
    MeasureCx, PaintCx, Phase as RoutePhase, Propagation, Role, SemanticsCx, Widget, Window,
    WindowBackground, WindowKey,
};
use cupertino_kit::geometry::Insets;
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec,
};
use cupertino_kit::platform::{
    CursorShape, DecorationMode, KeyEvent, Mods, MouseButton, PointerEvent, WindowSpec,
};
use cupertino_kit::render::{
    Effect, ImageResource, Paint, PaintOp, PixelFormat, ResourceId, Sampling, Shape,
};
use cupertino_kit::text::{AttributedText, FontWeight, ParagraphStyle, TextStyle};
use cupertino_kit::theme::Color;
use cupertino_kit::theme::Surface;

use crate::selection::{self, Handle};
use crate::session::{Key, KeyMods, Pointer, Session, Style};
use crate::settings::{Mode, Settings};
use crate::thumbnail::Look;
use crate::toolbar::{self, MenuAction, MenuRow};
use crate::values::{
    Rgba, BORDER_DASH, BORDER_WIDTH, CROSSHAIR_ARM, CROSSHAIR_CENTRE, CROSSHAIR_CORE,
    CROSSHAIR_GAP, CROSSHAIR_HALO, DIM, HANDLE_FILL, HANDLE_STROKE, LABEL_BACKDROP,
    LABEL_FONT_SIZE, LABEL_RADIUS, LABEL_TEXT, PICK_HIGHLIGHT, SIMPLE_TINT, THUMBNAIL_BORDER,
    THUMBNAIL_BORDER_COLOR, THUMBNAIL_RADIUS, THUMBNAIL_SHADOW_ALPHA, THUMBNAIL_SHADOW_BLUR,
    TOOLBAR_PADDING, TOOLBAR_RADIUS, TOOLBAR_SIZE, TOOLBAR_SPACING,
};

/// The `app_id` of an output's overlay is this, a slash and the output's index.
pub const OVERLAY_APP_ID: &str = "cupertino-screenshot.overlay";
pub const TOOLBAR_APP_ID: &str = "cupertino-screenshot.toolbar";
pub const THUMBNAIL_APP_ID: &str = "cupertino-screenshot.thumbnail";
pub const CLIPBOARD_APP_ID: &str = "cupertino-screenshot.clipboard";
/// What assistive technology calls the overlay and the toolbar.
pub const OVERLAY_TITLE: &str = "Screenshot";
pub const TOOLBAR_TITLE: &str = "Screenshot Toolbar";
pub const THUMBNAIL_TITLE: &str = "Screenshot Thumbnail";
/// The key of the Options button, under which its menu opens.
pub const OPTIONS_KEY: &str = "options";
/// The mode buttons, in the toolbar's order.
pub const MODES: [(Mode, &str); 3] = [
    (Mode::Screen, "Entire Screen"),
    (Mode::Window, "Window"),
    (Mode::Selection, "Selection"),
];

/// Diameter of the round handle drawn in each handle square (approximate: the handle path is not known).
const HANDLE_DOT: f64 = 9.0;
/// Text weight of the size readout (approximate).
const LABEL_WEIGHT: u16 = 400;

const ESCAPE: u32 = 0xff1b;
const SPACE: u32 = 0x20;
const RETURN: u32 = 0xff0d;
const KEYPAD_ENTER: u32 = 0xff8d;
const LEFT: u32 = 0xff51;
const UP: u32 = 0xff52;
const RIGHT: u32 = 0xff53;
const DOWN: u32 = 0xff54;

/// The `app_id` of output `index`'s overlay.
pub fn overlay_app_id(index: usize) -> String {
    format!("{OVERLAY_APP_ID}/{index}")
}

/// The output index of an overlay's `app_id`.
pub fn overlay_index(app_id: &str) -> Option<usize> {
    app_id
        .strip_prefix(OVERLAY_APP_ID)?
        .strip_prefix('/')?
        .parse()
        .ok()
}

/// What the pointer did on an overlay, in global points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerInput {
    Moved(Point),
    Button {
        pressed: bool,
        at: Point,
        control: bool,
    },
}

/// What the pointer or assistive technology did on the thumbnail, in its surface's points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ThumbnailInput {
    Press(Point),
    Motion(Point),
    Release,
    /// A click from assistive technology: commit at once.
    Activate,
}

/// What the user did on the tool's surfaces.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Pointer(PointerInput),
    Key(Key, KeyMods),
    /// A mode button.
    Mode(Mode),
    /// The Capture button.
    Capture,
    /// Whether Control was held at a press or release on the toolbar, heard before the button's own message.
    Control(bool),
    /// The close button.
    Cancel,
    /// Open or close the Options menu.
    Options(bool),
    Choose(MenuAction),
    /// An overlay showed the arrow pointer again before it goes.
    ArrowShown,
    Thumbnail(ThumbnailInput),
    /// The compositor closed a surface.
    Closed,
}

fn color(c: Rgba) -> Color {
    Color::new(c[0], c[1], c[2], c[3])
}

fn fill(cx: &mut PaintCx<'_>, shape: Shape, c: Rgba) {
    cx.push(PaintOp::Fill {
        shape,
        paint: Paint::Solid(color(c)),
    });
}

fn mods(m: Mods) -> KeyMods {
    KeyMods {
        control: m.contains(Mods::CTRL),
        shift: m.contains(Mods::SHIFT),
        alt: m.contains(Mods::ALT),
    }
}

/// The overlay's key for a key event, if it reacts to it.
fn key_of(event: &KeyEvent) -> Option<Key> {
    Some(match event.keysym.0 {
        ESCAPE => Key::Escape,
        SPACE => Key::Space,
        RETURN | KEYPAD_ENTER => Key::Return,
        LEFT => Key::Left,
        RIGHT => Key::Right,
        UP => Key::Up,
        DOWN => Key::Down,
        _ => return None,
    })
}

/// The system pointer for the overlay's look: none under the drawn crosshair.
fn cursor(look: Pointer) -> Option<CursorShape> {
    match look {
        Pointer::Crosshair => None,
        Pointer::Arrow => Some(CursorShape::Default),
    }
}

/// What one output's overlay shows. Rectangles and points are global; `bounds` is the output.
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayView {
    pub bounds: Rect,
    pub style: Style,
    pub mode: Mode,
    pub selection: Option<Rect>,
    /// The size readout is shown while the selection is spanned or resized.
    pub sizing: bool,
    pub picked: Option<Rect>,
    pub pointer: Option<Point>,
    pub look: Pointer,
    /// The overlay is about to go: it shows the arrow, so the pointer is visible over what was under it, and says so.
    pub leaving: bool,
    /// The overlay takes the keys (the direct shortcut's first output; the toolbar takes them otherwise).
    pub keys: bool,
}

impl OverlayView {
    /// The overlay of `bounds` for `session`.
    pub fn of(session: &Session, bounds: Rect, keys: bool, leaving: bool) -> Self {
        Self {
            bounds,
            style: session.style(),
            mode: session.mode(),
            selection: session.selection(),
            sizing: session.sizing(),
            picked: session.picked().map(|w| w.frame),
            pointer: session.pointer(),
            look: if leaving {
                Pointer::Arrow
            } else {
                session.pointer_look()
            },
            leaving,
            keys,
        }
    }

    /// What assistive technology reads as the overlay's value: what a capture would take now.
    pub fn summary(&self) -> String {
        match self.mode {
            Mode::Selection => match self.selection {
                Some(r) => format!(
                    "Selection {} at {}, {}",
                    selection::label_text(&r),
                    r.x().round() as i64,
                    r.y().round() as i64
                ),
                None => "No selection".into(),
            },
            Mode::Window => match self.picked {
                Some(r) => format!(
                    "Window {} at {}, {}",
                    selection::label_text(&r),
                    r.x().round() as i64,
                    r.y().round() as i64
                ),
                None => "No window chosen".into(),
            },
            Mode::Screen => "Entire screen".into(),
        }
    }

    fn local(&self, r: &Rect) -> Rect {
        r.translate(-self.bounds.x(), -self.bounds.y())
    }

    /// The pointer on this output, in its points.
    fn local_pointer(&self) -> Option<Point> {
        self.pointer
            .filter(|p| self.bounds.contains(*p))
            .map(|p| Point::new(p.x - self.bounds.x(), p.y - self.bounds.y()))
    }
}

/// An output's overlay: draws [`OverlayView`] and reports the pointer and the keys.
struct Overlay<M> {
    view: OverlayView,
    to: fn(Input) -> M,
}

impl<M: 'static> Overlay<M> {
    fn global(&self, p: Point) -> Point {
        Point::new(p.x + self.view.bounds.x(), p.y + self.view.bounds.y())
    }

    fn paint_selection(&self, cx: &mut PaintCx<'_>, full: Rect) {
        let v = &self.view;
        let selection = v
            .selection
            .filter(|r| r.intersects(&v.bounds))
            .map(|r| v.local(&r));
        match (v.style, v.mode) {
            (Style::Interactive, Mode::Selection) => match selection {
                Some(sel) => {
                    // the dim around the clear selection
                    for r in [
                        Rect::from_ltrb(full.min_x(), full.min_y(), full.max_x(), sel.min_y()),
                        Rect::from_ltrb(full.min_x(), sel.max_y(), full.max_x(), full.max_y()),
                        Rect::from_ltrb(full.min_x(), sel.min_y(), sel.min_x(), sel.max_y()),
                        Rect::from_ltrb(sel.max_x(), sel.min_y(), full.max_x(), sel.max_y()),
                    ] {
                        if r.width() > 0.0 && r.height() > 0.0 {
                            fill(cx, Shape::Rect(r), DIM);
                        }
                    }
                    border(cx, &sel);
                    for h in Handle::ALL {
                        let c = h.anchor(&sel);
                        let dot = Rect::new(
                            c.x - HANDLE_DOT / 2.0,
                            c.y - HANDLE_DOT / 2.0,
                            HANDLE_DOT,
                            HANDLE_DOT,
                        );
                        let shape = Shape::RoundedRect {
                            rect: dot,
                            radius: HANDLE_DOT / 2.0,
                        };
                        fill(cx, shape.clone(), HANDLE_FILL);
                        cx.push(PaintOp::Stroke {
                            shape,
                            width: HANDLE_STROKE,
                            paint: Paint::Solid(color([1.0, 1.0, 1.0, 1.0])),
                        });
                    }
                }
                None => fill(cx, Shape::Rect(full), DIM),
            },
            (Style::Simple, Mode::Selection) => {
                if let Some(sel) = selection {
                    fill(cx, Shape::Rect(sel), SIMPLE_TINT);
                }
            }
            (_, Mode::Window) => {
                if let Some(r) = v.picked.and_then(|w| w.intersection(&v.bounds)) {
                    fill(cx, Shape::Rect(v.local(&r)), PICK_HIGHLIGHT);
                }
            }
            (_, Mode::Screen) => {}
        }
    }

    /// The size readout beside the pointer while the selection is spanned or resized.
    fn paint_readout(&self, cx: &mut PaintCx<'_>, full: Rect, pointer: Point) {
        let Some(sel) = self.view.selection.filter(|_| self.view.sizing) else {
            return;
        };
        let text = selection::label_text(&sel);
        let style = TextStyle {
            weight: FontWeight(LABEL_WEIGHT),
            ..TextStyle::new(LABEL_FONT_SIZE as f32)
        };
        let Ok(paragraph) = cx.env().text.paragraph(
            &AttributedText::plain(&text, style),
            &ParagraphStyle::default(),
        ) else {
            return;
        };
        let layout = paragraph.layout(None);
        let size = layout.size();
        let r = selection::label_rect(size.width, pointer, &full);
        fill(
            cx,
            Shape::RoundedRect {
                rect: r,
                radius: LABEL_RADIUS,
            },
            LABEL_BACKDROP,
        );
        let origin = Point::new(
            r.x() + (r.width() - size.width) / 2.0,
            r.y() + (r.height() - size.height) / 2.0,
        );
        for line in layout.lines() {
            for run in &line.runs {
                cx.push(PaintOp::Glyphs {
                    origin: origin + run.origin,
                    run: run.run.clone(),
                    color: color(LABEL_TEXT),
                });
            }
        }
    }
}

/// The interactive selection's border: a black line under white dashes, both [`BORDER_WIDTH`] wide and centred on
/// the selection's edge, the dashes running clockwise from the top-left corner.
fn border(cx: &mut PaintCx<'_>, sel: &Rect) {
    cx.push(PaintOp::Stroke {
        shape: Shape::Rect(*sel),
        width: BORDER_WIDTH,
        paint: Paint::Solid(color([0.0, 0.0, 0.0, 1.0])),
    });
    let (on, off) = (f64::from(BORDER_DASH[0]), f64::from(BORDER_DASH[1]));
    let half = BORDER_WIDTH / 2.0;
    let (l, t, r, b) = (sel.min_x(), sel.min_y(), sel.max_x(), sel.max_y());
    // each edge as its start, its direction and its length
    let edges = [
        (Point::new(l, t), (1.0, 0.0), sel.width()),
        (Point::new(r, t), (0.0, 1.0), sel.height()),
        (Point::new(r, b), (-1.0, 0.0), sel.width()),
        (Point::new(l, b), (0.0, -1.0), sel.height()),
    ];
    let white = color([1.0, 1.0, 1.0, 1.0]);
    let mut phase = 0.0;
    for (start, (dx, dy), length) in edges {
        let mut at = 0.0;
        while at < length {
            let in_dash = phase % (on + off);
            let run = if in_dash < on {
                (on - in_dash).min(length - at)
            } else {
                (on + off - in_dash).min(length - at)
            };
            if in_dash < on && run > 0.0 {
                let a = Point::new(start.x + dx * at, start.y + dy * at);
                let z = Point::new(start.x + dx * (at + run), start.y + dy * (at + run));
                let dash = Rect::from_ltrb(
                    a.x.min(z.x) - half * dy.abs(),
                    a.y.min(z.y) - half * dx.abs(),
                    a.x.max(z.x) + half * dy.abs(),
                    a.y.max(z.y) + half * dx.abs(),
                );
                cx.push(PaintOp::Fill {
                    shape: Shape::Rect(dash),
                    paint: Paint::Solid(white),
                });
            }
            at += run;
            phase += run;
        }
    }
}

/// The crosshair with its centre pixel at `p` (surface points): arms with a centre gap, a dark core with a light halo.
fn crosshair(cx: &mut PaintCx<'_>, p: Point) {
    // asset pixel c spans c..c+1, so the centre pixel's middle is at CROSSHAIR_CENTRE + 0.5
    let mid = CROSSHAIR_CENTRE + 0.5;
    let (from, to) = (CROSSHAIR_ARM.0 - mid, CROSSHAIR_ARM.1 + 1.0 - mid);
    let arms = [(from, -CROSSHAIR_GAP), (CROSSHAIR_GAP, to)];
    for (halo, c) in [(1.0, CROSSHAIR_HALO), (0.0, CROSSHAIR_CORE)] {
        for (a, b) in arms {
            let horizontal = Rect::new(
                p.x + a - halo,
                p.y - 0.5 - halo,
                b - a + 2.0 * halo,
                1.0 + 2.0 * halo,
            );
            let vertical = Rect::new(
                p.x - 0.5 - halo,
                p.y + a - halo,
                1.0 + 2.0 * halo,
                b - a + 2.0 * halo,
            );
            fill(cx, Shape::Rect(horizontal), c);
            fill(cx, Shape::Rect(vertical), c);
        }
    }
}

fn fill_layout() -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fill,
        height: SizeSpec::Fill,
        ..LayoutSpec::new(ContainerKind::Leaf)
    }
}

impl<M: 'static> Widget<M> for Overlay<M> {
    /// Whether the view changed since its last frame was asked for: the system pointer follows the look on that frame.
    type State = std::cell::Cell<bool>;

    fn layout(&self) -> LayoutSpec {
        fill_layout()
    }

    fn state(&self) -> std::cell::Cell<bool> {
        std::cell::Cell::new(true)
    }

    fn update(&self, old: &Self, owed: &mut std::cell::Cell<bool>) -> Change {
        if self.view == old.view {
            Change::None
        } else {
            owed.set(true);
            Change::Paint
        }
    }

    fn measure(
        &self,
        _: &std::cell::Cell<bool>,
        _: Constraints,
        _: &mut MeasureCx<'_>,
    ) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(
        &self,
        _: &mut std::cell::Cell<bool>,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        let to = self.to;
        match event {
            Event::Pointer(
                PointerEvent::Enter { position } | PointerEvent::Motion { position },
                _,
            ) => {
                cx.set_cursor(cursor(self.view.look));
                cx.emit(to(Input::Pointer(PointerInput::Moved(
                    self.global(*position),
                ))));
                Propagation::Stop
            }
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed,
                    mods,
                    ..
                },
                _,
            ) => {
                if *pressed {
                    cx.capture_pointer();
                } else {
                    cx.release_pointer();
                }
                cx.emit(to(Input::Pointer(PointerInput::Button {
                    pressed: *pressed,
                    at: self.global(*position),
                    control: mods.contains(Mods::CTRL),
                })));
                Propagation::Stop
            }
            Event::Key(key, _) if key.pressed => match key_of(key) {
                Some(k) => {
                    cx.emit(to(Input::Key(k, mods(key.mods))));
                    Propagation::Stop
                }
                None => Propagation::Continue,
            },
            Event::Action(request) => match request.action {
                Action::Click => {
                    cx.emit(to(Input::Key(Key::Return, KeyMods::NONE)));
                    Propagation::Stop
                }
                Action::Focus => {
                    cx.request_focus();
                    Propagation::Stop
                }
                _ => Propagation::Continue,
            },
            // the look changed since the pointer last moved: set it now
            Event::AnimationFrame(_) => {
                cx.set_cursor(cursor(self.view.look));
                if self.view.leaving {
                    cx.emit(to(Input::ArrowShown));
                }
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, owed: &std::cell::Cell<bool>, cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let full = Rect::new(0.0, 0.0, size.width, size.height);
        self.paint_selection(cx, full);
        let pointer = self.view.local_pointer();
        if let Some(p) = pointer {
            self.paint_readout(cx, full, p);
            if self.view.look == Pointer::Crosshair && !self.view.leaving {
                crosshair(cx, p);
            }
        }
        // the system pointer follows the look on the next frame, and before the overlay goes; one frame per change, so
        // an overlay that stays as it is draws nothing more
        if owed.replace(false) {
            cx.request_animation_frame();
        }
    }

    fn semantics(&self, _: &std::cell::Cell<bool>, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
        cx.name(OVERLAY_TITLE);
        cx.description(match self.view.style {
            Style::Simple => "Drag to capture an area, or use the arrow keys and press Return. Space picks a window. Escape cancels.",
            Style::Interactive => "Drag to select an area, or use the arrow keys; Alt with an arrow resizes it. Space picks a window. Return captures. Escape cancels.",
        });
        cx.value(AccessibleValue::Text {
            value: self.view.summary(),
            editing: None,
        });
        if self.view.keys {
            cx.focusable();
            cx.initial_focus();
            cx.action(Action::Focus);
        }
        cx.action(Action::Click);
    }
}

fn spec(title: &str, app_id: String, size: Size, input_region: Option<Vec<Rect>>) -> WindowSpec {
    WindowSpec {
        title: title.to_owned(),
        app_id,
        size,
        min_size: None,
        max_size: None,
        decorations: DecorationMode::None,
        foreign_parent: None,
        input_region,
    }
}

/// The overlay window of output `index`; it takes the size of its output.
pub fn overlay<M: Clone + 'static>(
    index: usize,
    view: OverlayView,
    to: fn(Input) -> M,
) -> Window<M> {
    Window {
        key: WindowKey(NodeKey::from(format!("overlay/{index}"))),
        spec: spec(OVERLAY_TITLE, overlay_app_id(index), Size::ZERO, None),
        shortcuts: CommandRegistry::new(),
        on_close: Some(to(Input::Closed)),
        background: WindowBackground::None,
        content: Element::new(Overlay { view, to }),
        popup: None,
        menus: None,
    }
}

/// Turns the keys nothing in the toolbar took (Escape, Space, Return and the arrows while no control has the focus)
/// into [`Input::Key`], so the toolbar keeps the overlay's keys while it holds the keyboard.
struct Keys<M> {
    children: Vec<Element<M>>,
    to: fn(Input) -> M,
}

impl<M: 'static> Widget<M> for Keys<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Overlay)
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
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Pointer(PointerEvent::Button { mods, .. }, _)
                if cx.phase() == RoutePhase::Capture =>
            {
                cx.emit((self.to)(Input::Control(mods.contains(Mods::CTRL))));
                Propagation::Continue
            }
            Event::Key(key, _) if key.pressed && cx.phase() != RoutePhase::Capture => {
                match key_of(key) {
                    Some(k) => {
                        cx.emit((self.to)(Input::Key(k, mods(key.mods))));
                        Propagation::Stop
                    }
                    None => Propagation::Continue,
                }
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Toolbar);
        cx.name(TOOLBAR_TITLE);
    }
}

/// What the toolbar shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolbarView {
    pub mode: Mode,
    /// Seconds left on the timer, shown on the Capture button while it runs.
    pub countdown: Option<u32>,
}

/// The toolbar window: close, the three modes, Options and Capture, on the heads-up material.
pub fn toolbar<M: Clone + 'static>(view: ToolbarView, to: fn(Input) -> M) -> Window<M> {
    let counting = view.countdown.is_some();
    let selected = MODES.iter().position(|(m, _)| *m == view.mode);
    let children = vec![
        Element::new(Button {
            label: "Close".into(),
            role: ButtonRole::Normal,
            style: ButtonStyle::Borderless,
            enabled: true,
            busy: false,
            on_press: Some(to(Input::Cancel)),
        })
        .key("close"),
        Element::new(SegmentedControl {
            label: "Capture Mode".into(),
            segments: MODES.iter().map(|(_, label)| (*label).to_owned()).collect(),
            selected,
            enabled: !counting,
            on_select: Some(Box::new(move |i| to(Input::Mode(MODES[i].0)))),
        })
        .key("modes"),
        Element::new(Spacer { min: 0.0 }),
        Element::new(Button {
            label: "Options".into(),
            role: ButtonRole::Normal,
            style: ButtonStyle::Borderless,
            enabled: !counting,
            busy: false,
            on_press: Some(to(Input::Options(true))),
        })
        .key(OPTIONS_KEY),
        Element::new(Button {
            label: view
                .countdown
                .map_or_else(|| "Capture".to_owned(), |n| n.to_string()),
            role: ButtonRole::Default,
            style: ButtonStyle::Bordered,
            enabled: !counting,
            busy: false,
            on_press: Some(to(Input::Capture)),
        })
        .key("capture"),
    ];
    let size = Size::new(TOOLBAR_SIZE.0, TOOLBAR_SIZE.1);
    Window {
        key: WindowKey(NodeKey::from("toolbar")),
        spec: spec(TOOLBAR_TITLE, TOOLBAR_APP_ID.to_owned(), size, None),
        shortcuts: CommandRegistry::new(),
        on_close: Some(to(Input::Closed)),
        background: WindowBackground::Surface {
            surface: Surface::Hud,
            radius: TOOLBAR_RADIUS,
            alpha: 1.0,
        },
        content: Element::new(Keys {
            children: vec![Element::new(Stack {
                axis: Axis::Horizontal,
                padding: Insets::symmetric(TOOLBAR_PADDING, 0.0),
                spacing: TOOLBAR_SPACING,
                align: Align::Center,
                children,
            })
            .fill()],
            to,
        }),
        popup: None,
        menus: None,
    }
}

/// The Options menu's windows, below (or, at the bottom of the screen, above) the Options button. Section titles are
/// rows that cannot be chosen.
pub fn options_menu<M: Clone + 'static>(settings: &Settings, to: fn(Input) -> M) -> Vec<Window<M>> {
    let items = toolbar::menu(settings)
        .into_iter()
        .map(|row| match row {
            MenuRow::Header(title) => MenuItem::Item {
                label: title.to_owned(),
                enabled: false,
                checked: false,
                on_select: to(Input::Options(false)),
            },
            MenuRow::Choice {
                label,
                action,
                checked,
            } => MenuItem::Item {
                label,
                enabled: true,
                checked,
                on_select: to(Input::Choose(action)),
            },
            MenuRow::Separator => MenuItem::Separator,
        })
        .collect();
    Menu {
        parent: WindowKey(NodeKey::from("toolbar")),
        anchor: MenuAnchor::Below(NodeKey::from(OPTIONS_KEY)),
        label: "Options".into(),
        items,
        current: None,
        open: Vec::new(),
        on_open: Box::new(move |_| to(Input::Options(true))),
        on_close: to(Input::Options(false)),
    }
    .windows(WindowKey(NodeKey::from("options-menu")))
}

/// The thumbnail's picture: its pixels, named for good by `id`.
#[derive(Clone, Debug, PartialEq)]
pub struct ThumbnailImage {
    pub id: ResourceId,
    pub image: ImageResource,
}

impl ThumbnailImage {
    /// The premultiplied RGBA pixels of a `width` x `height` image, as `tiny-skia` keeps them.
    pub fn new(id: ResourceId, width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self {
            id,
            image: ImageResource {
                revision: 1,
                width,
                height,
                stride: width * 4,
                format: PixelFormat::Rgba8Premultiplied,
                pixels: pixels.into(),
            },
        }
    }
}

/// What the thumbnail window shows: the picture at `rect` (surface points, inside the room for its shadow), moved and
/// faded by `look`.
#[derive(Clone, Debug, PartialEq)]
pub struct ThumbnailView {
    pub image: ThumbnailImage,
    pub rect: Rect,
    pub look: Look,
}

struct ThumbnailWidget<M> {
    view: ThumbnailView,
    to: fn(Input) -> M,
}

impl<M: 'static> Widget<M> for ThumbnailWidget<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        fill_layout()
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.view == old.view {
            Change::None
        } else {
            Change::Paint
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let input = match event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    position,
                    ..
                },
                _,
            ) => {
                cx.capture_pointer();
                ThumbnailInput::Press(*position)
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) => {
                cx.release_pointer();
                ThumbnailInput::Release
            }
            Event::Pointer(PointerEvent::Motion { position }, _) => {
                ThumbnailInput::Motion(*position)
            }
            Event::Action(request) if request.action == Action::Click => ThumbnailInput::Activate,
            _ => return Propagation::Continue,
        };
        cx.emit((self.to)(Input::Thumbnail(input)));
        Propagation::Stop
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let v = &self.view;
        let shadow = Effect::DropShadow {
            offset: Point::ZERO,
            blur: THUMBNAIL_SHADOW_BLUR,
            color: Color::new(0.0, 0.0, 0.0, THUMBNAIL_SHADOW_ALPHA),
        };
        cx.set_group(v.look.alpha.clamp(0.0, 1.0) as f32, vec![shadow]);
        let rect = v.rect.translate(v.look.offset, 0.0);
        let shape = Shape::RoundedRect {
            rect,
            radius: THUMBNAIL_RADIUS,
        };
        cx.push(PaintOp::PushClip(shape.clone()));
        cx.image(v.image.id, &v.image.image, rect, Sampling::Linear);
        cx.push(PaintOp::PopClip);
        cx.push(PaintOp::Stroke {
            shape,
            width: THUMBNAIL_BORDER,
            paint: Paint::Solid(color(THUMBNAIL_BORDER_COLOR)),
        });
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(THUMBNAIL_TITLE);
        cx.description("Saves the screenshot now; it is saved by itself after a few seconds.");
        cx.action(Action::Click);
    }
}

/// The thumbnail window of `size` points (the picture and the room for its shadow); it takes the pointer over the
/// picture only.
pub fn thumbnail<M: Clone + 'static>(
    view: ThumbnailView,
    size: Size,
    to: fn(Input) -> M,
) -> Window<M> {
    let region = vec![view.rect];
    Window {
        key: WindowKey(NodeKey::from("thumbnail")),
        spec: spec(
            THUMBNAIL_TITLE,
            THUMBNAIL_APP_ID.to_owned(),
            size,
            Some(region),
        ),
        shortcuts: CommandRegistry::new(),
        on_close: Some(to(Input::Closed)),
        background: WindowBackground::None,
        content: Element::new(ThumbnailWidget { view, to }),
        popup: None,
        menus: None,
    }
}

/// Draws nothing and takes no pointer.
struct Blank;

impl<M: 'static> Widget<M> for Blank {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        fill_layout()
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    /// Nothing to reach: the pixel only holds the keyboard for the moment the capture goes on the clipboard.
    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}

/// The transparent 1 x 1 window that takes the keyboard while the capture is offered on the clipboard: the compositor
/// takes a selection only from a client with the keyboard focus.
pub fn clipboard_focus<M: Clone + 'static>(to: fn(Input) -> M) -> Window<M> {
    Window {
        key: WindowKey(NodeKey::from("clipboard")),
        spec: spec(
            OVERLAY_TITLE,
            CLIPBOARD_APP_ID.to_owned(),
            Size::new(1.0, 1.0),
            Some(Vec::new()),
        ),
        shortcuts: CommandRegistry::new(),
        on_close: Some(to(Input::Closed)),
        background: WindowBackground::None,
        content: Element::new(Blank),
        popup: None,
        menus: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_overlay_names_its_output_in_its_app_id() {
        assert_eq!(overlay_app_id(3), "cupertino-screenshot.overlay/3");
        assert_eq!(overlay_index("cupertino-screenshot.overlay/3"), Some(3));
        assert_eq!(overlay_index(TOOLBAR_APP_ID), None);
        assert_eq!(overlay_index("cupertino-screenshot.overlay"), None);
    }

    #[test]
    fn the_summary_says_what_a_capture_takes() {
        let session = Session::new(
            Style::Interactive,
            Mode::Selection,
            vec![Rect::new(0.0, 0.0, 800.0, 600.0)],
            Some(Rect::new(10.0, 20.0, 300.0, 200.0)),
            0,
        );
        let view = OverlayView::of(&session, Rect::new(0.0, 0.0, 800.0, 600.0), true, false);
        assert_eq!(view.summary(), "Selection 300 \u{d7} 200 at 10, 20");
        let leaving = OverlayView::of(&session, Rect::new(0.0, 0.0, 800.0, 600.0), true, true);
        assert_eq!(
            leaving.look,
            Pointer::Arrow,
            "an overlay that goes shows the arrow"
        );
    }
}
