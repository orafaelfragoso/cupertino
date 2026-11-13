//! The overview's chrome on CupertinoKit: one window over the whole output that the dock's roles put on an overlay
//! layer surface anchored to every edge (namespace [`APP_ID`], exclusive zone -1, so the window's coordinates are the
//! output's), with the keyboard while the overview is open ([`OverviewSurface::wants_keyboard`]). The server draws the
//! windows and the desktops' live pictures; the chrome draws the desktops band across the top (a tinted band with a
//! soft shadow under it, the desktops' names with the current one on a pill, the bare plus that adds a desktop, the
//! ring round the current desktop's picture and the remove button after a rest on a picture), the ring round the
//! selected window and its name on a plate in its middle, and springs out when the server closes the overview.
//!
//! The windows' layout is the overview model's ([`crate::overview_ui`] on `cupertino-overview`): below the band by its
//! layout gap, balanced in the area and never overlapping, moving down when the band opens. [`OverviewSurface::changed`]
//! returns the frames the server is told with `overview_frame`, and [`OverviewSurface::input`] and
//! [`OverviewSurface::tick`] the requests that follow from the pointer and the clock: new frames when the band opens
//! or closes, a dragged window's place, a window dropped on a desktop, and where the desktops' pictures go. Assistive
//! technology sees the windows and the desktops as buttons named by their titles and desktop names, with the
//! keyboard's selection as their focus. Under Reduce Motion the highlight and the names go straight to where they end
//! instead of gliding. Nothing here speaks Wayland.

use crate::overview_ui::{Mode, OverviewUi, UiAction, WindowEntry, Workspace};
use crate::wm_kit::{
    self, centred_text, faded, fill, kit_rect, line, outline, Look, Part, PointerInput, Rgba, Root,
};
use cupertino_kit::core::{
    AccessibleStates, CommandRegistry, Element, Key, PaintCx, Role, Window, WindowBackground,
    WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::platform::{DecorationMode, KeyEvent, WindowSpec};
use cupertino_kit::render::{GradientStop, Paint, PaintOp, Shape};
use cupertino_overview::Key as OverviewKey;
use cupertino_wm::geometry::{Point as WmPoint, Rect as WmRect};

/// The `app_id` of the chrome's window, and the namespace of its layer surface.
pub const APP_ID: &str = "cupertino-dock-overview";

/// What assistive technology calls the chrome.
const TITLE: &str = "Overview";

/// How often a moving part is redrawn, in seconds.
pub const FRAME: f64 = 1.0 / 60.0;

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

/// What reaches the chrome from its window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverviewInput {
    Pointer(PointerInput),
    Key(KeyEvent),
    /// Assistive technology clicked a window.
    Select(u32),
    /// Assistive technology clicked a workspace slot.
    SwitchWorkspace(u32),
    /// Assistive technology clicked the add slot.
    AddWorkspace,
    /// Assistive technology clicked a slot's remove button.
    RemoveWorkspace(u32),
}

/// What the chrome asks the server for (the dock role's requests of the same names).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverviewRequest {
    /// `overview_select`
    Select(u32),
    /// `overview_close`
    Close,
    /// `switch_workspace`
    SwitchWorkspace(u32),
    /// `add_workspace`
    AddWorkspace,
    /// `remove_workspace`
    RemoveWorkspace(u32),
    /// `move_workspace`
    MoveWorkspace { id: u32, index: usize },
    /// `overview_frame`: the window's place in a new layout.
    Frame(u32, WmRect),
    /// `overview_drag`: the window follows the pointer.
    Drag(u32, WmRect),
    /// `overview_move_window`: the window was dropped on another desktop.
    MoveWindow { window: u32, workspace: u32 },
    /// `overview_workspace_frame`: where the desktop's live picture is drawn; an empty rectangle for none.
    Picture(u32, WmRect),
}

impl OverviewRequest {
    fn of(action: UiAction) -> Option<OverviewRequest> {
        Some(match action {
            UiAction::None => return None,
            UiAction::Select(id) => OverviewRequest::Select(id),
            UiAction::Close => OverviewRequest::Close,
            UiAction::SwitchWorkspace(id) => OverviewRequest::SwitchWorkspace(id),
            UiAction::AddWorkspace => OverviewRequest::AddWorkspace,
            UiAction::RemoveWorkspace(id) => OverviewRequest::RemoveWorkspace(id),
            UiAction::MoveWorkspace { id, index } => OverviewRequest::MoveWorkspace { id, index },
            UiAction::MoveWindow { window, workspace } => {
                OverviewRequest::MoveWindow { window, workspace }
            }
        })
    }
}

/// The overview's chrome across overviews.
#[derive(Default)]
pub struct OverviewSurface {
    ui: Option<OverviewUi>,
    /// Counts the overviews, so each gets a window of its own.
    generation: u64,
    reduce_motion: bool,
    /// The layout the server was last told the frames of.
    sent_layout: u64,
    /// The desktops' pictures as the server was last told them.
    sent_pictures: Vec<(u32, WmRect)>,
    /// The window the server was last told follows the pointer.
    dragging: Option<u32>,
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

/// A rectangle on whole points, as the server is told it.
fn whole(r: WmRect) -> WmRect {
    let (x, y) = (r.x.floor(), r.y.floor());
    WmRect::new(x, y, r.right().ceil() - x, r.bottom().ceil() - y)
}

/// What a desktop is called: "Desktop" alone, else "Desktop" and its place.
fn desktop_name(index: usize, count: usize) -> String {
    if count > 1 {
        format!("Desktop {}", index + 1)
    } else {
        "Desktop".to_owned()
    }
}

impl OverviewSurface {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reduce Motion from the session's display options.
    pub fn set_reduce_motion(&mut self, on: bool) {
        self.reduce_motion = on;
    }

    /// The server opened the overview over `area` (output points) or changed its windows or workspaces. Returns the
    /// frame of every window, for `overview_frame`. A new overview, or another mode, starts a new window.
    pub fn changed(
        &mut self,
        mode: Mode,
        area: WmRect,
        windows: &[WindowEntry],
        workspaces: Vec<Workspace>,
        now: f64,
    ) -> Vec<(u32, WmRect)> {
        match self
            .ui
            .as_mut()
            .filter(|ui| !ui.is_closing() && ui.mode() == mode)
        {
            Some(ui) => {
                ui.set_windows(windows, now);
                ui.set_workspaces(workspaces);
            }
            None => {
                self.generation += 1;
                self.sent_pictures.clear();
                self.dragging = None;
                self.ui = Some(OverviewUi::new(mode, area, windows, workspaces, now));
            }
        }
        let Some(ui) = self.ui.as_ref() else {
            return Vec::new();
        };
        self.sent_layout = ui.layout_revision();
        ui.frames()
    }

    /// The server is leaving the overview: the chrome springs out and gives the keyboard back.
    pub fn closed(&mut self, now: f64) {
        if let Some(ui) = self.ui.as_mut() {
            ui.close(now);
        }
    }

    /// The server closed the chrome's window.
    pub fn lost(&mut self) {
        self.ui = None;
    }

    /// Advance the clock: a finished fade ends the chrome. Returns what the server is to be told of what moved by
    /// itself (the desktops' pictures while the band opens or closes). Call before [`Self::window`] on every timer.
    pub fn tick(&mut self, now: f64) -> Vec<OverviewRequest> {
        if self.ui.as_ref().is_some_and(|ui| ui.is_gone(now)) {
            self.ui = None;
        }
        self.follow(now)
    }

    /// The requests that bring the server up to date with the model: frames of a new layout, the dragged window's
    /// place (or its way back), and the desktops' pictures.
    fn follow(&mut self, now: f64) -> Vec<OverviewRequest> {
        let Some(ui) = self.ui.as_ref().filter(|ui| !ui.is_closing()) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let dragged = ui.window_drag();
        if ui.layout_revision() != self.sent_layout {
            self.sent_layout = ui.layout_revision();
            out.extend(
                ui.frames()
                    .into_iter()
                    .filter(|(id, _)| dragged.is_none_or(|d| d.0 != *id))
                    .map(|(id, r)| OverviewRequest::Frame(id, r)),
            );
        }
        match (dragged, self.dragging) {
            (Some((id, r)), _) => {
                self.dragging = Some(id);
                out.push(OverviewRequest::Drag(id, whole(r)));
            }
            (None, Some(id)) => {
                // dropped where nothing takes it: the window goes back to its place
                self.dragging = None;
                if let Some((_, r)) = ui.frames().into_iter().find(|f| f.0 == id) {
                    out.push(OverviewRequest::Frame(id, r));
                }
            }
            (None, None) => {}
        }
        let pictures: Vec<(u32, WmRect)> = ui
            .bar(now)
            .map(|bar| {
                bar.slots
                    .iter()
                    .filter(|s| s.thumb.w >= 1.0 && s.thumb.h >= 1.0)
                    .map(|s| (s.id.0 as u32, whole(s.thumb)))
                    .collect()
            })
            .unwrap_or_default();
        for (id, _) in &self.sent_pictures {
            if !pictures.iter().any(|p| p.0 == *id) {
                out.push(OverviewRequest::Picture(*id, WmRect::default()));
            }
        }
        for p in &pictures {
            if !self.sent_pictures.contains(p) {
                out.push(OverviewRequest::Picture(p.0, p.1));
            }
        }
        self.sent_pictures = pictures;
        out
    }

    /// Whether the chrome is open and takes the keyboard (not once it fades).
    pub fn wants_keyboard(&self) -> bool {
        self.ui.as_ref().is_some_and(|ui| !ui.is_closing())
    }

    /// Whether no chrome is shown.
    pub fn is_done(&self, now: f64) -> bool {
        self.ui.as_ref().is_none_or(|ui| ui.is_gone(now))
    }

    /// The model, while an overview is shown.
    pub fn ui(&self) -> Option<&OverviewUi> {
        self.ui.as_ref()
    }

    /// When the chrome next changes by itself: the next frame while the reflow, the band, the highlight, a drag or the
    /// fade moves, or when the remove button is due. Under Reduce Motion only the fades, the band and drags need frames.
    pub fn next_wake(&self, now: f64) -> Option<f64> {
        let ui = self.ui.as_ref()?;
        let moving = if self.reduce_motion {
            ui.is_closing()
                || ui.drag().is_some()
                || ui.window_drag().is_some()
                || ui
                    .bar(now)
                    .is_some_and(|b| b.expansion > 0.0 && b.expansion < 1.0)
                || ui
                    .highlight(now)
                    .is_some_and(|h| h.opacity > 0.0 && h.opacity < 1.0)
        } else {
            ui.is_animating(now)
        };
        if moving {
            Some(now + FRAME)
        } else {
            ui.next_change(now)
        }
    }

    /// What the window reported; the requests for the server.
    pub fn input(&mut self, input: OverviewInput, now: f64) -> Vec<OverviewRequest> {
        let Some(ui) = self.ui.as_mut() else {
            return Vec::new();
        };
        if ui.is_closing() {
            return Vec::new();
        }
        let at = |p: Point| WmPoint::new(p.x, p.y);
        let action = match input {
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
            OverviewInput::Select(id) if ui.frames().iter().any(|f| f.0 == id) => {
                UiAction::Select(id)
            }
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
        };
        let request = OverviewRequest::of(action);
        if matches!(
            request,
            Some(OverviewRequest::MoveWindow { .. } | OverviewRequest::Select(_))
        ) {
            // the window leaves the overview with the request: it does not go back
            self.dragging = None;
        }
        let mut out = self.follow(now);
        out.extend(request);
        out
    }

    /// Where every window's thumbnail is drawn at `now`: the reflow's place, or the final one under Reduce Motion. In
    /// the model's order, with the window's id; a dragged window where the pointer took it.
    pub fn thumbnails(&self, now: f64) -> Vec<(u32, WmRect)> {
        let Some(ui) = self.ui.as_ref() else {
            return Vec::new();
        };
        let mut placed: Vec<(u32, WmRect)> = if self.reduce_motion {
            ui.frames()
        } else {
            ui.placed(now)
                .into_iter()
                .map(|w| (w.id.0 as u32, w.frame))
                .collect()
        };
        if let Some((id, r)) = ui.window_drag() {
            if let Some(p) = placed.iter_mut().find(|p| p.0 == id) {
                p.1 = r;
            }
        }
        placed
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

    /// The chrome's window at `now`, until its fade has finished. `output` is the output's size in points.
    pub fn window<M, F>(&self, now: f64, output: Size, input: F) -> Option<Window<M>>
    where
        M: Clone + 'static,
        F: Fn(OverviewInput) -> M + Clone + 'static,
    {
        self.build(now, output, input, None)
    }

    /// [`Self::window`] that sends `frame` at every frame the compositor gives the window while something in it moves
    /// ([`Self::next_wake`]), so the caller advances the chrome with the display rather than with a timer.
    pub fn window_with_frames<M, F>(
        &self,
        now: f64,
        output: Size,
        input: F,
        frame: M,
    ) -> Option<Window<M>>
    where
        M: Clone + 'static,
        F: Fn(OverviewInput) -> M + Clone + 'static,
    {
        let moving = self.next_wake(now).is_some_and(|at| at - now <= FRAME);
        self.build(now, output, input, moving.then_some(frame))
    }

    fn build<M, F>(&self, now: f64, output: Size, input: F, frame: Option<M>) -> Option<Window<M>>
    where
        M: Clone + 'static,
        F: Fn(OverviewInput) -> M + Clone + 'static,
    {
        let ui = self.ui.as_ref()?;
        if ui.is_gone(now) {
            return None;
        }
        let mut children = Vec::new();
        if let Some(bar) = ui.bar(now) {
            Self::band(&mut children, &bar, ui, now, &input);
        }
        let thumbnails = self.thumbnails(now);
        let selection = ui.selection();
        for (id, frame) in &thumbnails {
            let (id, frame) = (*id, *frame);
            let title = ui.title(id).unwrap_or_default().to_owned();
            let name = if title.is_empty() {
                UNTITLED.to_owned()
            } else {
                title
            };
            let selected = selection == Some(id);
            children.push(
                Element::new(Part {
                    rect: kit_rect(frame),
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
                    // the server draws the window itself
                    paint: Box::new(|_, _| {}),
                })
                .key(Key::from(format!("window-{id}"))),
            );
        }
        if let Some(h) = ui.highlight(now).filter(|_| ui.window_drag().is_none()) {
            let id = h.window.0 as u32;
            // the ring and the name follow the window as it is drawn; under Reduce Motion straight to its place
            let frame = if self.reduce_motion {
                thumbnails
                    .iter()
                    .find(|(w, _)| *w == id)
                    .map_or(h.rect, |(_, f)| *f)
            } else {
                h.rect
            };
            let opacity = h.opacity;
            children.push(Element::new(Part {
                rect: kit_rect(Self::ring(frame)),
                role: None,
                name: String::new(),
                states: AccessibleStates::default(),
                click: None,
                look_key: format!("{opacity}"),
                take_focus: false,
                place: None,
                live: false,
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
            }));
            let title = ui.title(id).unwrap_or_default().to_owned();
            if !title.is_empty() {
                children.push(
                    Element::new(Part {
                        rect: kit_rect(frame),
                        role: None,
                        look_key: format!("{title}:{opacity}"),
                        take_focus: false,
                        place: None,
                        live: false,
                        name: String::new(),
                        states: AccessibleStates::default(),
                        click: None,
                        paint: Box::new(move |cx, look| label_paint(cx, look, &title, opacity)),
                    })
                    .key(Key::from(format!("label-{id}"))),
                );
            }
        }
        let alpha = ui.alpha(now);
        Some(Window {
            key: WindowKey(Key::from(format!("overview-{}", self.generation))),
            spec: WindowSpec {
                title: TITLE.to_owned(),
                app_id: APP_ID.to_owned(),
                size: output,
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
                opacity: alpha,
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
        })
    }

    /// The desktops band: its tint and shadow, a part per desktop (its picture's ring and its name), the add button
    /// and the remove button.
    fn band<M, F>(
        children: &mut Vec<Element<M>>,
        bar: &cupertino_overview::BarLayout,
        ui: &OverviewUi,
        now: f64,
        input: &F,
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
                                    color: wm_kit::color([0.0, 0.0, 0.0, BAND_SHADOW_ALPHA]),
                                },
                                GradientStop {
                                    offset: 1.0,
                                    color: wm_kit::color([0.0, 0.0, 0.0, 0.0]),
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
                // the picture itself is the server's; this part names it for assistive technology
                children.push(
                    Element::new(Part {
                        rect: kit_rect(thumb),
                        role: Some(Role::Button),
                        look_key: String::new(),
                        take_focus: false,
                        place: None,
                        live: false,
                        name: name.clone(),
                        states: AccessibleStates {
                            selected: current,
                            ..AccessibleStates::default()
                        },
                        click: Some(input(OverviewInput::SwitchWorkspace(id))),
                        paint: Box::new(|_, _| {}),
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
}

/// A desktop's name centred in its row, white; the current one on a pill.
fn name_paint(cx: &mut PaintCx<'_>, look: Look, name: &str, current: bool) {
    let size = cx.size();
    let row = Rect::new(0.0, 0.0, size.width, size.height);
    if current {
        let w = (wm_kit::text_width(cx, name, NAME_PT, NAME_WEIGHT) + 2.0 * NAME_PILL_SIDE)
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
    let text = wm_kit::text_width(cx, title, LABEL_PT, LABEL_WEIGHT);
    let plate = OverviewSurface::label_plate(WmRect::new(0.0, 0.0, size.width, size.height), text);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_plate_is_centred_on_the_window_and_no_wider() {
        let frame = WmRect::new(10.0, 20.0, 300.0, 200.0);
        let plate = OverviewSurface::label_plate(frame, 100.0);
        assert_eq!(plate.h, 28.0);
        assert_eq!(plate.w, 128.0);
        assert!((plate.cx() - frame.cx()).abs() < 1e-9 && (plate.cy() - frame.cy()).abs() < 1e-9);
        assert_eq!(OverviewSurface::label_plate(frame, 1000.0).w, 300.0);
    }

    #[test]
    fn the_ring_lies_just_outside_the_window() {
        let frame = WmRect::new(100.0, 100.0, 300.0, 200.0);
        let ring = OverviewSurface::ring(frame);
        assert_eq!(ring.x, frame.x - 6.5);
        assert_eq!(ring.w, frame.w + 13.0);
    }

    #[test]
    fn a_lone_desktop_is_called_desktop() {
        assert_eq!(desktop_name(0, 1), "Desktop");
        assert_eq!(desktop_name(1, 3), "Desktop 2");
    }
}
