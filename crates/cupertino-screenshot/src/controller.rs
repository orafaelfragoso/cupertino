//! One run of the tool without a display: the [`Session`] on the overlay, the Options menu and the settings it
//! changes, the moment the surfaces go before the capture, and where the capture goes (the thumbnail and the files, or
//! the clipboard). The process carries out the [`Effect`]s and shows [`Controller::windows`]; the tests drive the same
//! controller through the toolkit's headless window system.
//!
//! The phases follow each other: the overlay, while the user chooses ([`Phase::Selecting`]); the overlay showing the
//! arrow again so the pointer is not left hidden over what was under it ([`Phase::Leaving`]); no surface, while the
//! capture is copied ([`Phase::Capturing`]); then the thumbnail until it commits the files, or the clipboard pixel
//! until the offer is made, and the process serves it ([`Phase::Delivering`]). Escape, the close button and a
//! cancelled capture go through [`Phase::Leaving`] to [`Phase::Done`], with no surface and no keyboard.

use cupertino_geometry::{Point, Rect, Size};
use cupertino_kit::core::{next_resource_id, Window};
use tiny_skia::Pixmap;

use crate::image;
use crate::pick::PickWindow;
use crate::session::{Key, Outcome, Session, Style, Target};
use crate::settings::{Location, Mode, Settings};
use crate::thumbnail::{self, Phase as ThumbnailPhase, Thumbnail};
use crate::toolbar;
use crate::values::THUMBNAIL_SHADOW_OUTSET;
use crate::view::{
    self, Input, OverlayView, PointerInput, ThumbnailImage, ThumbnailInput, ThumbnailView,
    ToolbarView,
};

/// Seconds the overlay waits for a frame to show the arrow before it goes all the same (an output that is off draws
/// no frame).
pub const LEAVE_TIMEOUT: f64 = 0.1;
/// Seconds between the thumbnail's frames while it is dragged or slides away.
pub const FRAME: f64 = 1.0 / 60.0;
/// Pixels of the thumbnail's picture per point: enough for a 2x output, scaled down by the renderer below that.
pub const THUMBNAIL_DENSITY: f64 = 2.0;

/// What the process has to do for the controller.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Write [`Controller::settings`] to the settings file.
    StoreSettings,
    /// Copy `target`, with the pointer when `pointer`, once the tool's surfaces are gone; answer with
    /// [`Controller::shots`].
    Capture { target: Target, pointer: bool },
    /// Write [`Controller::images`] as files.
    Write,
    /// Offer the first of [`Controller::images`] on the clipboard while the clipboard pixel has the keyboard; answer
    /// with [`Controller::offered`] and, once another selection replaced it, [`Controller::clipboard_done`].
    Offer,
    /// End the process: cancelled or delivered, or the error.
    Exit(Result<(), String>),
}

/// How far the run is.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Selecting,
    /// The overlay shows the arrow before it goes, since `since`; then the capture, or nothing when `outcome` is a
    /// cancel.
    Leaving {
        outcome: Outcome,
        since: f64,
    },
    Capturing,
    /// The thumbnail is up, or the clipboard pixel waits for its offer.
    Delivering,
    /// The capture is on the clipboard, served by the process until it is replaced; no surface is left.
    Serving,
    Done,
}

/// The thumbnail on screen.
struct Shown {
    model: Thumbnail,
    image: ThumbnailImage,
    /// The picture's rectangle in its window, inside the room for the shadow, and the window's size.
    rect: Rect,
    size: Size,
    written: bool,
}

pub struct Controller {
    /// `None` when the run captures every output at once, with no overlay.
    session: Option<Session>,
    settings: Settings,
    screens: Vec<Rect>,
    /// The process was started to put the capture on the clipboard.
    clipboard: bool,
    phase: Phase,
    menu: bool,
    /// Control was held at the last press or release on the toolbar: the Capture button then sends to the clipboard.
    control: bool,
    images: Vec<Pixmap>,
    to_clipboard: bool,
    thumbnail: Option<Shown>,
}

impl Controller {
    /// A run over the outputs `screens` (by index, as the overlays and [`Target`] count them): with the overlay in
    /// `style`, or a capture of every output when `style` is `None`. The effects start it.
    pub fn new(
        style: Option<Style>,
        settings: Settings,
        screens: Vec<Rect>,
        clipboard: bool,
    ) -> (Self, Vec<Effect>) {
        let session = style.map(|style| match style {
            Style::Simple => Session::new(Style::Simple, Mode::Selection, screens.clone(), None, 0),
            Style::Interactive => Session::new(
                Style::Interactive,
                settings.mode,
                screens.clone(),
                settings.selection.filter(|_| settings.remember_selection),
                settings.timer,
            ),
        });
        let mut controller = Self {
            session,
            settings,
            screens,
            clipboard,
            phase: Phase::Selecting,
            menu: false,
            control: false,
            images: Vec::new(),
            to_clipboard: false,
            thumbnail: None,
        };
        let effects = if controller.session.is_none() {
            controller.capture(Target::Screens, clipboard)
        } else {
            Vec::new()
        };
        (controller, effects)
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    /// The captured images, upright, one per output for a capture of the whole screen.
    pub fn images(&self) -> &[Pixmap] {
        &self.images
    }

    /// Whether the Options menu is open.
    pub fn menu_open(&self) -> bool {
        self.menu
    }

    /// The windows on screen changed (the server's feed for the window pick).
    pub fn set_windows(&mut self, windows: Vec<PickWindow>) {
        if let Some(s) = &mut self.session {
            s.set_windows(windows);
        }
    }

    fn capture(&mut self, target: Target, clipboard: bool) -> Vec<Effect> {
        self.phase = Phase::Capturing;
        self.to_clipboard =
            clipboard || self.clipboard || self.settings.location == Location::Clipboard;
        vec![Effect::Capture {
            target,
            pointer: self.settings.show_pointer,
        }]
    }

    /// The session ended with `outcome`: the overlay shows the arrow and goes. A remembered selection is kept for
    /// the next toolbar.
    fn leave(&mut self, outcome: Outcome, now: f64) -> Vec<Effect> {
        let mut effects = Vec::new();
        if let (
            Outcome::Capture {
                target: Target::Area { rect, .. },
                ..
            },
            Some(Style::Interactive),
        ) = (&outcome, self.session.as_ref().map(Session::style))
        {
            if self.settings.remember_selection && self.settings.selection != Some(*rect) {
                self.settings.selection = Some(*rect);
                effects.push(Effect::StoreSettings);
            }
        }
        self.menu = false;
        self.phase = Phase::Leaving {
            outcome,
            since: now,
        };
        effects
    }

    /// The overlay showed the arrow, or waited long enough for it.
    fn left(&mut self) -> Vec<Effect> {
        let Phase::Leaving { outcome, .. } = std::mem::replace(&mut self.phase, Phase::Done) else {
            return Vec::new();
        };
        match outcome {
            Outcome::Capture { target, clipboard } => self.capture(target, clipboard),
            Outcome::Cancel => vec![Effect::Exit(Ok(()))],
        }
    }

    /// What the user did on a surface, at `now` (seconds).
    pub fn input(&mut self, input: Input, now: f64) -> Vec<Effect> {
        match input {
            Input::Closed => return self.fail("the server closed a screenshot surface".into()),
            Input::ArrowShown => return self.left(),
            Input::Thumbnail(input) => return self.thumbnail_input(input, now),
            _ => {}
        }
        if self.phase != Phase::Selecting {
            return Vec::new();
        }
        let Some(session) = self.session.as_mut() else {
            return Vec::new();
        };
        let mut effects = Vec::new();
        let outcome = match input {
            Input::Pointer(PointerInput::Moved(p)) => {
                session.pointer_moved(p);
                None
            }
            Input::Pointer(PointerInput::Button {
                pressed,
                at,
                control,
            }) => session.button(pressed, at, control, now),
            Input::Key(key, mods) => {
                // Escape closes the open menu first, as the toolkit's menus do
                if key == Key::Escape && self.menu {
                    self.menu = false;
                    return Vec::new();
                }
                session.key(key, mods, now)
            }
            Input::Mode(mode) => {
                session.set_mode(mode);
                if self.settings.mode != session.mode() {
                    self.settings.mode = session.mode();
                    effects.push(Effect::StoreSettings);
                }
                None
            }
            Input::Control(held) => {
                self.control = held;
                None
            }
            Input::Capture => session.capture(self.control, now),
            Input::Cancel => Some(Outcome::Cancel),
            Input::Options(open) => {
                self.menu = open && !session.counting_down();
                None
            }
            Input::Choose(action) => {
                toolbar::apply(&mut self.settings, &action);
                session.set_timer(self.settings.timer);
                self.menu = false;
                effects.push(Effect::StoreSettings);
                None
            }
            Input::Closed | Input::ArrowShown | Input::Thumbnail(_) => None,
        };
        if let Some(outcome) = outcome {
            effects.extend(self.leave(outcome, now));
        }
        effects
    }

    /// Advance to `now`: the countdown's capture, the overlay that waited too long for its arrow, the thumbnail's
    /// expiry and slide.
    pub fn tick(&mut self, now: f64) -> Vec<Effect> {
        match &self.phase {
            Phase::Selecting => match self.session.as_mut().and_then(|s| s.tick(now)) {
                Some(outcome) => self.leave(outcome, now),
                None => Vec::new(),
            },
            Phase::Leaving { since, .. } if now - since >= LEAVE_TIMEOUT => self.left(),
            Phase::Delivering => {
                let Some(shown) = self.thumbnail.as_mut() else {
                    return Vec::new();
                };
                shown.model.tick(now);
                self.after_thumbnail(now)
            }
            _ => Vec::new(),
        }
    }

    /// When [`Controller::tick`] next has something to do, in seconds from the start.
    pub fn deadline(&self, now: f64) -> Option<f64> {
        match &self.phase {
            Phase::Selecting => self.session.as_ref()?.deadline(now),
            Phase::Leaving { since, .. } => Some(since + LEAVE_TIMEOUT),
            Phase::Delivering => {
                let shown = self.thumbnail.as_ref()?;
                Some(shown.model.deadline().unwrap_or(now + FRAME))
            }
            _ => None,
        }
    }

    /// The copy of the capture arrived, its images upright; on the first output for the thumbnail.
    pub fn shots(&mut self, shots: Result<Vec<Pixmap>, String>, now: f64) -> Vec<Effect> {
        if self.phase != Phase::Capturing {
            return Vec::new();
        }
        let images = match shots {
            Ok(images) if !images.is_empty() => images,
            Ok(_) => return self.fail("the capture has no image".to_owned()),
            Err(e) => return self.fail(e),
        };
        self.images = images;
        self.phase = Phase::Delivering;
        if self.to_clipboard {
            return vec![Effect::Offer];
        }
        if !self.settings.show_thumbnail {
            self.phase = Phase::Done;
            return vec![Effect::Write, Effect::Exit(Ok(()))];
        }
        let first = &self.images[0];
        let screen = self.screens.first().copied().unwrap_or_default();
        let (w, h) = thumbnail::size(
            (f64::from(first.width()), f64::from(first.height())),
            (screen.width(), screen.height()),
        );
        let pixels = (
            ((w * THUMBNAIL_DENSITY).round() as u32).clamp(1, first.width()),
            ((h * THUMBNAIL_DENSITY).round() as u32).clamp(1, first.height()),
        );
        let Some(small) = image::downscale(first, pixels.0, pixels.1) else {
            self.phase = Phase::Done;
            return vec![Effect::Write, Effect::Exit(Ok(()))];
        };
        let outset = THUMBNAIL_SHADOW_OUTSET;
        self.thumbnail = Some(Shown {
            model: Thumbnail::new(w, now),
            image: ThumbnailImage::new(
                next_resource_id(),
                small.width(),
                small.height(),
                small.data().to_vec(),
            ),
            rect: Rect::new(outset, outset, w, h),
            size: Size::new((w + 2.0 * outset).ceil(), (h + 2.0 * outset).ceil()),
            written: false,
        });
        Vec::new()
    }

    fn fail(&mut self, problem: String) -> Vec<Effect> {
        self.phase = Phase::Done;
        vec![Effect::Exit(Err(problem))]
    }

    fn thumbnail_input(&mut self, input: ThumbnailInput, now: f64) -> Vec<Effect> {
        let Some(shown) = self.thumbnail.as_mut() else {
            return Vec::new();
        };
        let local = |p: Point| Point::new(p.x - shown.rect.x(), p.y - shown.rect.y());
        match input {
            ThumbnailInput::Press(p) => shown.model.press(local(p), now),
            ThumbnailInput::Motion(p) => shown.model.motion(local(p), now),
            ThumbnailInput::Release => shown.model.release(now),
            ThumbnailInput::Activate => {
                shown.model.press(Point::ZERO, now);
                shown.model.release(now);
            }
        }
        self.after_thumbnail(now)
    }

    /// The files are written when the thumbnail commits, and the run ends when it has slid away.
    fn after_thumbnail(&mut self, now: f64) -> Vec<Effect> {
        let Some(shown) = self.thumbnail.as_mut() else {
            return Vec::new();
        };
        let mut effects = Vec::new();
        if !shown.written && shown.model.committed().is_some() {
            shown.written = true;
            effects.push(Effect::Write);
        }
        if shown.model.phase(now) == ThumbnailPhase::Gone {
            self.thumbnail = None;
            self.phase = Phase::Done;
            effects.push(Effect::Exit(Ok(())));
        }
        effects
    }

    /// The offer is on the clipboard: the pixel that held the keyboard for it goes.
    pub fn offered(&mut self) {
        if self.phase == Phase::Delivering && self.to_clipboard {
            self.phase = Phase::Serving;
        }
    }

    /// Another selection replaced the offer, or it could not be made.
    pub fn clipboard_done(&mut self, result: Result<(), String>) -> Vec<Effect> {
        self.phase = Phase::Done;
        vec![Effect::Exit(result)]
    }

    /// The tool's windows at `now`, their messages made by `to`. The overlay of each output and the toolbar on the
    /// first while the user chooses; the overlays alone while they go; then the thumbnail or the clipboard pixel.
    pub fn windows<M: Clone + 'static>(&self, now: f64, to: fn(Input) -> M) -> Vec<Window<M>> {
        let overlays = |leaving: bool| {
            let session = self.session.as_ref().expect("an overlay runs a session");
            // the direct shortcut's first overlay takes the keys; the toolbar takes them otherwise
            let keys = session.style() == Style::Simple;
            self.screens
                .iter()
                .enumerate()
                .map(|(i, bounds)| {
                    view::overlay(
                        i,
                        OverlayView::of(session, *bounds, keys && i == 0, leaving),
                        to,
                    )
                })
                .collect::<Vec<_>>()
        };
        match &self.phase {
            Phase::Selecting => {
                let mut windows = overlays(false);
                let session = self.session.as_ref().expect("an overlay runs a session");
                if session.style() == Style::Interactive {
                    windows.push(view::toolbar(
                        ToolbarView {
                            mode: session.mode(),
                            countdown: session.countdown(now),
                        },
                        to,
                    ));
                    if self.menu {
                        windows.extend(view::options_menu(&self.settings, to));
                    }
                }
                windows
            }
            Phase::Leaving { .. } => overlays(true),
            Phase::Delivering if self.to_clipboard => vec![view::clipboard_focus(to)],
            Phase::Delivering => match &self.thumbnail {
                Some(shown) => {
                    let look = match shown.model.phase(now) {
                        ThumbnailPhase::Shown(look) | ThumbnailPhase::Dismissing(look) => look,
                        ThumbnailPhase::Gone => return Vec::new(),
                    };
                    vec![view::thumbnail(
                        ThumbnailView {
                            image: shown.image.clone(),
                            rect: shown.rect,
                            look,
                        },
                        shown.size,
                        to,
                    )]
                }
                None => Vec::new(),
            },
            Phase::Capturing | Phase::Serving | Phase::Done => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::KeyMods;
    use crate::toolbar::MenuAction;

    const OUTPUT: Rect = Rect::new(0.0, 0.0, 800.0, 600.0);

    fn toolbar(settings: Settings) -> Controller {
        Controller::new(Some(Style::Interactive), settings, vec![OUTPUT], false).0
    }

    fn drag(c: &mut Controller, from: Point, to: Point) {
        for input in [
            PointerInput::Moved(from),
            PointerInput::Button {
                pressed: true,
                at: from,
                control: false,
            },
            PointerInput::Moved(to),
            PointerInput::Button {
                pressed: false,
                at: to,
                control: false,
            },
        ] {
            c.input(Input::Pointer(input), 0.0);
        }
    }

    #[test]
    fn a_capture_waits_for_the_arrow_and_the_timer_from_the_menu_counts_first() {
        let mut c = toolbar(Settings::default());
        c.input(Input::Options(true), 0.0);
        assert!(c.menu_open());
        assert_eq!(
            c.input(Input::Choose(MenuAction::Timer(5)), 0.0),
            [Effect::StoreSettings]
        );
        assert!(!c.menu_open(), "a choice closes the menu");
        drag(&mut c, Point::new(10.0, 10.0), Point::new(110.0, 60.0));
        assert_eq!(c.input(Input::Capture, 1.0), []);
        assert_eq!(
            c.deadline(1.5),
            Some(2.0),
            "the countdown ticks each second"
        );
        assert_eq!(c.tick(5.9), []);
        // the countdown ran out: the selection is remembered and the overlay leaves
        assert_eq!(c.tick(6.0), [Effect::StoreSettings]);
        assert_eq!(
            c.settings().selection,
            Some(Rect::new(10.0, 10.0, 100.0, 50.0))
        );
        assert!(matches!(c.phase(), Phase::Leaving { .. }));
        let windows = c.windows::<Input>(6.0, |i| i);
        assert_eq!(
            windows.len(),
            1,
            "the toolbar goes first, the overlay shows the arrow"
        );
        assert_eq!(
            c.input(Input::ArrowShown, 6.01),
            [Effect::Capture {
                target: Target::Area {
                    output: 0,
                    rect: Rect::new(10.0, 10.0, 100.0, 50.0)
                },
                pointer: false
            }]
        );
        assert!(c.windows::<Input>(6.01, |i| i).is_empty());
    }

    #[test]
    fn an_overlay_that_draws_no_frame_still_goes_after_the_leave_timeout() {
        let mut c = toolbar(Settings::default());
        assert_eq!(c.input(Input::Key(Key::Escape, KeyMods::NONE), 1.0), []);
        assert_eq!(c.deadline(1.0), Some(1.0 + LEAVE_TIMEOUT));
        assert_eq!(c.tick(1.0 + LEAVE_TIMEOUT), [Effect::Exit(Ok(()))]);
        assert_eq!(c.phase(), &Phase::Done);
    }

    #[test]
    fn control_held_on_the_toolbar_sends_the_capture_to_the_clipboard() {
        let settings = Settings {
            mode: Mode::Screen,
            ..Settings::default()
        };
        let mut c = toolbar(settings);
        c.input(Input::Control(true), 0.0);
        c.input(Input::Capture, 0.0);
        c.input(Input::ArrowShown, 0.0);
        assert_eq!(
            c.shots(Ok(vec![Pixmap::new(4, 4).unwrap()]), 0.0),
            [Effect::Offer]
        );
        assert_eq!(
            c.windows::<Input>(0.0, |i| i).len(),
            1,
            "the clipboard pixel"
        );
        c.offered();
        assert!(c.windows::<Input>(0.0, |i| i).is_empty());
        assert_eq!(c.clipboard_done(Ok(())), [Effect::Exit(Ok(()))]);
    }

    #[test]
    fn a_failed_capture_ends_the_run_with_its_error_and_a_closed_surface_too() {
        let (mut c, effects) = Controller::new(None, Settings::default(), vec![OUTPUT], false);
        assert_eq!(
            effects,
            [Effect::Capture {
                target: Target::Screens,
                pointer: false
            }]
        );
        assert_eq!(
            c.shots(Err("gone".into()), 0.0),
            [Effect::Exit(Err("gone".into()))]
        );
        let mut c = toolbar(Settings::default());
        assert!(matches!(
            c.input(Input::Closed, 0.0).as_slice(),
            [Effect::Exit(Err(_))]
        ));
        assert!(c.windows::<Input>(0.0, |i| i).is_empty());
    }

    #[test]
    fn a_forgotten_selection_is_not_saved() {
        let settings = Settings {
            remember_selection: false,
            ..Settings::default()
        };
        let mut c = toolbar(settings);
        drag(&mut c, Point::new(10.0, 10.0), Point::new(110.0, 60.0));
        assert_eq!(c.input(Input::Key(Key::Return, KeyMods::NONE), 0.0), []);
        assert_eq!(c.settings().selection, None);
    }
}
