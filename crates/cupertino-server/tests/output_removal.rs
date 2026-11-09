//! Displays going away and coming back against a headless compositor with two offscreen displays of different scales:
//! `headless-1` (1000 × 700 pixels at scale 1) and, right of it, `headless-2` (1600 × 1000 pixels at scale 2, so
//! 800 × 500 points). Whatever was on the display that goes stays reachable on the other: windows (normal, full
//! screen, being dragged), the pointer, a locked session's keyboard and an overview. A display that comes back gets the
//! windows it lost that were not moved since. Tests skip (pass) when no EGL device is available.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{Client, TOPLEVEL_BLUE};
use cupertino_overview::OverviewMode;
use cupertino_server::backend::headless::{self, HeadlessDisplay};
use cupertino_server::config::Config;
use cupertino_server::displays::HeadId;
use cupertino_server::state::PointerMode;
use cupertino_server::Cupertino;
use cupertino_wm::{Command, LifecycleState, OutputId, Point as MPoint, Rect, Requester, WindowId};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::wayland::compositor::with_states;
use smithay::wayland::fractional_scale::with_fractional_scale;

const LEFT: (i32, i32) = (1000, 700);
const RIGHT: (i32, i32) = (1600, 1000);
const RIGHT_SCALE: f64 = 2.0;

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

fn right_display() -> HeadlessDisplay {
    HeadlessDisplay {
        name: "headless-2".into(),
        size: RIGHT,
        physical_mm: (0, 0),
    }
}

impl Harness {
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("creating the event loop");
        let display: Display<Cupertino> = Display::new().expect("creating the Wayland display");
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .expect("creating the compositor");
        let left = HeadlessDisplay {
            name: "headless-1".into(),
            size: LEFT,
            physical_mm: (0, 0),
        };
        if let Err(e) = headless::init_displays(&mut state, &[left], Some(1.0)) {
            common::egl_unavailable(format!("{e:#}"));
            return None;
        }
        headless::add_display(&mut state, &right_display(), Some(RIGHT_SCALE)).unwrap();
        Some(Harness { state, event_loop })
    }

    fn step(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(5),
        )
        .unwrap();
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            self.step();
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// Let every animation finish.
    fn settle(&mut self) {
        let time = self.state.now() + 5.0;
        common::set_time(&mut self.state, &mut self.event_loop, time);
        self.state.tick();
        self.step();
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    fn output(&self, name: &str) -> OutputId {
        self.state
            .outputs
            .iter()
            .find(|o| o.output.name() == name)
            .unwrap_or_else(|| panic!("no output {name}"))
            .id
    }

    fn head(&self, name: &str) -> HeadId {
        self.state
            .displays
            .heads
            .iter()
            .find(|h| h.info.connector == name)
            .unwrap()
            .id
    }

    fn usable(&self, output: OutputId) -> Rect {
        self.state.wm.output(output).unwrap().usable
    }

    /// Map one more toplevel; it is placed on the active output.
    fn map(&mut self, clients: &mut Vec<Client>) -> WindowId {
        let before: Vec<WindowId> = self.state.wm.windows().map(|w| w.id).collect();
        clients.push(Client::toplevel(self.socket_path()));
        let mut new = None;
        self.wait_until("the toplevel to be mapped", |s| {
            new =
                s.wm.windows()
                    .map(|w| w.id)
                    .find(|w| !before.contains(w) && s.entry(*w).is_some_and(|e| e.mapped));
            new.is_some()
        });
        self.settle();
        new.unwrap()
    }

    fn put(&mut self, window: WindowId, output: OutputId, frame: Rect) {
        self.state.command(Command::MoveWindowToOutput {
            window,
            output,
            frame,
        });
        self.settle();
    }

    fn frame(&self, window: WindowId) -> Rect {
        self.state.wm.window(window).unwrap().geometry()
    }

    fn output_of(&self, window: WindowId) -> OutputId {
        self.state.wm.window(window).unwrap().output
    }

    /// The fractional scale the compositor told the window's surface.
    fn told_scale(&self, window: WindowId) -> Option<f64> {
        let surface = self.state.entry(window).unwrap().surface().clone();
        with_states(&surface, |states| {
            with_fractional_scale(states, |fs| fs.preferred_scale())
        })
    }

    fn move_pointer(&mut self, x: f64, y: f64) {
        let time = (self.state.now() * 1000.0) as u32;
        self.state.pointer_moved_to((x, y).into(), time, None);
    }

    fn pointer(&self) -> (f64, f64) {
        let p = self.state.pointer.current_location();
        (p.x, p.y)
    }

    /// Drag window `w` by its title bar from `from` to `to`, as the person does.
    fn drag(&mut self, w: WindowId, from: (f64, f64), to: (f64, f64)) {
        self.move_pointer(from.0, from.1);
        self.state.begin_model_interaction(Command::BeginMove {
            window: w,
            pointer: MPoint::new(from.0, from.1),
        });
        self.move_pointer((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
        self.move_pointer(to.0, to.1);
        self.release();
    }

    fn release(&mut self) {
        self.state.command(Command::PointerRelease);
        self.state.pointer_mode = PointerMode::Normal;
        self.settle();
    }

    fn unplug(&mut self, name: &str) {
        let head = self.head(name);
        headless::remove_display(&mut self.state, head).unwrap();
        self.step();
    }
}

impl Harness {
    /// How many pixels of output `name`'s next frame, drawn as the screen shows it, have `colour` (a test buffer's
    /// colour, as `common` writes it).
    fn pixels_of(&mut self, name: &str, colour: [u8; 4]) -> usize {
        let idx = self
            .state
            .outputs
            .iter()
            .position(|o| o.output.name() == name)
            .unwrap();
        assert_eq!(
            self.state.power_overlay_alpha(),
            None,
            "nothing dims the frame"
        );
        let mut backend = self.state.backend.take().unwrap();
        let image = self
            .state
            .draw_output_image(backend.renderer(), idx, true, false)
            .unwrap();
        self.state.backend = Some(backend);
        image
            .pixels()
            .filter(|p| p.0[0] == colour[2] && p.0[1] == colour[1] && p.0[2] == colour[0])
            .count()
    }
}

fn inside(r: Rect, area: Rect) -> bool {
    r.x >= area.x - 1e-6
        && r.y >= area.y - 1e-6
        && r.right() <= area.right() + 1e-6
        && r.bottom() <= area.bottom() + 1e-6
}

fn on_output(h: &Harness, p: (f64, f64)) -> Option<OutputId> {
    h.state
        .outputs
        .iter()
        .find(|o| o.rect.contains(MPoint::new(p.0, p.1)))
        .map(|o| o.id)
}

/// A window on the display that goes comes onto the remaining one inside its usable area at that display's scale; the
/// pointer that was on it comes along. When the display comes back the window goes back to the same place on it and
/// to its scale, while a window the person moved meanwhile stays where they put it.
#[test]
fn windows_of_a_display_that_goes_come_back_to_it_unless_they_were_moved() {
    let Some(mut h) = Harness::new() else { return };
    let mut clients = Vec::new();
    let (left, right) = (h.output("headless-1"), h.output("headless-2"));
    let stays = h.map(&mut clients);
    let moved = h.map(&mut clients);
    let at = Rect::new(1100.0, 100.0, 400.0, 300.0);
    h.put(stays, right, at);
    h.put(moved, right, Rect::new(1300.0, 150.0, 400.0, 300.0));
    assert_eq!(h.frame(stays), at);
    assert_eq!(h.told_scale(stays), Some(RIGHT_SCALE));
    h.move_pointer(1500.0, 300.0);

    h.unplug("headless-2");
    assert_eq!(h.state.outputs.len(), 1);
    for w in [stays, moved] {
        assert_eq!(h.output_of(w), left);
        assert!(
            inside(h.frame(w), h.usable(left)),
            "{w:?} at {:?}",
            h.frame(w)
        );
        assert_eq!(h.told_scale(w), Some(1.0), "the remaining display's scale");
    }
    assert_eq!(
        on_output(&h, h.pointer()),
        Some(left),
        "the pointer is reachable"
    );
    // the person moves one window meanwhile
    let g = h.frame(moved);
    h.drag(moved, (g.x + 100.0, g.y + 10.0), (g.x + 40.0, g.y + 150.0));
    assert_ne!(h.frame(moved), g);

    headless::add_display(&mut h.state, &right_display(), Some(RIGHT_SCALE)).unwrap();
    h.settle();
    let right = h.output("headless-2");
    let back = h.state.output_entry(right).unwrap().rect;
    assert_eq!(h.output_of(stays), right, "the untouched window went back");
    assert_eq!(
        h.frame(stays),
        Rect::new(back.x + 100.0, back.y + 100.0, 400.0, 300.0)
    );
    assert_eq!(h.told_scale(stays), Some(RIGHT_SCALE));
    assert_eq!(h.output_of(moved), left, "the moved window stays");
    assert!(h.state.displays.departed.is_empty());
}

/// A full-screen window on the display that goes leaves full screen on the remaining one, inside its usable area, and
/// that display's menu bar keeps its place.
#[test]
fn a_full_screen_window_of_a_display_that_goes_comes_back_as_a_normal_window() {
    let Some(mut h) = Harness::new() else { return };
    let mut clients = Vec::new();
    let (left, right) = (h.output("headless-1"), h.output("headless-2"));
    let w = h.map(&mut clients);
    let other = h.map(&mut clients);
    h.put(w, right, Rect::new(1100.0, 100.0, 400.0, 300.0));
    h.state.command(Command::FocusWindow { window: w });
    h.state.command(Command::Fullscreen {
        window: w,
        output: None,
        requested_by: Requester::User,
    });
    h.settle();
    assert_eq!(
        h.state.wm.window(w).unwrap().state(),
        LifecycleState::FullScreen
    );
    // full screen on one display leaves the other alone: its windows show and its menu bar keeps its space
    assert!(h.state.wm.visible_windows(left).contains(&other));
    assert!(h.state.menu_bars.set.reserves_space(left));
    assert_eq!(h.frame(w), h.state.wm.output(right).unwrap().rect);

    h.unplug("headless-2");
    h.settle();
    assert_eq!(
        h.state.wm.window(w).unwrap().state(),
        LifecycleState::Normal
    );
    assert_eq!(h.output_of(w), left);
    assert!(inside(h.frame(w), h.usable(left)), "{:?}", h.frame(w));
    assert!(h.state.wm.visible_windows(left).contains(&w));
    assert!(h.state.menu_bars.set.reserves_space(left));
}

/// The display a window is being dragged on goes: the pointer comes onto the remaining display and the window with it;
/// the drag goes on there and the window rests inside the usable area.
#[test]
fn a_window_dragged_on_a_display_that_goes_follows_the_pointer_onto_the_other() {
    let Some(mut h) = Harness::new() else { return };
    let mut clients = Vec::new();
    let (left, right) = (h.output("headless-1"), h.output("headless-2"));
    let w = h.map(&mut clients);
    h.put(w, right, Rect::new(1100.0, 100.0, 400.0, 300.0));
    h.move_pointer(1200.0, 110.0);
    h.state.begin_model_interaction(Command::BeginMove {
        window: w,
        pointer: MPoint::new(1200.0, 110.0),
    });
    h.move_pointer(1400.0, 250.0);
    assert!(h.state.wm.is_interacting());

    h.unplug("headless-2");
    assert!(h.state.wm.is_interacting(), "the drag goes on");
    assert_eq!(on_output(&h, h.pointer()), Some(left));
    assert_eq!(h.output_of(w), left);
    h.move_pointer(700.0, 300.0);
    h.release();
    assert!(!h.state.wm.is_interacting());
    assert!(inside(h.frame(w), h.usable(left)), "{:?}", h.frame(w));
}

/// While the session is locked, the display without the lock surface goes: the session stays locked and the lock
/// surface keeps the keyboard. Then the display with the lock surface goes: the session stays locked and covered on
/// the remaining display, and no window takes the keyboard or the pointer.
#[test]
fn a_locked_session_stays_locked_and_covered_when_its_displays_go() {
    let Some(mut h) = Harness::new() else { return };
    let mut clients = Vec::new();
    let right = h.output("headless-2");
    let w = h.map(&mut clients);
    h.put(w, right, Rect::new(1100.0, 100.0, 400.0, 300.0));
    let locker = Client::locker(common::session_agent(&h.state));
    h.wait_until("the lock request", |s| s.session_lock.is_active());
    // its one lock surface covers one display: the other stays black, and the lock holds once the wait is over
    let time = h.state.now() + 2.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.wait_until("the lock", |_| locker.locked());
    h.move_pointer(1300.0, 200.0);
    let keyboard = |h: &Harness| h.state.seat.get_keyboard().unwrap().current_focus();
    h.wait_until("the lock surface's keyboard", |s| {
        s.seat
            .get_keyboard()
            .unwrap()
            .current_focus()
            .is_some_and(|f| s.is_lock_surface(&f))
    });

    h.unplug("headless-2");
    assert!(h.state.session_lock.is_active());
    assert!(locker.locked());
    assert!(keyboard(&h).is_some_and(|f| h.state.is_lock_surface(&f)));
    assert!(on_output(&h, h.pointer()).is_some());

    // now the other way round: a display comes, and the one with the lock surface goes
    headless::add_display(&mut h.state, &right_display(), Some(RIGHT_SCALE)).unwrap();
    h.step();
    h.unplug("headless-1");
    assert!(h.state.session_lock.is_active(), "still locked");
    let window_surface = h.state.entry(w).unwrap().surface().clone();
    assert_ne!(keyboard(&h), Some(window_surface.clone()));
    let p = h.pointer();
    assert!(on_output(&h, p).is_some());
    assert!(
        h.state
            .surface_under(p.into())
            .is_none_or(|(s, _)| s != window_surface),
        "the window is not reachable while locked"
    );
    locker.unlock();
    h.wait_until("the unlock", |s| !s.session_lock.is_active());
}

/// An overview open on the display that goes ends; one open on the remaining display takes the windows that arrive.
#[test]
fn an_overview_on_a_display_that_goes_ends_and_one_elsewhere_takes_its_windows() {
    let Some(mut h) = Harness::new() else { return };
    let mut clients = Vec::new();
    let (left, right) = (h.output("headless-1"), h.output("headless-2"));
    let w = h.map(&mut clients);
    let x = h.map(&mut clients);
    h.put(w, right, Rect::new(1100.0, 100.0, 400.0, 300.0));
    h.state.open_overview(right, OverviewMode::Overview);
    assert!(h.state.shell.overview_on(right).is_some());
    h.unplug("headless-2");
    assert!(h.state.shell.overview.is_none(), "it ended at once");
    assert_eq!(h.output_of(w), left);

    headless::add_display(&mut h.state, &right_display(), Some(RIGHT_SCALE)).unwrap();
    h.settle();
    let right = h.output("headless-2");
    assert_eq!(h.output_of(w), right);
    h.state.open_overview(left, OverviewMode::Overview);
    let shown = |h: &Harness, window| {
        h.state
            .shell
            .overview_on(left)
            .is_some_and(|o| o.contains(window))
    };
    assert!(shown(&h, x) && !shown(&h, w));
    h.unplug("headless-2");
    assert!(shown(&h, w), "the arriving window joins the overview");
}

/// What opens without an output of its own opens on the active output: the one the pointer moved onto, or the one of
/// the window that took the keyboard last. Each output keeps its own usable area and scale.
#[test]
fn new_windows_and_the_overview_open_on_the_output_the_person_works_on() {
    let Some(mut h) = Harness::new() else { return };
    let mut clients = Vec::new();
    let (left, right) = (h.output("headless-1"), h.output("headless-2"));
    h.move_pointer(1300.0, 200.0);
    assert_eq!(h.state.active_output(), Some(right));
    let a = h.map(&mut clients);
    assert_eq!(h.output_of(a), right, "the pointer's output");
    assert!(inside(h.frame(a), h.usable(right)));
    assert_eq!(h.told_scale(a), Some(RIGHT_SCALE));

    h.move_pointer(300.0, 300.0);
    let b = h.map(&mut clients);
    assert_eq!(h.output_of(b), left);
    assert_eq!(h.told_scale(b), Some(1.0));
    assert!(inside(h.frame(b), h.usable(left)));

    // the keyboard goes to the window on the right while the pointer stays on the left
    h.state.command(Command::FocusWindow { window: a });
    h.step();
    assert_eq!(h.state.active_output(), Some(right));
    let c = h.map(&mut clients);
    assert_eq!(h.output_of(c), right, "the focused window's output");
    h.state.toggle_overview(false);
    assert!(h.state.shell.overview_on(right).is_some());
    h.state.toggle_overview(false);
    // the work areas stay apart
    let (ul, ur) = (h.usable(left), h.usable(right));
    assert!(ul.right() <= ur.x + 1e-6, "{ul:?} {ur:?}");
    assert_eq!(ur.w, f64::from(RIGHT.0) / RIGHT_SCALE);
}

/// While the session is locked no frame of any display shows a window, from the first one: the display that remains
/// when one goes, a display that comes (with a window going back to it at once) and the display left when the one with
/// the lock surface goes. Unlocked, the same window shows.
#[test]
fn while_locked_no_frame_of_a_remaining_or_arriving_display_shows_a_window() {
    let Some(mut h) = Harness::new() else { return };
    let mut clients = Vec::new();
    let right = h.output("headless-2");
    let w = h.map(&mut clients);
    h.put(w, right, Rect::new(1100.0, 100.0, 400.0, 300.0));
    assert!(
        h.pixels_of("headless-2", TOPLEVEL_BLUE) > 10_000,
        "the window shows"
    );
    let locker = Client::locker(common::session_agent(&h.state));
    h.wait_until("the lock request", |s| s.session_lock.is_active());
    let time = h.state.now() + 2.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.wait_until("the lock", |_| locker.locked());

    h.unplug("headless-2");
    assert_eq!(h.output_of(w), h.output("headless-1"));
    assert_eq!(h.pixels_of("headless-1", TOPLEVEL_BLUE), 0);
    // the display comes back and the window goes back to it before its first frame
    headless::add_display(&mut h.state, &right_display(), Some(RIGHT_SCALE)).unwrap();
    assert_eq!(h.output_of(w), h.output("headless-2"));
    assert_eq!(
        h.pixels_of("headless-2", TOPLEVEL_BLUE),
        0,
        "its first frame"
    );
    h.step();
    assert_eq!(h.pixels_of("headless-2", TOPLEVEL_BLUE), 0);
    // the display with the lock surface goes: the one left stays covered
    h.unplug("headless-1");
    assert_eq!(h.pixels_of("headless-2", TOPLEVEL_BLUE), 0);
    h.settle();
    assert_eq!(h.pixels_of("headless-2", TOPLEVEL_BLUE), 0);

    locker.unlock();
    h.wait_until("the unlock", |s| !s.session_lock.is_active());
    // the person is back: the displays do not dim for the idle time the test's clock jumps covered
    h.state.input_activity();
    h.settle();
    assert!(
        h.pixels_of("headless-2", TOPLEVEL_BLUE) > 10_000,
        "the window shows again"
    );
}

/// The last display goes and another comes: the windows come onto it, inside its usable area. When their own display
/// comes back as well, they go back to it.
#[test]
fn windows_left_when_the_last_display_goes_come_onto_the_next_one_and_back_to_their_own() {
    let Some(mut h) = Harness::new() else { return };
    let mut clients = Vec::new();
    let right = h.output("headless-2");
    let w = h.map(&mut clients);
    let at = Rect::new(1100.0, 100.0, 400.0, 300.0);
    h.put(w, right, at);
    let from = h.state.output_entry(right).unwrap().rect;
    h.unplug("headless-1");
    h.unplug("headless-2");
    assert!(h.state.outputs.is_empty());

    let other = HeadlessDisplay {
        name: "headless-3".into(),
        size: (1280, 800),
        physical_mm: (0, 0),
    };
    headless::add_display(&mut h.state, &other, Some(1.0)).unwrap();
    h.settle();
    let third = h.output("headless-3");
    assert_eq!(h.output_of(w), third, "the next display takes it");
    assert!(inside(h.frame(w), h.usable(third)), "{:?}", h.frame(w));
    assert!(h.state.wm.visible_windows(third).contains(&w));
    assert_eq!(h.told_scale(w), Some(1.0));

    headless::add_display(&mut h.state, &right_display(), Some(RIGHT_SCALE)).unwrap();
    h.settle();
    let right = h.output("headless-2");
    let back = h.state.output_entry(right).unwrap().rect;
    assert_eq!(h.output_of(w), right, "it goes back to its own display");
    assert_eq!(
        h.frame(w),
        Rect::new(
            back.x + (at.x - from.x),
            back.y + (at.y - from.y),
            at.w,
            at.h
        )
    );
    assert_eq!(h.told_scale(w), Some(RIGHT_SCALE));
    assert!(h.state.displays.departed.is_empty());
}

/// The last display goes and comes back: its windows come back to the same place on it.
#[test]
fn windows_of_the_last_display_come_back_to_it_when_it_returns() {
    let Some(mut h) = Harness::new() else { return };
    let mut clients = Vec::new();
    let right = h.output("headless-2");
    let w = h.map(&mut clients);
    let at = Rect::new(1100.0, 100.0, 400.0, 300.0);
    h.put(w, right, at);
    let from = h.state.output_entry(right).unwrap().rect;
    h.unplug("headless-1");
    h.unplug("headless-2");
    // it comes back alone, so at another place in the global space
    headless::add_display(&mut h.state, &right_display(), Some(RIGHT_SCALE)).unwrap();
    h.settle();
    let right = h.output("headless-2");
    let rect = h.state.output_entry(right).unwrap().rect;
    assert_eq!(h.output_of(w), right);
    assert_eq!(
        h.frame(w),
        Rect::new(
            rect.x + (at.x - from.x),
            rect.y + (at.y - from.y),
            at.w,
            at.h
        ),
        "the display is at {rect:?}"
    );
    assert!(h.state.displays.departed.is_empty());
}

/// A window's subsurfaces are told the scale of the output the window is on, and change with it.
#[test]
fn a_subsurface_draws_at_the_scale_of_its_windows_output() {
    use smithay::wayland::compositor::get_children;
    let Some(mut h) = Harness::new() else { return };
    let before: Vec<WindowId> = h.state.wm.windows().map(|w| w.id).collect();
    let _client = Client::constraining_under_subsurface(h.socket_path(), "covered");
    let mut new = None;
    h.wait_until("the toplevel to be mapped", |s| {
        new =
            s.wm.windows()
                .map(|w| w.id)
                .find(|w| !before.contains(w) && s.entry(*w).is_some_and(|e| e.mapped));
        new.is_some()
    });
    h.settle();
    let w = new.unwrap();
    let surface = h.state.entry(w).unwrap().surface().clone();
    let child = get_children(&surface)
        .into_iter()
        .next()
        .expect("a subsurface");
    let told = |surface| {
        with_states(surface, |states| {
            with_fractional_scale(states, |fs| fs.preferred_scale())
        })
    };
    let right = h.output("headless-2");
    h.put(w, right, Rect::new(1100.0, 100.0, 400.0, 300.0));
    assert_eq!(told(&surface), Some(RIGHT_SCALE));
    assert_eq!(told(&child), Some(RIGHT_SCALE));
    h.unplug("headless-2");
    assert_eq!(told(&child), Some(1.0));
}
