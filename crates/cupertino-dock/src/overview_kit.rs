//! The overview's chrome on CupertinoKit: one window over the whole output that the dock's roles put on an overlay
//! layer surface anchored to every edge (namespace [`APP_ID`], exclusive zone -1, so the window's coordinates are the
//! output's), with the keyboard while the overview is open ([`OverviewSurface::wants_keyboard`]). The server draws the
//! dim and the windows; the chrome draws the workspaces strip (a slot per workspace with its name, a border on the
//! current one, the add slot, the remove button on the hovered slot), the selection highlight and a title under every
//! window, and fades out when the server closes the overview.
//!
//! The windows' layout is the overview model's ([`crate::overview_ui`] on `cupertino-overview`): below the strip by
//! the strip's layout gap, balanced in the area and never overlapping. [`OverviewSurface::changed`] returns the frames
//! the server is told with `overview_frame`; the window draws and hit-tests the same frames. Pointer and keys go to the
//! model as before and come back as [`OverviewRequest`]s for the dock role; assistive technology sees the windows and
//! the slots as buttons named by their titles and desktop numbers, with the keyboard's selection as their focus.
//! Under Reduce Motion the highlight and the titles go straight to where they end instead of gliding. Nothing here
//! speaks Wayland.

use crate::overview_ui::{Mode, OverviewUi, UiAction, WindowEntry, Workspace};
use crate::wm_kit::{
    self, centred_text, faded, fill, kit_rect, line, outline, Look, Part, PointerInput, Rgba, Root,
};
use cupertino_kit::core::{
    AccessibleStates, CommandRegistry, Element, Key, Role, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::platform::{DecorationMode, KeyEvent, WindowSpec};
use cupertino_overview::{Key as OverviewKey, OverviewConfig};
use cupertino_wm::geometry::{Point as WmPoint, Rect as WmRect};

/// The `app_id` of the chrome's window, and the namespace of its layer surface.
pub const APP_ID: &str = "cupertino-dock-overview";

/// What assistive technology calls the chrome.
const TITLE: &str = "Overview";

/// How often a moving part is redrawn, in seconds.
pub const FRAME: f64 = 1.0 / 60.0;

/// Corner radius of a workspace slot, in points.
const SLOT_RADIUS: f64 = 8.0;
/// A slot's fill over the dim: translucent black in light, translucent white in dark.
const SLOT_LIGHT: Rgba = [0.0, 0.0, 0.0, 0.18];
const SLOT_DARK: Rgba = [1.0, 1.0, 1.0, 0.16];
/// A slot's fill under Reduce Transparency or Increase Contrast: opaque grays.
const SLOT_OPAQUE_LIGHT: Rgba = [0.80, 0.80, 0.82, 1.0];
const SLOT_OPAQUE_DARK: Rgba = [0.22, 0.22, 0.24, 1.0];
/// The current slot's border, and its width in points (doubled under Increase Contrast).
const CURRENT_BORDER: Rgba = [1.0, 1.0, 1.0, 0.85];
const CURRENT_BORDER_WIDTH: f64 = 2.0;
/// Under Increase Contrast every slot has an outline this wide, in points.
const CONTRAST_OUTLINE_WIDTH: f64 = 1.0;
/// The selection highlight round the selected window: colour, corner radius and width in points.
const HIGHLIGHT: Rgba = [1.0, 1.0, 1.0, 0.9];
const HIGHLIGHT_RADIUS: f64 = 14.0;
const HIGHLIGHT_WIDTH: f64 = 3.0;
/// The highlight's width under Increase Contrast.
const CONTRAST_HIGHLIGHT_WIDTH: f64 = 4.0;
/// The remove button's fill.
const REMOVE_FILL: Rgba = [0.2, 0.2, 0.22, 0.95];
/// Labels and glyphs over the dim.
const GLYPH: Rgba = [1.0, 1.0, 1.0, 0.95];
/// Labels and glyphs over an opaque light slot.
const GLYPH_ON_LIGHT: Rgba = [0.0, 0.0, 0.0, 0.95];
/// The plus of the add slot as a fraction of the slot's smaller side, and its stroke width in points.
const PLUS_FRACTION: f64 = 0.18;
const PLUS_WIDTH: f64 = 2.0;
/// The cross of the remove button as a fraction of its diameter, and its stroke width in points.
const CROSS_FRACTION: f64 = 0.22;
const CROSS_WIDTH: f64 = 1.5;
/// A slot's label: size, weight and the space kept to the slot's sides, in points.
const LABEL_PT: f64 = 12.0;
const LABEL_WEIGHT: u16 = 500;
const LABEL_SIDE: f64 = 4.0;
/// A window's title: size, weight, the space between the window and its title, and the title row's height, in points.
const TITLE_PT: f64 = 13.0;
const TITLE_WEIGHT: u16 = 500;
const TITLE_GAP: f64 = 6.0;
const TITLE_ROW: f64 = 18.0;
/// The title's shadow: one point down, black at this opacity.
const TITLE_SHADOW: Rgba = [0.0, 0.0, 0.0, 0.2];
/// Under Increase Contrast a title sits on a plate of this colour, this far round the text, in points.
const TITLE_PLATE: Rgba = [0.0, 0.0, 0.0, 0.85];
const TITLE_PLATE_PADDING: f64 = 4.0;
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
                self.ui = Some(OverviewUi::new(mode, area, windows, workspaces, now));
            }
        }
        self.ui.as_ref().map(OverviewUi::frames).unwrap_or_default()
    }

    /// The server is leaving the overview: the chrome fades out and gives the keyboard back.
    pub fn closed(&mut self, now: f64) {
        if let Some(ui) = self.ui.as_mut() {
            ui.close(now);
        }
    }

    /// The server closed the chrome's window.
    pub fn lost(&mut self) {
        self.ui = None;
    }

    /// Advance the clock: a finished fade ends the chrome. Call before [`Self::window`] on every timer.
    pub fn tick(&mut self, now: f64) {
        if self.ui.as_ref().is_some_and(|ui| ui.is_gone(now)) {
            self.ui = None;
        }
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

    /// When the chrome next changes by itself: the next frame while the reflow, the highlight, a drag or the fade
    /// moves. Under Reduce Motion only the fades need frames.
    pub fn next_wake(&self, now: f64) -> Option<f64> {
        let ui = self.ui.as_ref()?;
        let moving = if self.reduce_motion {
            ui.is_closing()
                || ui.drag().is_some()
                || ui
                    .highlight(now)
                    .is_some_and(|h| h.opacity > 0.0 && h.opacity < 1.0)
        } else {
            ui.is_animating(now)
        };
        moving.then_some(now + FRAME)
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
        OverviewRequest::of(action).into_iter().collect()
    }

    /// Where every window's thumbnail and title are drawn at `now`: the reflow's place, or the final one under Reduce
    /// Motion. In the model's order, with the window's id.
    pub fn thumbnails(&self, now: f64) -> Vec<(u32, WmRect)> {
        let Some(ui) = self.ui.as_ref() else {
            return Vec::new();
        };
        if self.reduce_motion {
            ui.frames()
        } else {
            ui.placed(now)
                .into_iter()
                .map(|w| (w.id.0 as u32, w.frame))
                .collect()
        }
    }

    /// The title row of a window drawn at `frame`.
    pub fn title_row(frame: WmRect) -> WmRect {
        WmRect::new(frame.x, frame.bottom() + TITLE_GAP, frame.w, TITLE_ROW)
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
        let moving = self.next_wake(now).is_some();
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
            let drag = ui.drag();
            let several = bar.slots.len() > 1;
            for (i, slot) in bar.slots.iter().enumerate() {
                let id = slot.id.0 as u32;
                let mut rect = slot.rect;
                if let Some(offset) = drag.filter(|d| d.0 == id).map(|d| d.1) {
                    rect = rect.translate(offset.x, offset.y);
                }
                let name = format!("Desktop {}", i + 1);
                let current = slot.current;
                let label = name.clone();
                children.push(
                    Element::new(Part {
                        rect: kit_rect(rect),
                        role: Some(Role::Button),
                        look_key: format!("{name}:{current}"),
                        take_focus: false,
                        live: false,
                        name,
                        states: AccessibleStates {
                            selected: current,
                            ..AccessibleStates::default()
                        },
                        click: Some(input(OverviewInput::SwitchWorkspace(id))),
                        paint: Box::new(move |cx, look| {
                            let size = cx.size();
                            let all = Rect::new(0.0, 0.0, size.width, size.height);
                            fill(cx, all, SLOT_RADIUS, slot_fill(look));
                            if look.high_contrast {
                                outline(cx, all, SLOT_RADIUS, CONTRAST_OUTLINE_WIDTH, glyph(look));
                            }
                            if current {
                                let width = if look.high_contrast {
                                    2.0 * CURRENT_BORDER_WIDTH
                                } else {
                                    CURRENT_BORDER_WIDTH
                                };
                                outline(cx, all, SLOT_RADIUS, width, current_border(look));
                            }
                            let text = Rect::new(
                                LABEL_SIDE,
                                0.0,
                                (size.width - 2.0 * LABEL_SIDE).max(0.0),
                                size.height,
                            );
                            centred_text(cx, &label, text, LABEL_PT, LABEL_WEIGHT, glyph(look));
                        }),
                    })
                    .key(Key::from(format!("slot-{id}"))),
                );
                if several && drag.is_none() && ui.hovered_slot() == Some(id) {
                    children.push(
                        Element::new(Part {
                            rect: kit_rect(OverviewUi::remove_button(&rect)),
                            role: Some(Role::Button),
                            name: format!("Remove Desktop {}", i + 1),
                            states: AccessibleStates::default(),
                            click: Some(input(OverviewInput::RemoveWorkspace(id))),
                            look_key: String::new(),
                            take_focus: false,
                            live: false,
                            paint: Box::new(|cx, _| {
                                let size = cx.size();
                                let d = size.width;
                                fill(cx, Rect::new(0.0, 0.0, d, d), d / 2.0, REMOVE_FILL);
                                let (c, r) = (d / 2.0, d * CROSS_FRACTION);
                                let p = |x: f64, y: f64| Point::new(c + x, c + y);
                                line(cx, p(-r, -r), p(r, r), CROSS_WIDTH, GLYPH);
                                line(cx, p(-r, r), p(r, -r), CROSS_WIDTH, GLYPH);
                            }),
                        })
                        .key(Key::from(format!("remove-{id}"))),
                    );
                }
            }
            if let Some(add) = bar.add {
                children.push(
                    Element::new(Part {
                        rect: kit_rect(add),
                        role: Some(Role::Button),
                        name: "Add Desktop".into(),
                        states: AccessibleStates::default(),
                        click: Some(input(OverviewInput::AddWorkspace)),
                        look_key: String::new(),
                        take_focus: false,
                        live: false,
                        paint: Box::new(|cx, look| {
                            let size = cx.size();
                            let all = Rect::new(0.0, 0.0, size.width, size.height);
                            fill(cx, all, SLOT_RADIUS, slot_fill(look));
                            if look.high_contrast {
                                outline(cx, all, SLOT_RADIUS, CONTRAST_OUTLINE_WIDTH, glyph(look));
                            }
                            let (cx_, cy) = (size.width / 2.0, size.height / 2.0);
                            let d = size.width.min(size.height) * PLUS_FRACTION;
                            let c = glyph(look);
                            line(
                                cx,
                                Point::new(cx_ - d, cy),
                                Point::new(cx_ + d, cy),
                                PLUS_WIDTH,
                                c,
                            );
                            line(
                                cx,
                                Point::new(cx_, cy - d),
                                Point::new(cx_, cy + d),
                                PLUS_WIDTH,
                                c,
                            );
                        }),
                    })
                    .key(Key::from("add")),
                );
            }
        }
        let thumbnails = self.thumbnails(now);
        if let Some(h) = ui.highlight(now) {
            let rect = if self.reduce_motion {
                // straight to the selected window, without the glide
                thumbnails
                    .iter()
                    .find(|(id, _)| u64::from(*id) == h.window.0)
                    .map(|(_, f)| f.inset(-OverviewConfig::default().highlight.inset))
                    .unwrap_or(h.rect)
            } else {
                h.rect
            };
            let opacity = h.opacity;
            children.push(Element::new(Part {
                rect: kit_rect(rect),
                role: None,
                name: String::new(),
                states: AccessibleStates::default(),
                click: None,
                look_key: format!("{opacity}"),
                take_focus: false,
                live: false,
                paint: Box::new(move |cx, look| {
                    let size = cx.size();
                    let (width, alpha) = if look.high_contrast {
                        (CONTRAST_HIGHLIGHT_WIDTH, 1.0)
                    } else {
                        (HIGHLIGHT_WIDTH, f64::from(HIGHLIGHT[3]))
                    };
                    let c = [HIGHLIGHT[0], HIGHLIGHT[1], HIGHLIGHT[2], alpha as f32];
                    outline(
                        cx,
                        Rect::new(0.0, 0.0, size.width, size.height),
                        HIGHLIGHT_RADIUS,
                        width,
                        faded(c, opacity),
                    );
                }),
            }));
        }
        let selection = ui.selection();
        for (id, frame) in thumbnails {
            let title = ui.title(id).unwrap_or_default().to_owned();
            let name = if title.is_empty() {
                UNTITLED.to_owned()
            } else {
                title.clone()
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
                    live: false,
                    // the server draws the window itself
                    paint: Box::new(|_, _| {}),
                })
                .key(Key::from(format!("window-{id}"))),
            );
            children.push(
                Element::new(Part {
                    rect: kit_rect(Self::title_row(frame)),
                    role: None,
                    look_key: title.clone(),
                    take_focus: false,
                    live: false,
                    name: String::new(),
                    states: AccessibleStates::default(),
                    click: None,
                    paint: Box::new(move |cx, look| title_paint(cx, look, &title)),
                })
                .key(Key::from(format!("title-{id}"))),
            );
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
}

fn slot_fill(look: Look) -> Rgba {
    match (look.opaque(), look.dark) {
        (false, false) => SLOT_LIGHT,
        (false, true) => SLOT_DARK,
        (true, false) => SLOT_OPAQUE_LIGHT,
        (true, true) => SLOT_OPAQUE_DARK,
    }
}

/// Labels and glyphs on a slot: white, or black on an opaque light slot.
fn glyph(look: Look) -> Rgba {
    if look.opaque() && !look.dark {
        GLYPH_ON_LIGHT
    } else {
        GLYPH
    }
}

fn current_border(look: Look) -> Rgba {
    if look.opaque() && !look.dark {
        GLYPH_ON_LIGHT
    } else {
        CURRENT_BORDER
    }
}

/// A window's title centred in its row: white with a soft shadow over the dim, on a dark plate under Increase
/// Contrast.
fn title_paint(cx: &mut cupertino_kit::core::PaintCx<'_>, look: Look, title: &str) {
    let size = cx.size();
    let row = Rect::new(0.0, 0.0, size.width, size.height);
    if look.high_contrast {
        let w = (wm_kit::text_width(cx, title, TITLE_PT, TITLE_WEIGHT) + 2.0 * TITLE_PLATE_PADDING)
            .min(size.width);
        let plate = Rect::new((size.width - w) / 2.0, 0.0, w, size.height);
        fill(cx, plate, size.height / 2.0, TITLE_PLATE);
    } else {
        let shadow = Rect::new(0.0, 1.0, size.width, size.height);
        centred_text(cx, title, shadow, TITLE_PT, TITLE_WEIGHT, TITLE_SHADOW);
    }
    centred_text(cx, title, row, TITLE_PT, TITLE_WEIGHT, [1.0, 1.0, 1.0, 1.0]);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The model keeps room under every window for its title; the title's row stays inside it, so a title never lies
    /// over the window below.
    #[test]
    fn the_title_row_fits_the_space_the_layout_keeps_under_a_window() {
        let space = OverviewConfig::default().layout.title_space;
        let frame = WmRect::new(10.0, 20.0, 300.0, 200.0);
        let row = OverviewSurface::title_row(frame);
        assert!(row.y >= frame.bottom());
        assert!(
            row.bottom() <= frame.bottom() + space + 1e-9,
            "{row:?} below the {space} pt under {frame:?}"
        );
    }
}
