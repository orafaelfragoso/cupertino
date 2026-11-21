//! The overview's surface on CupertinoKit: one window over the whole output, on an overlay layer surface anchored to
//! every edge, so the window's coordinates are the output's. Each window shows in a hole of the surface (a surface
//! layer the server draws the window in, [`window_layer`]); the desktops' pictures in the band are holes too
//! ([`workspace_layer`]). The chrome is one surface layer above them ([`CHROME_LAYER`]), so the component fades it
//! as one: the desktops band across the top (a tinted band with a soft shadow under it, the desktops' names with the
//! current one on a pill, the bare plus that adds a desktop, the ring round the current desktop's picture and the
//! remove button after a rest on a picture), and the ring round the selected window with its name on a plate in its
//! middle.
//!
//! The windows' layout is the model's ([`crate::layout`] on `cupertino_wm::overview`); where each hole is laid
//! out is the component's ([`Scene::holes`]), which moves the holes with layer animations in its transactions.
//! Assistive technology sees the windows and the desktops as buttons named by their titles and desktop names, with
//! the keyboard's selection as their focus. Nothing here speaks Wayland.

use crate::layout::{Mode, OverviewUi, UiAction};
use crate::parts::{
    self, centred_text, faded, fill, kit_rect, line, outline, Group, Look, Painter, Part,
    PointerInput, Rgba, Root,
};
use cupertino_kit::core::{
    AccessibleStates, CommandRegistry, Element, Key, PaintCx, Role, Window, WindowBackground,
    WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::platform::{DecorationMode, KeyEvent, WindowSpec};
use cupertino_kit::render::{GradientStop, Paint, PaintOp, Shape};
use cupertino_wm::geometry::{Point as WmPoint, Rect as WmRect};
use cupertino_wm::overview::bar::BarLayout;
use cupertino_wm::overview::Key as OverviewKey;

/// The `app_id` of the overview's window, and the namespace of its layer surface.
pub const APP_ID: &str = "cupertino-overview";

/// The surface layer of the chrome: the band, the rings and the name, which fade as one.
pub const CHROME_LAYER: u64 = 1;

/// The first layer key of the windows' holes and of the desktops' pictures: a window's or a desktop's id is added.
const WINDOW_LAYERS: u64 = 1 << 32;
const WORKSPACE_LAYERS: u64 = 2 << 32;

/// The corner radius of what a capture paints where the server would draw a window ([`Scene::hole_fill`]).
const HOLE_RADIUS: f64 = 10.0;

/// What assistive technology calls the chrome.
const TITLE: &str = "Overview";

/// The desktops band's tint over the desktop, and under Reduce Transparency or Increase Contrast.
const BAND: Rgba = [0.0, 0.0, 0.0, 0.05];
const BAND_OPAQUE: Rgba = [0.12, 0.12, 0.14, 1.0];
/// The shadow under the band: this tall, black from this opacity at the band's edge to none (44 pt).
const BAND_SHADOW: f64 = 44.0;
const BAND_SHADOW_ALPHA: f32 = 0.22;
/// The pill under the current desktop's name: a light veil, its corner radius and the space beside the name.
const NAME_PILL: Rgba = [1.0, 1.0, 1.0, 0.2];
const NAME_PILL_OPAQUE: Rgba = [0.35, 0.35, 0.38, 1.0];
const NAME_PILL_RADIUS: f64 = 5.0;
const NAME_PILL_SIDE: f64 = 11.5;
/// A desktop's name: size and weight.
const NAME_PT: f64 = 14.0;
const NAME_WEIGHT: u16 = 400;
/// Labels and glyphs over the band.
const GLYPH: Rgba = [1.0, 1.0, 1.0, 0.95];
/// The ring round the current desktop's picture, and round a picture a window would be dropped on: blue, 3 pt, with
/// corners of 4 pt.
const SELECTION: Rgba = [0.188, 0.514, 0.984, 1.0];
const PICTURE_RING_WIDTH: f64 = 3.0;
const PICTURE_RING_RADIUS: f64 = 4.0;
/// The plus of the add button: the length of its arms and its stroke width in points.
const PLUS_ARM: f64 = 10.0;
const PLUS_WIDTH: f64 = 2.0;
/// The remove button's fill, and the cross on it as a fraction of its diameter and its stroke width.
const REMOVE_FILL: Rgba = [0.82, 0.82, 0.84, 0.95];
const REMOVE_CROSS: Rgba = [0.2, 0.2, 0.22, 1.0];
const CROSS_FRACTION: f64 = 0.18;
const CROSS_WIDTH: f64 = 2.0;
/// The ring round the selected window: the same blue, this wide, this far outside the window, and the corner radius
/// of its outer edge.
const RING_WIDTH: f64 = 5.0;
const RING_GAP: f64 = 1.5;
const RING_RADIUS: f64 = 12.5;
/// The ring's width under Increase Contrast.
const CONTRAST_RING_WIDTH: f64 = 6.0;
/// The selected window's name: on a light plate in the window's middle, this tall, with round ends and this much space
/// beside the text, in dark text of this size and weight.
const LABEL_PLATE: Rgba = [1.0, 1.0, 1.0, 0.38];
const LABEL_PLATE_OPAQUE: Rgba = [0.95, 0.95, 0.96, 1.0];
const LABEL_HEIGHT: f64 = 28.0;
const LABEL_SIDE: f64 = 14.0;
const LABEL_PT: f64 = 16.0;
const LABEL_WEIGHT: u16 = 600;
const LABEL_TEXT: Rgba = [0.0, 0.0, 0.0, 0.92];
/// What a window without a title is called.
const UNTITLED: &str = "Untitled Window";

/// What reaches the overview from its window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverviewInput {
    Pointer(PointerInput),
    Key(KeyEvent),
    /// Assistive technology clicked a window.
    Select(u32),
    /// Assistive technology clicked a desktop.
    SwitchWorkspace(u32),
    /// Assistive technology clicked the add button.
    AddWorkspace,
    /// Assistive technology clicked a desktop's remove button.
    RemoveWorkspace(u32),
}

/// The surface layer of window `id`'s hole.
pub fn window_layer(id: u32) -> u64 {
    WINDOW_LAYERS + u64::from(id)
}

/// The surface layer of desktop `id`'s picture in the band.
pub fn workspace_layer(id: u32) -> u64 {
    WORKSPACE_LAYERS + u64::from(id)
}

/// The window whose hole is surface layer `layer`, if it is one.
pub fn layer_window(layer: u64) -> Option<u32> {
    layer
        .checked_sub(WINDOW_LAYERS)
        .filter(|id| *id < WORKSPACE_LAYERS - WINDOW_LAYERS)
        .and_then(|id| u32::try_from(id).ok())
}

/// Keysyms of the keys the overview takes.
const KEY_LEFT: u32 = 0xff51;
const KEY_UP: u32 = 0xff52;
const KEY_RIGHT: u32 = 0xff53;
const KEY_DOWN: u32 = 0xff54;
const KEY_TAB: u32 = 0xff09;
const KEY_LEFT_TAB: u32 = 0xfe20;
const KEY_RETURN: u32 = 0xff0d;
const KEY_KEYPAD_ENTER: u32 = 0xff8d;
const KEY_SPACE: u32 = 0x20;
const KEY_ESCAPE: u32 = 0xff1b;

/// The overview key a pressed key stands for: arrows move the selection, Tab and Shift+Tab walk it, Return, Enter and
/// Space choose it, Escape leaves.
fn overview_key(key: &KeyEvent) -> Option<OverviewKey> {
    if !key.pressed {
        return None;
    }
    Some(match key.keysym.0 {
        KEY_LEFT => OverviewKey::Left,
        KEY_RIGHT => OverviewKey::Right,
        KEY_UP => OverviewKey::Up,
        KEY_DOWN => OverviewKey::Down,
        KEY_LEFT_TAB => OverviewKey::Previous,
        KEY_TAB if key.mods.contains(cupertino_kit::platform::Mods::SHIFT) => OverviewKey::Previous,
        KEY_TAB => OverviewKey::Next,
        KEY_RETURN | KEY_KEYPAD_ENTER | KEY_SPACE => OverviewKey::Activate,
        KEY_ESCAPE => OverviewKey::Cancel,
        _ => return None,
    })
}

/// What a desktop is called: "Desktop" alone, else "Desktop" and its place.
fn desktop_name(index: usize, count: usize) -> String {
    if count > 1 {
        format!("Desktop {}", index + 1)
    } else {
        "Desktop".to_owned()
    }
}

/// The plate of a window's name: [`LABEL_HEIGHT`] tall, `text_width` and the space beside it wide (no wider than
/// the window), centred on the window drawn at `frame`.
pub fn label_plate(frame: WmRect, text_width: f64) -> WmRect {
    let w = (text_width + 2.0 * LABEL_SIDE).min(frame.w);
    WmRect::new(
        frame.cx() - w / 2.0,
        frame.cy() - LABEL_HEIGHT / 2.0,
        w,
        LABEL_HEIGHT,
    )
}

/// The ring round a window drawn at `frame`: outside it by the gap, as wide as the ring.
pub fn ring(frame: WmRect) -> WmRect {
    frame.inset(-(RING_GAP + RING_WIDTH))
}

/// What the pointer or a key on the window, or assistive technology, asks of the model at `now`.
pub fn action(ui: &mut OverviewUi, input: OverviewInput, now: f64) -> UiAction {
    if ui.is_closing() {
        return UiAction::None;
    }
    let at = |p: Point| WmPoint::new(p.x, p.y);
    match input {
        OverviewInput::Pointer(PointerInput::Moved(p)) => {
            ui.pointer_moved(Some(at(p)), now);
            UiAction::None
        }
        OverviewInput::Pointer(PointerInput::Left) => {
            ui.pointer_moved(None, now);
            UiAction::None
        }
        OverviewInput::Pointer(PointerInput::Pressed(p)) => ui.press(at(p), now),
        OverviewInput::Pointer(PointerInput::Released(p)) => ui.release(at(p), now),
        OverviewInput::Key(key) => match overview_key(&key) {
            Some(key) => ui.key(key, now),
            None => UiAction::None,
        },
        OverviewInput::Select(id) if ui.frames().iter().any(|f| f.0 == id) => UiAction::Select(id),
        OverviewInput::SwitchWorkspace(id) if ui.workspaces().iter().any(|w| w.id == id) => {
            UiAction::SwitchWorkspace(id)
        }
        OverviewInput::AddWorkspace if ui.mode() == Mode::All => UiAction::AddWorkspace,
        OverviewInput::RemoveWorkspace(id)
            if ui.workspaces().len() > 1 && ui.workspaces().iter().any(|w| w.id == id) =>
        {
            UiAction::RemoveWorkspace(id)
        }
        OverviewInput::Select(_)
        | OverviewInput::SwitchWorkspace(_)
        | OverviewInput::AddWorkspace
        | OverviewInput::RemoveWorkspace(_) => UiAction::None,
    }
}

/// Where the hole of a window of the overview is laid out, in the output's points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HolePlace {
    pub id: u32,
    pub rect: WmRect,
}

/// What the overview's window shows.
pub struct Scene<'a> {
    pub key: WindowKey,
    /// The output's size in points.
    pub size: Size,
    pub ui: &'a OverviewUi,
    /// The windows' holes, in the order they stack (the first lowest).
    pub holes: Vec<HolePlace>,
    /// Reduce Motion: the ring and the name go straight to the selected window's hole instead of gliding.
    pub reduce_motion: bool,
    /// What a hole paints itself: nothing in the shell, where the server draws the window or the desktop in it; a
    /// flat colour in a capture, which has no server.
    pub hole_fill: Option<Rgba>,
    /// The chrome's opacity where it paints in place (a capture); 1 in the shell, which fades the chrome's layer.
    pub chrome_opacity: f64,
}

/// What a hole paints: `fill`, or nothing.
fn hole_paint(fill: Option<Rgba>) -> Painter {
    Box::new(move |cx, _| {
        if let Some(colour) = fill {
            let size = cx.size();
            parts::fill(
                cx,
                Rect::new(0.0, 0.0, size.width, size.height),
                HOLE_RADIUS,
                colour,
            );
        }
    })
}

/// The overview's window at `now`; `frame`, when set, comes at every frame the compositor gives the window, while
/// something in it moves.
pub fn window<M, F>(scene: &Scene<'_>, now: f64, input: F, frame: Option<M>) -> Window<M>
where
    M: Clone + 'static,
    F: Fn(OverviewInput) -> M + Clone + 'static,
{
    let ui = scene.ui;
    let selection = ui.selection();
    let mut children = Vec::new();
    for hole in &scene.holes {
        let id = hole.id;
        let title = ui.title(id).unwrap_or_default();
        let name = if title.is_empty() { UNTITLED } else { title }.to_owned();
        let selected = selection == Some(id);
        children.push(
            Element::new(Part {
                rect: kit_rect(hole.rect),
                role: Some(Role::Button),
                name,
                states: AccessibleStates {
                    selected,
                    ..AccessibleStates::default()
                },
                click: Some(input(OverviewInput::Select(id))),
                look_key: String::new(),
                take_focus: selected,
                place: None,
                live: false,
                layer: Some(window_layer(id)),
                paint: hole_paint(scene.hole_fill),
            })
            .key(Key::from(format!("window-{id}"))),
        );
    }
    let mut chrome = Vec::new();
    if let Some(bar) = ui.bar(now) {
        band(
            &mut chrome,
            &mut children,
            &bar,
            ui,
            now,
            &input,
            scene.hole_fill,
        );
    }
    highlight(&mut chrome, scene, now);
    children.push(
        Element::new(Group {
            rect: Rect::new(0.0, 0.0, scene.size.width, scene.size.height),
            layer: CHROME_LAYER,
            opacity: scene.chrome_opacity,
            children: chrome,
        })
        .key(Key::from("chrome")),
    );
    Window {
        key: scene.key.clone(),
        spec: WindowSpec {
            title: TITLE.to_owned(),
            app_id: APP_ID.to_owned(),
            size: scene.size,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background: WindowBackground::None,
        content: Element::new(Root {
            role: Role::Group,
            name: TITLE.to_owned(),
            items: None,
            opacity: 1.0,
            pointer: Some(Box::new({
                let input = input.clone();
                move |p| input(OverviewInput::Pointer(p))
            })),
            key: Some(Box::new(move |k| input(OverviewInput::Key(k)))),
            frame,
            children,
        }),
        popup: None,
        menus: None,
    }
}

/// The ring round the selected window and its name on its plate, following the window as the model draws it; under
/// Reduce Motion, or while a window is dragged, at its hole.
fn highlight<M: Clone + 'static>(chrome: &mut Vec<Element<M>>, scene: &Scene<'_>, now: f64) {
    let ui = scene.ui;
    let Some(h) = ui.highlight(now) else {
        return;
    };
    let id = h.window.0 as u32;
    let at_hole = scene.reduce_motion || ui.window_drag().is_some();
    let frame = if at_hole {
        scene
            .holes
            .iter()
            .find(|p| p.id == id)
            .map_or(h.rect, |p| p.rect)
    } else {
        h.rect
    };
    let opacity = h.opacity;
    chrome.push(
        Element::new(Part {
            rect: kit_rect(ring(frame)),
            role: None,
            name: String::new(),
            states: AccessibleStates::default(),
            click: None,
            look_key: format!("{opacity}"),
            take_focus: false,
            place: None,
            live: false,
            layer: None,
            paint: Box::new(move |cx, look| {
                let size = cx.size();
                let width = if look.high_contrast {
                    CONTRAST_RING_WIDTH
                } else {
                    RING_WIDTH
                };
                outline(
                    cx,
                    Rect::new(0.0, 0.0, size.width, size.height),
                    RING_RADIUS,
                    width,
                    faded(SELECTION, opacity),
                );
            }),
        })
        .key(Key::from("ring")),
    );
    let title = ui.title(id).unwrap_or_default().to_owned();
    if !title.is_empty() {
        chrome.push(
            Element::new(Part {
                rect: kit_rect(frame),
                role: None,
                look_key: format!("{title}:{opacity}"),
                take_focus: false,
                place: None,
                live: false,
                layer: None,
                name: String::new(),
                states: AccessibleStates::default(),
                click: None,
                paint: Box::new(move |cx, look| label_paint(cx, look, &title, opacity)),
            })
            .key(Key::from(format!("label-{id}"))),
        );
    }
}

/// The desktops band: its tint and shadow, a part per desktop (its picture's ring and its name), the add button
/// and the remove button.
fn band<M, F>(
    children: &mut Vec<Element<M>>,
    pictures: &mut Vec<Element<M>>,
    bar: &BarLayout,
    ui: &OverviewUi,
    now: f64,
    input: &F,
    hole_fill: Option<Rgba>,
) where
    M: Clone + 'static,
    F: Fn(OverviewInput) -> M + Clone + 'static,
{
    children.push(
        Element::new(Part {
            rect: kit_rect(bar.rect),
            role: None,
            name: String::new(),
            states: AccessibleStates::default(),
            click: None,
            look_key: String::new(),
            take_focus: false,
            layer: None,
            place: None,
            live: false,
            paint: Box::new(|cx, look| {
                let size = cx.size();
                let fill_colour = if look.opaque() { BAND_OPAQUE } else { BAND };
                fill(
                    cx,
                    Rect::new(0.0, 0.0, size.width, size.height),
                    0.0,
                    fill_colour,
                );
            }),
        })
        .key(Key::from("band")),
    );
    let shadow = WmRect::new(bar.rect.x, bar.rect.bottom(), bar.rect.w, BAND_SHADOW);
    children.push(
        Element::new(Part {
            rect: kit_rect(shadow),
            role: None,
            name: String::new(),
            states: AccessibleStates::default(),
            click: None,
            look_key: String::new(),
            take_focus: false,
            layer: None,
            place: None,
            live: false,
            paint: Box::new(|cx, _| {
                let size = cx.size();
                cx.push(PaintOp::Fill {
                    shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height)),
                    paint: Paint::Linear {
                        start: Point::new(0.0, 0.0),
                        end: Point::new(0.0, size.height),
                        stops: vec![
                            GradientStop {
                                offset: 0.0,
                                color: parts::color([0.0, 0.0, 0.0, BAND_SHADOW_ALPHA]),
                            },
                            GradientStop {
                                offset: 1.0,
                                color: parts::color([0.0, 0.0, 0.0, 0.0]),
                            },
                        ],
                    },
                });
            }),
        })
        .key(Key::from("band-shadow")),
    );
    let drag = ui.drag();
    let drop = ui.drop_target(now);
    let count = bar.slots.len();
    for (i, slot) in bar.slots.iter().enumerate() {
        let id = slot.id.0 as u32;
        let offset = drag
            .filter(|d| d.0 == id)
            .map_or(WmPoint::new(0.0, 0.0), |d| d.1);
        let (thumb, name_rect) = (
            slot.thumb.translate(offset.x, offset.y),
            slot.name.translate(offset.x, offset.y),
        );
        let name = desktop_name(i, count);
        let current = slot.current;
        let ringed = (current || drop == Some(id)) && thumb.w >= 1.0;
        if ringed {
            children.push(
                Element::new(Part {
                    rect: kit_rect(thumb.inset(-PICTURE_RING_WIDTH)),
                    role: None,
                    name: String::new(),
                    states: AccessibleStates::default(),
                    click: None,
                    look_key: String::new(),
                    take_focus: false,
                    layer: None,
                    place: None,
                    live: false,
                    paint: Box::new(|cx, _| {
                        let size = cx.size();
                        outline(
                            cx,
                            Rect::new(0.0, 0.0, size.width, size.height),
                            PICTURE_RING_RADIUS + PICTURE_RING_WIDTH,
                            PICTURE_RING_WIDTH,
                            SELECTION,
                        );
                    }),
                })
                .key(Key::from(format!("ring-{id}"))),
            );
        }
        if thumb.w >= 1.0 {
            // the server draws the desktop in this hole; the part names it for assistive technology
            pictures.push(
                Element::new(Part {
                    rect: kit_rect(thumb),
                    role: Some(Role::Button),
                    look_key: String::new(),
                    take_focus: false,
                    layer: Some(workspace_layer(id)),
                    place: None,
                    live: false,
                    name: name.clone(),
                    states: AccessibleStates {
                        selected: current,
                        ..AccessibleStates::default()
                    },
                    click: Some(input(OverviewInput::SwitchWorkspace(id))),
                    paint: hole_paint(hole_fill),
                })
                .key(Key::from(format!("picture-{id}"))),
            );
        }
        let label = name.clone();
        children.push(
            Element::new(Part {
                rect: kit_rect(name_rect),
                role: (thumb.w < 1.0).then_some(Role::Button),
                look_key: format!("{name}:{current}"),
                take_focus: false,
                layer: None,
                place: None,
                live: false,
                name: if thumb.w < 1.0 { name } else { String::new() },
                states: AccessibleStates {
                    selected: current,
                    ..AccessibleStates::default()
                },
                click: Some(input(OverviewInput::SwitchWorkspace(id))),
                paint: Box::new(move |cx, look| name_paint(cx, look, &label, current)),
            })
            .key(Key::from(format!("slot-{id}"))),
        );
        if drag.is_none() && ui.removable(now) == Some(id) && thumb.w >= 1.0 {
            children.push(
                Element::new(Part {
                    rect: kit_rect(OverviewUi::remove_button(&thumb)),
                    role: Some(Role::Button),
                    name: format!("Remove {}", desktop_name(i, count)),
                    states: AccessibleStates::default(),
                    click: Some(input(OverviewInput::RemoveWorkspace(id))),
                    look_key: String::new(),
                    take_focus: false,
                    layer: None,
                    place: None,
                    live: false,
                    paint: Box::new(|cx, _| {
                        let size = cx.size();
                        let d = size.width;
                        fill(cx, Rect::new(0.0, 0.0, d, d), d / 2.0, REMOVE_FILL);
                        let (c, r) = (d / 2.0, d * CROSS_FRACTION);
                        let p = |x: f64, y: f64| Point::new(c + x, c + y);
                        line(cx, p(-r, -r), p(r, r), CROSS_WIDTH, REMOVE_CROSS);
                        line(cx, p(-r, r), p(r, -r), CROSS_WIDTH, REMOVE_CROSS);
                    }),
                })
                .key(Key::from(format!("remove-{id}"))),
            );
        }
    }
    children.push(
        Element::new(Part {
            rect: kit_rect(bar.add),
            role: Some(Role::Button),
            name: "Add Desktop".into(),
            states: AccessibleStates::default(),
            click: Some(input(OverviewInput::AddWorkspace)),
            look_key: String::new(),
            take_focus: false,
            layer: None,
            place: None,
            live: false,
            paint: Box::new(|cx, _| {
                let size = cx.size();
                let (x, y) = (size.width / 2.0, size.height / 2.0);
                let a = PLUS_ARM;
                line(
                    cx,
                    Point::new(x - a, y),
                    Point::new(x + a, y),
                    PLUS_WIDTH,
                    GLYPH,
                );
                line(
                    cx,
                    Point::new(x, y - a),
                    Point::new(x, y + a),
                    PLUS_WIDTH,
                    GLYPH,
                );
            }),
        })
        .key(Key::from("add")),
    );
}

/// A desktop's name centred in its row, white; the current one on a pill.
fn name_paint(cx: &mut PaintCx<'_>, look: Look, name: &str, current: bool) {
    let size = cx.size();
    let row = Rect::new(0.0, 0.0, size.width, size.height);
    if current {
        let w = (parts::text_width(cx, name, NAME_PT, NAME_WEIGHT) + 2.0 * NAME_PILL_SIDE)
            .min(size.width);
        let pill = Rect::new((size.width - w) / 2.0, 0.0, w, size.height);
        let colour = if look.opaque() {
            NAME_PILL_OPAQUE
        } else {
            NAME_PILL
        };
        fill(cx, pill, NAME_PILL_RADIUS, colour);
    }
    centred_text(cx, name, row, NAME_PT, NAME_WEIGHT, GLYPH);
}

/// The selected window's name on its plate, in the middle of the window's part.
fn label_paint(cx: &mut PaintCx<'_>, look: Look, title: &str, opacity: f64) {
    let size = cx.size();
    let text = parts::text_width(cx, title, LABEL_PT, LABEL_WEIGHT);
    let plate = label_plate(WmRect::new(0.0, 0.0, size.width, size.height), text);
    let colour = if look.opaque() {
        LABEL_PLATE_OPAQUE
    } else {
        LABEL_PLATE
    };
    fill(
        cx,
        kit_rect(plate),
        LABEL_HEIGHT / 2.0,
        faded(colour, opacity),
    );
    let row = kit_rect(plate.inset(0.0));
    let inner = Rect::new(
        row.origin.x + LABEL_SIDE / 2.0,
        row.origin.y,
        (row.size.width - LABEL_SIDE).max(0.0),
        row.size.height,
    );
    centred_text(
        cx,
        title,
        inner,
        LABEL_PT,
        LABEL_WEIGHT,
        faded(LABEL_TEXT, opacity),
    );
}
