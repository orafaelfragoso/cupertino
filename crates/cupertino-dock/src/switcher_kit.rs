//! The application switcher's panel on CupertinoKit: one window per switch, as large as the panel, that the dock's
//! roles put on an overlay layer surface centred on the output the server named (namespace [`APP_ID`], no keyboard:
//! the server keeps the keys of the switch). The panel shows after its delay, draws the selection, one icon per
//! application and the selected application's name over the `Switcher` material, and fades out after a commit. The
//! pointer selects by hovering (once it left its first box) or pressing, and a release activates the entry under it or
//! cancels the switch; what it asks for goes back as [`SwitcherRequest`]s for the dock role. The model is
//! [`crate::switcher`]; nothing here speaks Wayland.

use crate::switcher::{HoverGate, Switcher, SwitcherLayout};
use crate::wm_kit::{self, faded, fill, ImageSource, Look, Part, PointerInput, Rgba, Root};
use cupertino_kit::core::{
    AccessibleStates, CommandRegistry, Element, Key, Role, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::platform::{DecorationMode, WindowSpec};
use cupertino_kit::theme::Surface;
use cupertino_wm::geometry::Point as WmPoint;

/// The `app_id` of the panel's window, and the namespace of its layer surface.
pub const APP_ID: &str = "cupertino-dock-switcher";

/// What assistive technology calls the panel.
const TITLE: &str = "Application Switcher";

/// How often the fade is redrawn, in seconds.
pub const FRAME: f64 = 1.0 / 60.0;

/// Weight of the selected application's name (regular).
const LABEL_WEIGHT: u16 = 400;

/// One entry of the server's list: the application id and the name the panel shows (the desktop entry's name, else
/// the id).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitcherEntry {
    pub app_id: String,
    pub name: String,
}

/// What reaches the panel from its window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SwitcherInput {
    Pointer(PointerInput),
    /// Assistive technology clicked entry `i`.
    Activate(usize),
}

/// What the panel asks the server for (the dock role's requests of the same names).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitcherRequest {
    /// `switcher_select`
    Select(usize),
    /// `switcher_activate`
    Activate(usize),
    /// `switcher_cancel`
    Cancel,
}

/// The switch on screen.
#[derive(Clone, Debug)]
struct Open {
    /// Opaque to the panel: the output the server named, compared to tell a step from a new switch.
    output: u32,
    switcher: Switcher,
    names: Vec<String>,
    layout: SwitcherLayout,
    hover: HoverGate,
    /// Entry the last `Select` asked for, until the server answers.
    requested: Option<usize>,
    /// Counts the switches, so each gets a window of its own.
    generation: u64,
}

/// The switcher's panel across switches. The server's events and the window's input come in, the window and the
/// requests go out.
#[derive(Clone, Debug, Default)]
pub struct SwitcherSurface {
    open: Option<Open>,
    /// The switch whose window the server closed, kept for its next step.
    orphan: Option<Open>,
    generation: u64,
}

impl SwitcherSurface {
    pub fn new() -> Self {
        Self::default()
    }

    /// The server sent the list and the selection for `output`, an output of `output_size` points: a step of the
    /// switch on screen moves the selection, anything else is a new switch. An error when the server broke the
    /// protocol (an empty list or a selection outside it).
    pub fn changed(
        &mut self,
        output: u32,
        output_size: (f64, f64),
        entries: Vec<SwitcherEntry>,
        selected: usize,
        now: f64,
    ) -> Result<(), &'static str> {
        let apps: Vec<String> = entries.iter().map(|e| e.app_id.clone()).collect();
        if let Some(open) = self
            .open
            .as_mut()
            .filter(|o| o.output == output && o.switcher.continues_with(&apps))
        {
            open.switcher.select(selected)?;
            open.requested = None;
            open.names = entries.into_iter().map(|e| e.name).collect();
            return Ok(());
        }
        let resumed = self
            .orphan
            .take()
            .filter(|o| o.output == output && o.switcher.continues_with(&apps));
        self.generation += 1;
        let layout = SwitcherLayout::new(apps.len(), output_size.0, output_size.1);
        let names = entries.into_iter().map(|e| e.name).collect();
        let switcher = match resumed {
            Some(mut open) => {
                open.switcher.select(selected)?;
                open.switcher
            }
            None => Switcher::new(apps, selected, now)?,
        };
        self.open = Some(Open {
            output,
            switcher,
            names,
            layout,
            hover: HoverGate::default(),
            requested: None,
            generation: self.generation,
        });
        Ok(())
    }

    /// The switch ended. A panel on screen fades out after a commit; a cancel, or a commit before the panel showed,
    /// removes it at once.
    pub fn closed(&mut self, committed: bool, now: f64) {
        self.orphan = None;
        match self.open.as_mut() {
            Some(open) if committed && open.switcher.is_shown(now) => open.switcher.close(now),
            _ => self.open = None,
        }
    }

    /// The server closed the panel's window: the switch waits for its next step, which opens a new window.
    pub fn lost(&mut self) {
        self.orphan = self.open.take();
    }

    /// The output the panel is on, while a switch is shown or waits for its delay.
    pub fn output(&self) -> Option<u32> {
        self.open.as_ref().map(|o| o.output)
    }

    /// The panel's rectangle on its output, in points.
    pub fn panel(&self) -> Option<Rect> {
        self.open.as_ref().map(|o| wm_kit::kit_rect(o.layout.panel))
    }

    /// The selected entry of the switch.
    pub fn selected(&self) -> Option<usize> {
        self.open.as_ref().map(|o| o.switcher.selected())
    }

    /// What the window reported; the requests for the server.
    pub fn input(&mut self, input: SwitcherInput, now: f64) -> Vec<SwitcherRequest> {
        let Some(open) = self.open.as_mut().filter(|o| !o.switcher.is_fading()) else {
            return Vec::new();
        };
        if !open.switcher.is_shown(now) {
            return Vec::new();
        }
        let n = open.switcher.apps().len();
        // the window covers only the panel: its origin is the panel's in output points
        let at = |p: cupertino_kit::geometry::Point| {
            WmPoint::new(p.x + open.layout.panel.x, p.y + open.layout.panel.y)
        };
        let (position, pressed) = match input {
            SwitcherInput::Activate(entry) if entry < n => {
                return vec![SwitcherRequest::Activate(entry)]
            }
            SwitcherInput::Activate(_) | SwitcherInput::Pointer(PointerInput::Left) => {
                return Vec::new()
            }
            SwitcherInput::Pointer(PointerInput::Released(p)) => {
                return vec![match open.layout.hit(at(p), n) {
                    Some(entry) => SwitcherRequest::Activate(entry),
                    None => SwitcherRequest::Cancel,
                }];
            }
            SwitcherInput::Pointer(PointerInput::Moved(p)) => (at(p), false),
            SwitcherInput::Pointer(PointerInput::Pressed(p)) => (at(p), true),
        };
        let armed = open.hover.moved(position);
        let Some(entry) = open.layout.hit(position, n) else {
            return Vec::new();
        };
        if (pressed || armed) && entry != open.switcher.selected() && open.requested != Some(entry)
        {
            open.requested = Some(entry);
            return vec![SwitcherRequest::Select(entry)];
        }
        Vec::new()
    }

    /// When the panel next changes by itself, in the same clock as `now`: when its delay ends, or the next frame of
    /// its fade. `None` while nothing is due.
    pub fn next_wake(&self, now: f64) -> Option<f64> {
        let open = self.open.as_ref()?;
        if !open.switcher.is_shown(now) {
            return Some(now + open.switcher.until_shown(now));
        }
        open.switcher.is_fading().then_some(now + FRAME)
    }

    /// Advance the clock: a finished fade ends the panel. Call before [`Self::window`] on every timer.
    pub fn tick(&mut self, now: f64) {
        if self.open.as_ref().is_some_and(|o| o.switcher.is_gone(now)) {
            self.open = None;
        }
    }

    /// Whether no panel is shown or waiting.
    pub fn is_done(&self, now: f64) -> bool {
        self.open.as_ref().is_none_or(|o| o.switcher.is_gone(now))
    }

    /// The panel's window at `now`, from its delay to the end of its fade. `icon` gives the renditions of an
    /// application's icon (the generic application icon for one without).
    pub fn window<M: Clone + 'static>(
        &self,
        now: f64,
        icon: &dyn Fn(&str) -> Vec<ImageSource>,
        input: fn(SwitcherInput) -> M,
    ) -> Option<Window<M>> {
        let open = self.open.as_ref()?;
        if !open.switcher.is_shown(now) || open.switcher.is_gone(now) {
            return None;
        }
        let layout = open.layout;
        let size = Size::new(layout.panel.w, layout.panel.h);
        let alpha = open.switcher.alpha(now);
        let local = |r: cupertino_wm::geometry::Rect| {
            wm_kit::kit_rect(r.translate(-layout.panel.x, -layout.panel.y))
        };
        let selected = open.switcher.selected();
        let mut children = Vec::new();
        let selection = local(layout.selection(selected));
        children.push(Element::new(Part {
            rect: selection,
            role: None,
            name: String::new(),
            states: AccessibleStates::default(),
            click: None,
            look_key: "selection".into(),
            take_focus: false,
            live: false,
            paint: Box::new(|cx, look| {
                let size = cx.size();
                fill(
                    cx,
                    Rect::new(0.0, 0.0, size.width, size.height),
                    SwitcherLayout::SELECTION_RADIUS,
                    selection_fill(look),
                );
            }),
        }));
        for (i, app) in open.switcher.apps().iter().enumerate() {
            let sources = icon(app);
            let name = open.names.get(i).cloned().unwrap_or_else(|| app.clone());
            children.push(
                Element::new(Part {
                    rect: local(layout.item(i)),
                    role: Some(Role::ListItem),
                    look_key: format!("{app}:{}", sources.len()),
                    take_focus: false,
                    live: false,
                    name,
                    states: AccessibleStates {
                        selected: i == selected,
                        ..AccessibleStates::default()
                    },
                    click: Some(input(SwitcherInput::Activate(i))),
                    paint: Box::new(move |cx, _| {
                        let size = cx.size();
                        wm_kit::icon(cx, &sources, Rect::new(0.0, 0.0, size.width, size.height));
                    }),
                })
                .key(Key::from(format!("entry-{i}"))),
            );
        }
        let name = open
            .names
            .get(selected)
            .cloned()
            .unwrap_or_else(|| open.switcher.apps()[selected].clone());
        // the label's row spans the panel between its insets; where the name sits in it is the model's rule for the
        // width the text measures, which only the toolkit's text system knows, so it is placed where it draws
        let row = layout.label(selected, layout.panel.w);
        let label = local(cupertino_wm::geometry::Rect::new(
            layout.panel.x + SwitcherLayout::INSET_X,
            row.y,
            layout.panel.w - 2.0 * SwitcherLayout::INSET_X,
            row.h,
        ));
        children.push(Element::new(Part {
            rect: label,
            role: Some(Role::StaticText),
            look_key: format!("{name}@{selected}"),
            take_focus: false,
            // the panel never has the keyboard, so each step announces the selected application
            live: true,
            name: name.clone(),
            states: AccessibleStates::default(),
            click: None,
            paint: Box::new(move |cx, look| {
                let width =
                    wm_kit::text_width(cx, &name, SwitcherLayout::LABEL_FONT_PT, LABEL_WEIGHT);
                let text = layout.label(selected, width);
                let rect = Rect::new(
                    text.x - layout.panel.x - label.origin.x,
                    0.0,
                    text.w,
                    cx.size().height,
                );
                wm_kit::centred_text(
                    cx,
                    &name,
                    rect,
                    SwitcherLayout::LABEL_FONT_PT,
                    LABEL_WEIGHT,
                    label_color(look),
                );
            }),
        }));
        Some(Window {
            key: WindowKey(Key::from(format!("switcher-{}", open.generation))),
            spec: WindowSpec {
                title: TITLE.to_owned(),
                app_id: APP_ID.to_owned(),
                size,
                min_size: None,
                max_size: None,
                decorations: DecorationMode::None,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Surface {
                surface: Surface::Switcher,
                radius: SwitcherLayout::RADIUS,
                alpha,
            },
            content: Element::new(Root {
                role: Role::List,
                name: TITLE.to_owned(),
                opacity: alpha,
                pointer: Some(Box::new(move |p| input(SwitcherInput::Pointer(p)))),
                key: None,
                children,
            }),
            popup: None,
            menus: None,
        })
    }
}

/// The selection's fill: the switcher's own colours over its material; over the opaque fill of Reduce Transparency or
/// Increase Contrast a stronger one, so the selection stays visible.
fn selection_fill(look: Look) -> Rgba {
    let base = SwitcherLayout::selection_fill(look.dark);
    if look.high_contrast {
        if look.dark {
            [1.0, 1.0, 1.0, 0.35]
        } else {
            [0.0, 0.0, 0.0, 0.3]
        }
    } else if look.reduce_transparency {
        faded(base, 1.2)
    } else {
        base
    }
}

/// The label's colour: the switcher's own gray, or full black or white under Increase Contrast.
fn label_color(look: Look) -> Rgba {
    match (look.high_contrast, look.dark) {
        (true, true) => [1.0, 1.0, 1.0, 1.0],
        (true, false) => [0.0, 0.0, 0.0, 1.0],
        (false, dark) => SwitcherLayout::label_color(dark),
    }
}
