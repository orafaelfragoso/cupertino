//! The bar and its open menus as CupertinoKit windows. A bar is one window along the top of its output with its slots
//! where the controller laid them out; each open menu level is a popup of the level before it (the first of the bar)
//! at the rectangle the controller gave it. The windows draw and describe what the controller holds and send the
//! pointer and the keys back as [`Input`], in the coordinates of the bar.

use crate::controller::{Level, MenuController};
use crate::fallback::HELP_SEARCH;
use crate::feed::{Shared, StatusIcon, SCREEN_CAPTURE_SLOT};
use crate::geom::{self, Point as BarPoint};
use crate::item::{ShortcutGlyphs, Toggle};
use crate::layout::{
    BarMetrics, MenuMetrics, PopupRow, Slot, SlotKey, TextMeasure, PRIVACY_DOT_PT, STATUS_ICON_PT,
};
use crate::popups;
use crate::tone::{BarTone, Halo, MenuPalette, Rgba};
use cupertino_kit::core::{
    next_resource_id, AccessibleStates, AccessibleValue, Action, Change, Checked, CommandRegistry,
    Element, Environment, Event, EventCx, IconImage, Key, MeasureCx, PaintCx, Phase, Popup,
    PopupAnchor, Propagation, Role, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, Overflow, SizeSpec};
use cupertino_kit::platform::{DecorationMode, KeyEvent, MouseButton, PointerEvent, WindowSpec};
use cupertino_kit::render::{
    Effect, FillRule, ImageResource, Paint, PaintOp, PathVerb, PixelFormat, Sampling, Shape,
};
use cupertino_kit::text::{
    AttributedText, FontWeight, ParagraphLayout, ParagraphStyle, TextStyle, TextSystem,
};
use cupertino_kit::theme::{Appearance, Color, Surface};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};
use tiny_skia::Pixmap;

/// The `app_id` of a bar's window is this followed by the bar's number; the roles give it its layer surface.
pub const BAR_APP_ID: &str = "cupertino-menubar.bar-";
/// What assistive technology calls a bar.
const BAR_TITLE: &str = "Menu Bar";
/// Corner radius of a popup menu in points. approximate.
pub const POPUP_RADIUS: f64 = 8.0;
/// Edge of the system menu's mark in points: the grid it is drawn on, so its strokes fall on whole pixels.
const SYSTEM_MARK_PT: f64 = 16.0;
/// Opacity factor of a row's shortcut and of its secondary text. approximate.
const FADED_ALPHA: f32 = 0.65;
/// Opacity factor of a status item's dot, drawn when it has no icon. approximate.
const DOT_ALPHA: f32 = 0.7;
/// Opacity factor of an item symbol whose radio is off or has no link. approximate (the design does not say).
const DIMMED_ALPHA: f32 = 0.4;
/// Inset of the search field's box in its row, vertically, in points. approximate.
const FIELD_INSET_Y: f64 = 2.0;
/// Corner radius of the search field's box in points. approximate.
const FIELD_RADIUS: f64 = 5.0;
/// Space between the search field's box and its text in points. approximate.
const FIELD_TEXT_INSET: f64 = 6.0;
/// Space between the search field's box and the chord after it in points. approximate.
const FIELD_CHORD_GAP: f64 = 8.0;
/// Width of the search field's caret in points. approximate.
const CARET_WIDTH: f64 = 1.0;
/// Opacity factor of the row highlight colour behind the field's selected text. approximate.
const FIELD_SELECTION_ALPHA: f32 = 0.35;
/// Inset of a row's highlight from the menu's sides, and its corner radius, in points. approximate.
const ROW_HIGHLIGHT_INSET: f64 = 4.0;
const ROW_HIGHLIGHT_RADIUS: f64 = 4.0;
/// Stroke widths of the check mark and the submenu chevron in points. approximate.
const CHECK_STROKE: f64 = 1.6;
const CHEVRON_STROKE: f64 = 1.4;
/// Radius of a radio row's dot in points. approximate.
const RADIO_DOT_RADIUS: f64 = 2.4;

/// One output's bar, numbered by the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BarId(pub u64);

impl BarId {
    /// The bar whose window was opened with `app_id`.
    pub fn of(app_id: &str) -> Option<BarId> {
        app_id.strip_prefix(BAR_APP_ID)?.parse().ok().map(BarId)
    }

    pub fn window(self) -> WindowKey {
        WindowKey(Key::from(format!("bar-{}", self.0)))
    }

    fn menu(self, level: usize) -> WindowKey {
        WindowKey(Key::from(format!("menu-{}-{level}", self.0)))
    }
}

/// What the pointer did, at a point of the bar's coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BarPointer {
    Move(BarPoint),
    Press(BarPoint),
    Release(BarPoint),
    /// Assistive technology clicked the slot or the row around the point.
    Click(BarPoint),
}

/// What the user did on a bar or its menus.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Pointer {
        bar: BarId,
        pointer: BarPointer,
    },
    Key {
        bar: BarId,
        key: KeyEvent,
    },
    /// The compositor dismissed the bar's menus.
    Dismissed(BarId),
    /// The compositor closed the bar's window.
    Closed(BarId),
}

/// The type of the bar and its menus: the regular items' and the bold application name's weights at one size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarFont {
    pub size: f32,
    pub regular: u16,
    pub bold: u16,
    /// Cap height in em: the text is centred on it.
    pub cap_height: f64,
}

impl BarFont {
    pub fn new(typography: &cupertino_theme::Typography) -> Self {
        let (regular, bold) = (typography.menu_bar_item(), typography.menu_bar_app_name());
        Self {
            size: regular.size as f32,
            regular: regular.weight,
            bold: bold.weight,
            cap_height: typography.metrics.cap_height / typography.metrics.units_per_em,
        }
    }

    fn layout(&self, text: &TextSystem, s: &str, bold: bool) -> Option<ParagraphLayout> {
        if s.is_empty() {
            return None;
        }
        let style = TextStyle {
            weight: FontWeight(if bold { self.bold } else { self.regular }),
            ..TextStyle::new(self.size)
        };
        let paragraph = text
            .paragraph(&AttributedText::plain(s, style), &ParagraphStyle::default())
            .ok()?;
        Some(paragraph.layout(None))
    }
}

/// Measures the bar's and the menus' text with the text system the windows draw it with.
pub struct KitMeasure {
    pub text: Arc<TextSystem>,
    pub font: BarFont,
}

impl TextMeasure for KitMeasure {
    fn width(&self, text: &str, bold: bool) -> f64 {
        self.font
            .layout(&self.text, text, bold)
            .map_or(0.0, |l| l.size().width)
    }
}

/// What a bar's windows show.
pub struct BarScene<'a> {
    pub bar: BarId,
    pub controller: &'a MenuController,
    pub shared: &'a Shared,
    pub font: BarFont,
    pub tone: BarTone,
    /// Whether the bar draws its solid strip ([`crate::tone::solid_bar`] of the session's display options): then no
    /// item draws a halo, from the same source.
    pub solid: bool,
    /// How unevenly light the wallpaper is under the bar, band by band across it, from the server; empty while not
    /// known.
    pub spread: &'a [u16],
    /// The bar's size: the output's width and the bar's height.
    pub size: Size,
}

/// The bar's window and one popup per open menu level.
pub fn windows<M: Clone + 'static>(scene: &BarScene<'_>, input: fn(Input) -> M) -> Vec<Window<M>> {
    let mut windows = vec![bar(scene, input)];
    let levels = scene.controller.levels();
    let rects: Vec<geom::Rect> = levels.iter().map(|l| popups::snap(l.layout.rect)).collect();
    for (level, at) in popups::relative_to_parents(&rects).into_iter().enumerate() {
        let parent = match level {
            0 => scene.bar.window(),
            _ => scene.bar.menu(level - 1),
        };
        windows.push(menu(scene, level, rects[level], at, parent, input));
    }
    windows
}

fn spec(app_id: String, size: Size) -> WindowSpec {
    WindowSpec {
        title: BAR_TITLE.to_owned(),
        app_id,
        size,
        min_size: None,
        max_size: None,
        decorations: DecorationMode::None,
        foreign_parent: None,
        input_region: None,
    }
}

fn bar<M: Clone + 'static>(scene: &BarScene<'_>, input: fn(Input) -> M) -> Window<M> {
    let controller = scene.controller;
    let highlight = controller.highlighted_slot();
    let open = controller.open_slot();
    let mut places = 0;
    let children: Vec<_> = controller
        .layout()
        .slots
        .iter()
        .map(|slot| {
            let (role, name) = slot_semantics(slot, scene);
            let place = (role == Role::MenuItem).then(|| {
                places += 1;
                places - 1
            });
            Element::new(SlotNode {
                place,
                bar: scene.bar,
                slot: slot.clone(),
                look: slot_look(slot, scene),
                highlighted: highlight == Some(&slot.key),
                expanded: has_menu(&slot.key, scene).then(|| open == Some(&slot.key)),
                role,
                name,
                metrics: controller.bar_metrics.clone(),
                font: scene.font,
                tone: scene.tone,
                halo: crate::tone::halo(
                    crate::tone::spread_under(
                        scene.spread,
                        scene.size.width,
                        slot.rect.x,
                        slot.rect.x + slot.rect.w,
                    ),
                    scene.solid,
                ),
                input,
            })
        })
        .collect();
    Window {
        key: scene.bar.window(),
        spec: spec(format!("{BAR_APP_ID}{}", scene.bar.0), scene.size),
        shortcuts: CommandRegistry::new(),
        on_close: Some(input(Input::Closed(scene.bar))),
        background: WindowBackground::None,
        content: Element::new(Root {
            bar: scene.bar,
            origin: BarPoint::new(0.0, 0.0),
            size: None,
            role: Role::MenuBar,
            name: BAR_TITLE.to_owned(),
            items: places,
            input,
            children,
        }),
        popup: None,
        menus: None,
    }
}

/// Whether the slot opens a menu.
fn has_menu(key: &SlotKey, scene: &BarScene<'_>) -> bool {
    let content = scene.controller.content();
    match key {
        SlotKey::System | SlotKey::AppMenu | SlotKey::Menu(_) => true,
        SlotKey::Status(id) => content
            .status
            .iter()
            .any(|s| &s.id == id && s.menu.is_some()),
        SlotKey::ControlPanel | SlotKey::Clock => false,
    }
}

fn slot_semantics(slot: &Slot, scene: &BarScene<'_>) -> (Role, String) {
    let content = scene.controller.content();
    let item = if has_menu(&slot.key, scene) {
        Role::MenuItem
    } else {
        Role::Button
    };
    let name = match &slot.key {
        SlotKey::System => "System".to_owned(),
        SlotKey::AppMenu => content.app_name.clone(),
        SlotKey::Menu(id) => content
            .menus
            .iter()
            .find(|m| m.id == *id)
            .map(|m| m.label.clone())
            .unwrap_or_default(),
        SlotKey::Status(id) if id == SCREEN_CAPTURE_SLOT => {
            return (Role::Image, "Screen Capture".to_owned())
        }
        SlotKey::Status(id) => content
            .status
            .iter()
            .find(|s| &s.id == id)
            .and_then(|s| s.menu.as_ref())
            .map(|m| m.label.clone())
            .unwrap_or_default(),
        SlotKey::ControlPanel => "Control Panel".to_owned(),
        SlotKey::Clock => content.clock.clone(),
    };
    (item, name)
}

/// What a slot of the bar draws.
#[derive(Clone, Debug, PartialEq)]
enum SlotLook {
    SystemMark,
    Text {
        text: String,
        padding: f64,
        bold: bool,
    },
    PrivacyDot,
    Image(IconImage),
    Symbol {
        name: &'static str,
        dimmed: bool,
    },
    Dot,
    ControlPanel,
    Nothing,
}

fn slot_look(slot: &Slot, scene: &BarScene<'_>) -> SlotLook {
    let content = scene.controller.content();
    let metrics = &scene.controller.bar_metrics;
    match &slot.key {
        SlotKey::System => SlotLook::SystemMark,
        SlotKey::AppMenu => SlotLook::Text {
            text: content.app_name.clone(),
            padding: metrics.title_padding,
            bold: true,
        },
        SlotKey::Menu(id) => match content.menus.iter().find(|m| m.id == *id) {
            Some(m) => SlotLook::Text {
                text: m.label.clone(),
                padding: metrics.title_padding,
                bold: false,
            },
            None => SlotLook::Nothing,
        },
        SlotKey::Status(id) if id == SCREEN_CAPTURE_SLOT => SlotLook::PrivacyDot,
        SlotKey::Status(id) => match scene.shared.status_icon(id) {
            Some(StatusIcon::Image(image)) => SlotLook::Image(image.clone()),
            Some(StatusIcon::Symbol(glyph)) => SlotLook::Symbol {
                name: glyph.symbol,
                dimmed: glyph.dimmed,
            },
            Some(StatusIcon::Dot) => SlotLook::Dot,
            None => SlotLook::Nothing,
        },
        SlotKey::ControlPanel => SlotLook::ControlPanel,
        SlotKey::Clock => SlotLook::Text {
            text: content.clock.clone(),
            padding: metrics.clock_padding,
            bold: false,
        },
    }
}

fn menu<M: Clone + 'static>(
    scene: &BarScene<'_>,
    level: usize,
    rect: geom::Rect,
    at: geom::Rect,
    parent: WindowKey,
    input: fn(Input) -> M,
) -> Window<M> {
    let levels = scene.controller.levels();
    let open: &Level = &levels[level];
    let next = levels.get(level + 1).map(|l| l.menu.id);
    let layout = &open.layout;
    let metrics = scene.controller.menu_metrics.clone();
    let any_mark = layout.rows.iter().any(|r| r.item.toggle != Toggle::None);
    let any_icon = layout
        .rows
        .iter()
        .any(|r| r.item.icon_name.is_some() || r.item.icon_data.is_some());
    let field = scene.controller.search_field().map(|f| Field {
        text: f.text().to_owned(),
        caret: f.caret(),
        selection: f.selection(),
    });
    let mut places = 0;
    let children: Vec<_> = layout
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let searching = field.is_some() && row.item.id == HELP_SEARCH;
            let place = (!row.item.separator && !searching).then(|| {
                places += 1;
                places - 1
            });
            Element::new(RowNode {
                place,
                bar: scene.bar,
                row: row.clone(),
                origin: BarPoint::new(rect.x, rect.y),
                hot: open.selected == Some(i) && row.item.enabled,
                expanded: row.item.has_submenu.then(|| next == Some(row.item.id)),
                text_x: metrics.padding_x
                    + if any_mark { metrics.mark_column } else { 0.0 }
                    + if any_icon { metrics.icon_column } else { 0.0 },
                key_width: layout.shortcut_key_width,
                field: field.clone().filter(|_| row.item.id == HELP_SEARCH),
                metrics: metrics.clone(),
                font: scene.font,
                input,
            })
        })
        .collect();
    let size = Size::new(rect.w, rect.h);
    Window {
        key: scene.bar.menu(level),
        spec: spec(BAR_APP_ID.to_owned(), size),
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background: WindowBackground::Surface {
            surface: Surface::Menu,
            radius: POPUP_RADIUS,
            alpha: 1.0,
        },
        content: Element::new(Root {
            bar: scene.bar,
            origin: BarPoint::new(rect.x, rect.y),
            size: Some(size),
            role: Role::Menu,
            name: open.menu.label.clone(),
            items: places,
            input,
            children,
        }),
        popup: Some(Popup {
            parent,
            // `At` hangs the popup off the far corner of the point's one-point square
            anchor: PopupAnchor::At(Point::new(at.x - 1.0, at.y - 1.0)),
            grab: true,
            on_dismiss: input(Input::Dismissed(scene.bar)),
        }),
        menus: None,
    }
}

/// The Help menu's search field as the controller holds it.
#[derive(Clone, Debug, PartialEq)]
struct Field {
    text: String,
    caret: usize,
    selection: Range<usize>,
}

/// Whether the session's appearance is dark, which picks the palette of the menus and of a bar without its own tone.
/// The colours of a popup's rows: the appearance's, strengthened with Increase Contrast.
fn popup_palette(env: &Environment) -> MenuPalette {
    let palette = MenuPalette::new(dark(env));
    if env.theme.preferences.appearance.is_high_contrast() {
        palette.high_contrast()
    } else {
        palette
    }
}

fn dark(env: &Environment) -> bool {
    matches!(
        env.theme.preferences.appearance,
        Appearance::Dark | Appearance::DarkHighContrast
    )
}

fn color(c: Rgba) -> Color {
    Color::new(c[0], c[1], c[2], c[3])
}

fn faded(c: Rgba, alpha: f32) -> Rgba {
    [c[0], c[1], c[2], c[3] * alpha]
}

fn fill(cx: &mut PaintCx<'_>, rect: Rect, radius: f64, c: Rgba) {
    let shape = if radius > 0.0 {
        Shape::RoundedRect { rect, radius }
    } else {
        Shape::Rect(rect)
    };
    cx.push(PaintOp::Fill {
        shape,
        paint: Paint::Solid(color(c)),
    });
}

fn circle(cx: &mut PaintCx<'_>, centre: Point, radius: f64, c: Rgba) {
    let rect = Rect::new(
        centre.x - radius,
        centre.y - radius,
        2.0 * radius,
        2.0 * radius,
    );
    fill(cx, rect, radius, c);
}

fn polyline(cx: &mut PaintCx<'_>, points: &[Point], width: f64, c: Rgba) {
    let mut verbs = Vec::with_capacity(points.len());
    for (i, &p) in points.iter().enumerate() {
        verbs.push(if i == 0 {
            PathVerb::MoveTo(p)
        } else {
            PathVerb::LineTo(p)
        });
    }
    cx.push(PaintOp::Stroke {
        shape: Shape::Path {
            verbs,
            fill_rule: FillRule::NonZero,
        },
        width,
        paint: Paint::Solid(color(c)),
    });
}

/// `text` with its left edge at `x`, centred on its cap height in the band `top..top + height`.
#[allow(clippy::too_many_arguments)]
fn text(
    cx: &mut PaintCx<'_>,
    font: &BarFont,
    s: &str,
    x: f64,
    top: f64,
    height: f64,
    bold: bool,
    c: Rgba,
) {
    let system = Arc::clone(&cx.env().text);
    let Some(layout) = font.layout(&system, s, bold) else {
        return;
    };
    let baseline = top + height / 2.0 + font.cap_height * f64::from(font.size) / 2.0;
    let first = layout.lines().first().map_or(0.0, |line| line.baseline);
    let origin = Point::new(x, baseline - first);
    for line in layout.lines() {
        for run in &line.runs {
            cx.push(PaintOp::Glyphs {
                origin: origin + run.origin,
                run: run.run.clone(),
                color: color(c),
            });
        }
    }
}

fn width(cx: &PaintCx<'_>, font: &BarFont, s: &str) -> f64 {
    font.layout(&cx.env().text, s, false)
        .map_or(0.0, |l| l.size().width)
}

/// `image` centred at `centre`, scaled down to fit `side` points square.
fn image(cx: &mut PaintCx<'_>, image: &IconImage, centre: Point, side: f64) {
    let scale = cx.env().scale;
    let (w, h) = (f64::from(image.image.width), f64::from(image.image.height));
    let k = (side * scale / w.max(h)).min(1.0 + 1e-3) / scale;
    let dst = Rect::new(centre.x - w * k / 2.0, centre.y - h * k / 2.0, w * k, h * k);
    cx.image(image.id, &image.image, dst, Sampling::Linear);
}

/// `pixmap` as an image the windows draw; each call names new pixels.
pub fn icon_image(pixmap: &Pixmap) -> IconImage {
    IconImage {
        id: next_resource_id(),
        image: ImageResource {
            revision: 1,
            width: pixmap.width(),
            height: pixmap.height(),
            stride: pixmap.width() * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: Arc::from(pixmap.data()),
        },
    }
}

/// The system menu's mark at `px` device pixels in `c`, drawn once per size and colour.
fn system_mark(px: u32, c: Rgba) -> IconImage {
    type Marks = Mutex<HashMap<(u32, [u8; 4]), IconImage>>;
    static MARKS: OnceLock<Marks> = OnceLock::new();
    let key = (px, c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8));
    let mut marks = MARKS
        .get_or_init(Marks::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    marks
        .entry(key)
        .or_insert_with(|| icon_image(&cupertino_icons::system_mark(px, c)))
        .clone()
}

/// The root of a bar's window or of one of its menus: it takes the pointer and the keys for the controller before any
/// node below it, and lays its children out where the controller put them.
struct Root<M> {
    bar: BarId,
    /// The window's top-left corner in the bar's coordinates.
    origin: BarPoint,
    /// A menu's size; a bar fills its window.
    size: Option<Size>,
    role: Role,
    name: String,
    /// How many items the bar or menu holds, which a screen reader reads with each item's place.
    items: usize,
    input: fn(Input) -> M,
    children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for Root<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        let (width, height) = match self.size {
            Some(size) => (SizeSpec::Fixed(size.width), SizeSpec::Fixed(size.height)),
            None => (SizeSpec::Fill, SizeSpec::Fill),
        };
        LayoutSpec {
            width,
            height,
            overflow: Overflow::Clip,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.size != old.size {
            Change::Layout
        } else if self.name != old.name || self.items != old.items || self.origin != old.origin {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.size.unwrap_or(Size::ZERO),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let at = |p: Point| BarPoint::new(p.x + self.origin.x, p.y + self.origin.y);
        let input = match event {
            Event::Pointer(pointer, _) if cx.phase() == Phase::Capture => {
                let pointer = match *pointer {
                    PointerEvent::Enter { position } | PointerEvent::Motion { position } => {
                        BarPointer::Move(at(position))
                    }
                    PointerEvent::Button {
                        position,
                        button: MouseButton::Left,
                        pressed,
                        ..
                    } => {
                        if pressed {
                            BarPointer::Press(at(position))
                        } else {
                            BarPointer::Release(at(position))
                        }
                    }
                    // the controller keeps its selection until the pointer shows up elsewhere
                    _ => return Propagation::Stop,
                };
                Input::Pointer {
                    bar: self.bar,
                    pointer,
                }
            }
            Event::Key(key, _) => Input::Key {
                bar: self.bar,
                key: *key,
            },
            _ => return Propagation::Continue,
        };
        cx.emit((self.input)(input));
        Propagation::Stop
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(self.role);
        cx.name(self.name.clone());
        cx.item_count(self.items);
    }
}

/// Places a leaf at `rect` of its absolute parent.
fn placed(rect: Rect) -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fixed(rect.width()),
        height: SizeSpec::Fixed(rect.height()),
        position: rect.origin,
        ..LayoutSpec::new(ContainerKind::Leaf)
    }
}

fn kit_rect(r: geom::Rect) -> Rect {
    Rect::new(r.x, r.y, r.w, r.h)
}

fn centre(r: geom::Rect) -> BarPoint {
    BarPoint::new(r.x + r.w / 2.0, r.y + r.h / 2.0)
}

/// Assistive technology's click on a slot or a row: the pointer pressed and released at its centre.
fn clicked<M>(
    event: &Event,
    cx: &mut EventCx<'_, M>,
    bar: BarId,
    at: BarPoint,
    input: fn(Input) -> M,
) -> Propagation {
    match event {
        Event::Action(request) if request.action == Action::Click => {
            cx.emit(input(Input::Pointer {
                bar,
                pointer: BarPointer::Click(at),
            }));
            Propagation::Stop
        }
        _ => Propagation::Continue,
    }
}

/// One slot of a bar: its highlight and what it draws.
struct SlotNode<M> {
    /// The slot's place among the bar's menus, if it opens one.
    place: Option<usize>,
    bar: BarId,
    slot: Slot,
    look: SlotLook,
    highlighted: bool,
    /// Whether the slot's menu is open; `None` for a slot without a menu.
    expanded: Option<bool>,
    role: Role,
    name: String,
    metrics: BarMetrics,
    font: BarFont,
    tone: BarTone,
    /// The halo under the slot's text and symbols, for the wallpaper under it; `None` on a solid bar or a calm
    /// wallpaper.
    halo: Option<Halo>,
    input: fn(Input) -> M,
}

impl<M: Clone + 'static> Widget<M> for SlotNode<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        placed(kit_rect(self.slot.rect))
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.slot.rect != old.slot.rect {
            Change::Layout
        } else if self.look != old.look
            || self.highlighted != old.highlighted
            || self.expanded != old.expanded
            || self.name != old.name
            || self.place != old.place
            || self.tone != old.tone
            || self.halo != old.halo
            || self.font != old.font
            || self.metrics != old.metrics
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.slot.rect.w, self.slot.rect.h),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        clicked(event, cx, self.bar, centre(self.slot.rect), self.input)
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let appearance = MenuPalette::new(dark(cx.env()));
        let palette = self.tone.palette(appearance);
        let size = cx.size();
        let m = &self.metrics;
        if self.highlighted {
            let rect = Rect::new(
                0.0,
                m.selection_inset_y,
                size.width,
                size.height - 2.0 * m.selection_inset_y,
            );
            fill(cx, rect, m.selection_corner_radius, palette.bar_highlight);
        }
        // over a busy wallpaper the text and symbols keep a soft halo of the opposite tone, so they read where the
        // picture under them is as light or as dark as they are; a calm wallpaper and a solid bar need none
        // (over measured wallpaper a dense core hugs the strokes first, and the soft halo spreads from both)
        if let Some(halo) = self.halo {
            let core = halo
                .core_color(palette.text)
                .map(|core| Effect::DropShadow {
                    offset: Point::ZERO,
                    blur: crate::tone::HALO_CORE_BLUR,
                    color: color(core),
                });
            let soft = Effect::DropShadow {
                offset: Point::ZERO,
                blur: halo.blur,
                color: color(halo.color(palette.text)),
            };
            cx.push(PaintOp::PushLayer {
                opacity: 1.0,
                effects: core.into_iter().chain([soft]).collect(),
            });
        }
        self.paint_look(cx, &palette);
        if self.halo.is_some() {
            cx.push(PaintOp::PopLayer);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(self.role);
        cx.name(self.name.clone());
        if let Some(place) = self.place {
            cx.item_index(place);
        }
        cx.states(AccessibleStates {
            selected: self.highlighted,
            expanded: self.expanded,
            ..AccessibleStates::default()
        });
        if self.role != Role::Image {
            cx.action(Action::Click);
        }
    }
}

impl<M> SlotNode<M> {
    /// The slot's text, symbol or mark in `palette`.
    fn paint_look(&self, cx: &mut PaintCx<'_>, palette: &MenuPalette) {
        let size = cx.size();
        let middle = Point::new(size.width / 2.0, size.height / 2.0);
        let scale = cx.env().scale;
        match &self.look {
            SlotLook::SystemMark => {
                let px = (SYSTEM_MARK_PT * scale).round().max(1.0) as u32;
                let mark = system_mark(px, palette.text);
                let side = f64::from(mark.image.width) / scale;
                // on whole device pixels, so the strokes stay sharp
                let snap = |v: f64| (v * scale).round() / scale;
                let dst = Rect::new(
                    snap(middle.x - side / 2.0),
                    snap(middle.y - side / 2.0),
                    side,
                    f64::from(mark.image.height) / scale,
                );
                cx.image(mark.id, &mark.image, dst, Sampling::Nearest);
            }
            SlotLook::Text {
                text: s,
                padding,
                bold,
            } => text(
                cx,
                &self.font,
                s,
                *padding,
                0.0,
                size.height,
                *bold,
                palette.text,
            ),
            SlotLook::PrivacyDot => {
                circle(cx, middle, PRIVACY_DOT_PT / 2.0, palette.screen_capture)
            }
            SlotLook::Image(icon) => image(cx, icon, middle, STATUS_ICON_PT),
            SlotLook::Symbol { name, dimmed } => {
                let alpha = if *dimmed { DIMMED_ALPHA } else { 1.0 };
                let px = (STATUS_ICON_PT * scale).ceil().max(1.0) as u32;
                let icon =
                    cx.env()
                        .icons
                        .symbol(name, px, color(faded(palette.text, alpha)), false);
                image(cx, &icon, middle, STATUS_ICON_PT);
            }
            SlotLook::Dot => circle(
                cx,
                middle,
                STATUS_ICON_PT * 0.3,
                faded(palette.text, DOT_ALPHA),
            ),
            SlotLook::ControlPanel => {
                // two small switches
                for (dy, knob) in [(-3.2, 1.8), (3.2, -1.8)] {
                    let track = Rect::new(middle.x - 6.0, middle.y + dy - 2.2, 12.0, 4.4);
                    fill(cx, track, 2.2, faded(palette.text, 0.45));
                    let at = Point::new(middle.x + knob * 2.0, middle.y + dy);
                    circle(cx, at, 2.6, palette.text);
                }
            }
            SlotLook::Nothing => {}
        }
    }
}

/// One row of a menu.
struct RowNode<M> {
    /// The row's place among the menu's items, separators and the search field not counted.
    place: Option<usize>,
    bar: BarId,
    row: PopupRow,
    /// The menu's top-left corner in the bar's coordinates.
    origin: BarPoint,
    /// Highlighted: selected and enabled.
    hot: bool,
    /// Whether the row's submenu is open; `None` for a row without one.
    expanded: Option<bool>,
    /// Where the label starts, after the mark and icon columns the menu has.
    text_x: f64,
    /// The width of the column the shortcuts' keys stand in.
    key_width: f64,
    /// The search field, in the Help menu's search row.
    field: Option<Field>,
    metrics: MenuMetrics,
    font: BarFont,
    input: fn(Input) -> M,
}

impl<M> RowNode<M> {
    /// The shortcut at the right end of the row's `width`: the key in its column, the modifier glyphs right-aligned
    /// just before it. Returns where the modifier glyphs begin.
    fn shortcut(&self, cx: &mut PaintCx<'_>, sc: &ShortcutGlyphs, width: f64, c: Rgba) -> f64 {
        let height = cx.size().height;
        let key_x = width - self.metrics.padding_x - self.key_width;
        text(cx, &self.font, &sc.key, key_x, 0.0, height, false, c);
        let modifiers_x = key_x - self::width(cx, &self.font, &sc.modifiers);
        text(
            cx,
            &self.font,
            &sc.modifiers,
            modifiers_x,
            0.0,
            height,
            false,
            c,
        );
        modifiers_x
    }

    /// The search field: a box with the text, its selection and a steady caret, or the dimmed label while it is empty.
    /// The box ends before `chord_left`, where the row's chord begins.
    fn field(
        &self,
        cx: &mut PaintCx<'_>,
        field: &Field,
        palette: &MenuPalette,
        chord_left: Option<f64>,
    ) {
        let size = cx.size();
        let m = &self.metrics;
        let left = m.padding_x;
        let right = chord_left.map_or(size.width - m.padding_x, |x| x - FIELD_CHORD_GAP);
        let frame = Rect::new(
            left,
            FIELD_INSET_Y,
            right - left,
            size.height - 2.0 * FIELD_INSET_Y,
        );
        fill(cx, frame, FIELD_RADIUS, palette.separator);
        let x = frame.min_x() + FIELD_TEXT_INSET;
        if field.text.is_empty() {
            let dimmed = faded(palette.text, palette.disabled_alpha);
            text(
                cx,
                &self.font,
                &self.row.item.label,
                x,
                0.0,
                size.height,
                false,
                dimmed,
            );
        }
        let offset =
            |cx: &PaintCx<'_>, at: usize| x + self::width(cx, &self.font, &field.text[..at]);
        let (y, h) = (
            frame.min_y() + FIELD_INSET_Y,
            frame.height() - 2.0 * FIELD_INSET_Y,
        );
        if field.selection.is_empty() {
            let caret = Rect::new(offset(cx, field.caret), y, CARET_WIDTH, h);
            fill(cx, caret, 0.0, palette.text);
        } else {
            let (start, end) = (
                offset(cx, field.selection.start),
                offset(cx, field.selection.end),
            );
            let tint = faded(palette.row_highlight, FIELD_SELECTION_ALPHA);
            fill(cx, Rect::new(start, y, end - start, h), 0.0, tint);
        }
        text(
            cx,
            &self.font,
            &field.text,
            x,
            0.0,
            size.height,
            false,
            palette.text,
        );
    }
}

impl<M: Clone + 'static> Widget<M> for RowNode<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        let r = self.row.rect;
        placed(Rect::new(
            r.x - self.origin.x,
            r.y - self.origin.y,
            r.w,
            r.h,
        ))
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.row.rect != old.row.rect || self.origin != old.origin {
            Change::Layout
        } else if self.row != old.row
            || self.hot != old.hot
            || self.expanded != old.expanded
            || self.place != old.place
            || self.text_x != old.text_x
            || self.key_width != old.key_width
            || self.field != old.field
            || self.metrics != old.metrics
            || self.font != old.font
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.row.rect.w, self.row.rect.h),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if self.row.item.separator || !self.row.item.enabled {
            return Propagation::Continue;
        }
        clicked(event, cx, self.bar, centre(self.row.rect), self.input)
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let palette = popup_palette(cx.env());
        let size = cx.size();
        let m = &self.metrics;
        let item = &self.row.item;
        if let Some(field) = &self.field {
            let faded_text = faded(palette.text, FADED_ALPHA);
            let chord_left = self
                .row
                .shortcut
                .as_ref()
                .map(|sc| self.shortcut(cx, sc, size.width, faded_text));
            self.field(cx, field, &palette, chord_left);
            return;
        }
        if item.separator {
            let line = 1.0 / cx.env().scale.max(1.0);
            let rect = Rect::new(
                m.padding_x,
                size.height / 2.0 - 0.5,
                size.width - 2.0 * m.padding_x,
                line,
            );
            fill(cx, rect, 0.0, palette.rule);
            return;
        }
        if self.hot {
            let rect = Rect::new(
                ROW_HIGHLIGHT_INSET,
                0.0,
                size.width - 2.0 * ROW_HIGHLIGHT_INSET,
                size.height,
            );
            fill(cx, rect, ROW_HIGHLIGHT_RADIUS, palette.row_highlight);
        }
        let base = if self.hot {
            palette.row_highlight_text
        } else {
            palette.text
        };
        let c = faded(
            base,
            if item.enabled {
                1.0
            } else {
                palette.disabled_alpha
            },
        );
        let middle = size.height / 2.0;
        if item.toggle.is_marked() {
            let x = m.padding_x;
            match item.toggle {
                Toggle::Radio(_) => circle(
                    cx,
                    Point::new(x + m.mark_column * 0.35, middle),
                    RADIO_DOT_RADIUS,
                    c,
                ),
                _ => {
                    let x = x + m.mark_column * 0.3;
                    let check = [
                        Point::new(x - 4.0, middle),
                        Point::new(x - 1.2, middle + 3.2),
                        Point::new(x + 4.2, middle - 3.8),
                    ];
                    polyline(cx, &check, CHECK_STROKE, c);
                }
            }
        }
        text(
            cx,
            &self.font,
            &item.label,
            self.text_x,
            0.0,
            size.height,
            false,
            c,
        );
        if let Some(detail) = &self.row.detail {
            let x = self.text_x + width(cx, &self.font, &item.label) + m.detail_gap;
            text(
                cx,
                &self.font,
                detail,
                x,
                0.0,
                size.height,
                false,
                faded(c, FADED_ALPHA),
            );
        }
        if let Some(sc) = &self.row.shortcut {
            let room = if item.has_submenu {
                size.width - m.arrow_width
            } else {
                size.width
            };
            self.shortcut(cx, sc, room, faded(c, FADED_ALPHA));
        }
        if item.has_submenu {
            let x = size.width - m.padding_x - m.arrow_width * 0.35;
            let chevron = [
                Point::new(x - 1.8, middle - 3.6),
                Point::new(x + 1.8, middle),
                Point::new(x - 1.8, middle + 3.6),
            ];
            polyline(cx, &chevron, CHEVRON_STROKE, c);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        let item = &self.row.item;
        if item.separator {
            cx.role(Role::Separator);
            return;
        }
        if let Some(field) = &self.field {
            cx.role(Role::TextInput);
            cx.name(item.label.clone());
            cx.value(AccessibleValue::Text {
                value: field.text.clone(),
                editing: None,
            });
            cx.states(AccessibleStates {
                focused: true,
                ..AccessibleStates::default()
            });
            return;
        }
        let checked =
            |on: bool| AccessibleValue::Toggle(if on { Checked::True } else { Checked::False });
        match item.toggle {
            Toggle::None => cx.role(Role::MenuItem),
            Toggle::Check(on) => {
                cx.role(Role::MenuItemCheckBox);
                cx.value(checked(on));
            }
            Toggle::Indeterminate => {
                cx.role(Role::MenuItemCheckBox);
                cx.value(AccessibleValue::Toggle(Checked::Mixed));
            }
            Toggle::Radio(on) => {
                cx.role(Role::MenuItemRadio);
                cx.value(checked(on));
            }
        }
        cx.name(item.label.clone());
        if let Some(place) = self.place {
            cx.item_index(place);
        }
        if let Some(detail) = &self.row.detail {
            cx.description(detail.clone());
        }
        cx.states(AccessibleStates {
            disabled: !item.enabled,
            selected: self.hot,
            expanded: self.expanded,
            ..AccessibleStates::default()
        });
        if item.enabled {
            cx.action(Action::Click);
        }
    }
}
