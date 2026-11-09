//! Session lock against a headless compositor: a toplevel client and a lock client connect through the socket.
//! While the lock holds, only the lock surface may take the pointer, the keyboard and the pixels. Tests skip
//! (pass) when no EGL device is available.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
mod common;

use common::Client;

const OUTPUT: (i32, i32) = (1000, 700);

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
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
        match headless::init(&mut state, OUTPUT, 1.0) {
            Ok(()) => Some(Harness { state, event_loop }),
            Err(e) => {
                common::egl_unavailable(format!("{e:#}"));
                None
            }
        }
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(5),
            )
            .unwrap();
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    fn screenshot(&mut self, name: &str) -> image::RgbaImage {
        let path =
            std::env::temp_dir().join(format!("cupertino-test-{}-{name}.png", std::process::id()));
        self.state.screenshot_requests.push(path.clone());
        self.state.request_redraw();
        let p = path.clone();
        self.wait_until("the screenshot", move |_| p.exists());
        let img = image::open(&path).unwrap().to_rgba8();
        let _ = std::fs::remove_file(path);
        img
    }

    /// A point inside the only toplevel.
    fn toplevel_center(&self) -> (f64, f64) {
        let id = self.state.wm.key_window().expect("a focused toplevel");
        let g = self.state.wm.window(id).unwrap().geometry();
        (g.x + g.w / 2.0, g.y + g.h / 2.0)
    }

    fn surface_under(
        &self,
        p: (f64, f64),
    ) -> Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface> {
        self.state.surface_under(p.into()).map(|(s, _)| s)
    }

    fn keyboard_focus(
        &self,
    ) -> Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface> {
        self.state.seat.get_keyboard().unwrap().current_focus()
    }

    fn center_pixel(&mut self, name: &str) -> [u8; 4] {
        let p = self.toplevel_center();
        self.screenshot(name).get_pixel(p.0 as u32, p.1 as u32).0
    }
}

fn client_count(state: &Cupertino) -> usize {
    let mut count = 0;
    state
        .display_handle
        .backend_handle()
        .with_all_clients(|_| count += 1);
    count
}

fn mapped_toplevel(h: &mut Harness) -> (Client, (f64, f64)) {
    let client = Client::toplevel(h.socket_path());
    h.wait_until("the toplevel to be mapped", |s| {
        s.wm.windows().count() == 1
            && s.wm
                .key_window()
                .is_some_and(|w| s.entry(w).is_some_and(|e| e.mapped))
    });
    // the opening animation settles
    let time = h.state.now() + 5.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    let center = h.toplevel_center();
    let under = h
        .surface_under(center)
        .expect("the toplevel is under its center");
    assert!(
        h.state.entry_for_surface(&under).is_some(),
        "a toplevel surface is hit before the lock"
    );
    (client, center)
}

#[test]
fn a_lock_covers_the_toplevel_and_gives_the_pointer_and_keyboard_to_the_lock_surface() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, center) = mapped_toplevel(&mut h);
    assert_eq!(
        h.center_pixel("before")[..3],
        [0, 0, 255][..],
        "the toplevel shows before the lock"
    );

    let locker = Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());

    let under = h
        .surface_under(center)
        .expect("the lock surface is under the pointer");
    assert!(
        h.state.is_lock_surface(&under),
        "the surface under the former toplevel is the lock surface"
    );
    assert!(
        h.keyboard_focus()
            .is_some_and(|s| h.state.is_lock_surface(&s)),
        "the keyboard is on the lock surface"
    );
    assert_eq!(
        h.center_pixel("locked")[..3],
        [0, 255, 0][..],
        "only the lock surface is drawn"
    );
}

#[test]
fn a_lock_client_that_disconnects_leaves_the_session_locked_and_black() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, center) = mapped_toplevel(&mut h);
    let locker = Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());
    drop(locker);

    h.wait_until("the lock surface to go away", |s| {
        s.surface_under(center.into()).is_none()
    });
    assert!(
        h.state.session_lock.is_active(),
        "the session is still locked"
    );
    assert!(
        h.keyboard_focus().is_none(),
        "no keyboard focus while the lock has no client"
    );
    assert_eq!(
        h.center_pixel("orphaned")[..3],
        [0, 0, 0][..],
        "the output is black"
    );

    let successor = Client::locker(common::session_agent(&h.state));
    h.wait_until("a new client to take the lock", |_| successor.locked());
    let under = h
        .surface_under(center)
        .expect("the new lock surface is under the pointer");
    assert!(h.state.is_lock_surface(&under));
}

#[test]
fn a_second_lock_request_is_refused_while_the_session_is_locked() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, _) = mapped_toplevel(&mut h);
    let holder = Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| holder.locked());
    let intruder = Client::locker(common::session_agent(&h.state));
    h.wait_until("the second request to be refused", |_| intruder.finished());
    assert!(!intruder.locked());
}

#[test]
fn an_unlock_from_a_refused_lock_does_not_open_a_session_whose_locker_crashed() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, center) = mapped_toplevel(&mut h);
    let holder = Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| holder.locked());
    let intruder = Client::locker(common::session_agent(&h.state));
    h.wait_until("the second request to be refused", |_| intruder.finished());

    // both reach the compositor in the same dispatch: the intruder's unlock request and the holder's crash
    intruder.unlock();
    // the server is not dispatched meanwhile, so the unlock waits on the connection for the dispatch that sees the crash
    let deadline = Instant::now() + common::HANG_GUARD;
    while !intruder.unlock_sent() {
        assert!(
            Instant::now() < deadline,
            "the intruder did not send its unlock"
        );
        std::thread::yield_now();
    }
    holder.kill();
    h.wait_until("the holder's lock surface to go away", |s| {
        s.surface_under(center.into()).is_none()
    });
    assert!(
        h.state.session_lock.is_active(),
        "the session is still locked"
    );
    assert_eq!(
        h.center_pixel("after the intruder's unlock")[..3],
        [0, 0, 0][..],
        "the output is black"
    );
}

#[test]
fn unlocking_gives_the_toplevel_back_the_pointer_the_keyboard_and_the_screen() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, center) = mapped_toplevel(&mut h);
    let toplevel_surface = h.surface_under(center).unwrap();
    let locker = Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());

    locker.unlock();
    h.wait_until("the session to be unlocked", |s| {
        !s.session_lock.is_active()
    });
    assert_eq!(
        h.surface_under(center),
        Some(toplevel_surface.clone()),
        "the toplevel is under the pointer again"
    );
    assert_eq!(
        h.keyboard_focus(),
        Some(toplevel_surface),
        "the toplevel has the keyboard again"
    );
    assert_eq!(
        h.center_pixel("unlocked")[..3],
        [0, 0, 255][..],
        "the toplevel is drawn again"
    );
}

/// While the session is locked an application's layer surface cannot take the keyboard exclusively, neither to read
/// what is typed at the lock screen nor to be waiting for it at the unlock.
#[test]
fn an_application_cannot_take_the_keyboard_exclusively_while_locked() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, _) = mapped_toplevel(&mut h);
    let surface = {
        let window = h.state.wm.key_window().unwrap();
        h.state.entry(window).unwrap().surface().clone()
    };

    h.state.layer_keyboard_exclusivity(&surface, true);
    assert_eq!(
        h.state.exclusive_layer.as_ref(),
        Some(&surface),
        "outside a lock a layer surface takes the keyboard"
    );
    h.state.layer_keyboard_exclusivity(&surface, false);

    let locker = Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());
    h.state.layer_keyboard_exclusivity(&surface, true);
    assert_eq!(
        h.state.exclusive_layer, None,
        "an application took the keyboard at the lock screen"
    );
}

/// An application is told the session is locking before the lock client is told it holds the lock, and unlocked only
/// once the lock is over; one that starts during the lock is told at once, and one bound below version 5 never is.
#[test]
fn an_application_learns_the_lock_as_it_starts_and_the_unlock_once_it_is_over() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, _) = mapped_toplevel(&mut h);
    let current = Client::appearance_observer(h.socket_path(), 5);
    let older = Client::appearance_observer(h.socket_path(), 4);
    h.wait_until("the observer to be told the session is open", |_| {
        current.lock_events() == ["session_lock 0"]
    });

    let locker = Client::observing_locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());
    assert_eq!(
        locker.lock_events(),
        ["session_lock 0", "session_lock 1", "locked"]
    );
    h.wait_until("the observer to be told the session is locked", |_| {
        current.lock_events().len() == 2
    });
    let late = Client::appearance_observer(h.socket_path(), 5);
    h.wait_until("the late observer to be told", |_| {
        !late.lock_events().is_empty()
    });
    assert_eq!(late.lock_events(), ["session_lock 1"]);

    locker.unlock();
    h.wait_until("the observers to be told the session is open again", |_| {
        current.lock_events().len() == 3 && late.lock_events().len() == 2
    });
    assert!(!h.state.session_lock.is_active());
    assert_eq!(
        current.lock_events(),
        ["session_lock 0", "session_lock 1", "session_lock 0"]
    );
    assert_eq!(late.lock_events(), ["session_lock 1", "session_lock 0"]);
    assert!(
        older.lock_events().is_empty(),
        "a version 4 observer was told"
    );
}

#[test]
fn an_application_stays_told_locked_when_the_lock_client_crashes() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, center) = mapped_toplevel(&mut h);
    let observer = Client::appearance_observer(h.socket_path(), 5);
    h.wait_until("the observer to be told the session is open", |_| {
        !observer.lock_events().is_empty()
    });
    let locker = Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());
    locker.kill();
    h.wait_until("the lock surface to go away", |s| {
        s.surface_under(center.into()).is_none()
    });
    let successor = Client::locker(common::session_agent(&h.state));
    h.wait_until("a new client to take the lock", |_| successor.locked());
    assert_eq!(observer.lock_events(), ["session_lock 0", "session_lock 1"]);
}

impl Harness {
    /// Wait for each of `clients`' [`Client::barrier`]: they have every event sent before, and the server handled
    /// their answers.
    fn sync(&mut self, clients: &[&Client]) {
        let barriers: Vec<_> = clients.iter().map(|c| c.barrier()).collect();
        self.wait_until("the clients' roundtrips", move |_| {
            barriers.iter().all(|b| b())
        });
    }

    /// Press `keys` in order on the keyboard device and release them in reverse.
    fn chord(&mut self, keys: &[u32]) {
        use smithay::backend::input::KeyState;
        use smithay::input::keyboard::Keycode;
        for (state, keys) in [
            (KeyState::Pressed, keys.to_vec()),
            (KeyState::Released, keys.iter().rev().copied().collect()),
        ] {
            for key in keys {
                self.state.key_from_device(Keycode::new(key + 8), state, 0);
            }
        }
    }
}

/// The window ids of the mapped toplevels, in the order they were made.
fn window_ids(h: &Harness) -> Vec<cupertino_wm::WindowId> {
    h.state
        .windows
        .iter()
        .filter(|e| e.mapped)
        .map(|e| e.id)
        .collect()
}

#[test]
fn every_toplevel_is_deactivated_while_locked_and_the_active_one_is_active_again_after_the_unlock()
{
    let Some(mut h) = Harness::new() else { return };
    let (first, _) = mapped_toplevel(&mut h);
    let second = Client::toplevel(h.socket_path());
    h.wait_until("the second toplevel to be mapped", |s| {
        s.windows.iter().filter(|e| e.mapped).count() == 2
    });
    let ids = window_ids(&h);
    // the first window is the active one when the lock begins
    h.state
        .command(cupertino_wm::Command::FocusWindow { window: ids[0] });
    h.wait_until("the first toplevel to be active", |s| {
        s.wm.key_window() == Some(ids[0])
            && first.activations().last() == Some(&true)
            && second.activations().last() == Some(&false)
    });

    let locker = Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());
    h.wait_until("every toplevel to be deactivated", |_| {
        first.activations().last() == Some(&false) && second.activations().last() == Some(&false)
    });
    // a window that opens behind the lock is never activated, whatever focus it takes in the model
    let third = Client::toplevel(h.socket_path());
    h.wait_until("a toplevel to be mapped behind the lock", |s| {
        s.windows.iter().filter(|e| e.mapped).count() == 3
    });
    h.sync(&[&first, &second, &third]);
    assert!(
        !third.activations().contains(&true),
        "a window was activated behind the lock: {:?}",
        third.activations()
    );
    assert!(!first.activations().last().unwrap() && !second.activations().last().unwrap());

    locker.unlock();
    h.wait_until("the session to be unlocked", |s| {
        !s.session_lock.is_active()
    });
    h.wait_until("the first toplevel to be active again", |_| {
        first.activations().last() == Some(&true)
    });
    assert_eq!(h.state.wm.key_window(), Some(ids[0]));
    h.sync(&[&second, &third]);
    assert_eq!(second.activations().last(), Some(&false));
    assert!(!third.activations().contains(&true));
}

/// A screen reader that only runs: its process stands for the session's reader.
fn fake_reader(dir: &std::path::Path) -> PathBuf {
    let script = dir.join("fake-reader");
    std::fs::write(&script, "#!/bin/sh\nexec /usr/bin/sleep 300\n").unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    script
}

#[test]
fn at_the_lock_screen_super_alt_s_turns_the_reader_on_and_off_and_no_other_shortcut_acts() {
    const KEY_SUPER: u32 = 125;
    const KEY_LEFTALT: u32 = 56;
    const KEY_LEFTCTRL: u32 = 29;
    const KEY_LEFTSHIFT: u32 = 42;
    const KEY_S: u32 = 31;
    const KEY_TAB: u32 = 15;
    const KEY_SPACE: u32 = 57;
    const KEY_F: u32 = 33;
    const KEY_M: u32 = 50;
    const KEY_H: u32 = 35;
    const KEY_Q: u32 = 16;
    const KEY_W: u32 = 17;
    const KEY_UP: u32 = 103;
    const KEY_F3: u32 = 61;
    const KEY_4: u32 = 5;
    let Some(mut h) = Harness::new() else { return };
    let dir = tempfile::tempdir().unwrap();
    h.state.session_reader.program = Some(fake_reader(dir.path()));
    h.state
        .use_accessibility_store(dir.path().join("accessibility.conf"))
        .unwrap();
    let (_toplevel, _) = mapped_toplevel(&mut h);
    let window = h.state.wm.key_window().unwrap();
    let locker = Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());
    h.wait_until("the lock surface to take the keyboard", |s| {
        s.seat
            .get_keyboard()
            .unwrap()
            .current_focus()
            .is_some_and(|f| s.is_lock_surface(&f))
    });

    h.chord(&[KEY_SUPER, KEY_LEFTALT, KEY_S]);
    let pid = h
        .state
        .session_reader
        .pid()
        .expect("Super+Alt+S starts the reader at the lock screen");
    assert!(h.state.accessibility.settings.screen_reader);

    // the desktop's and the applications' shortcuts: each key goes to the lock screen, none acts
    let shortcuts: [&[u32]; 10] = [
        &[KEY_SUPER, KEY_TAB],
        &[KEY_SUPER, KEY_SPACE],
        &[KEY_SUPER, KEY_LEFTCTRL, KEY_F],
        &[KEY_SUPER, KEY_LEFTCTRL, KEY_UP],
        &[KEY_SUPER, KEY_M],
        &[KEY_SUPER, KEY_H],
        &[KEY_SUPER, KEY_Q],
        &[KEY_SUPER, KEY_W],
        &[KEY_F3],
        &[KEY_SUPER, KEY_LEFTSHIFT, KEY_4],
    ];
    for chord in shortcuts {
        h.chord(chord);
    }
    h.sync(&[&locker]);
    let pressed: Vec<u32> = locker
        .keys()
        .into_iter()
        .filter(|(_, down)| *down)
        .map(|(key, _)| key)
        .collect();
    for chord in shortcuts {
        let key = *chord.last().unwrap();
        assert!(
            pressed.contains(&key),
            "{chord:?} did not reach the lock screen: {pressed:?}"
        );
    }
    assert!(
        !pressed.contains(&KEY_S),
        "the screen reader's chord reached the lock screen"
    );
    let w = h.state.wm.window(window).unwrap();
    assert_eq!(w.state(), cupertino_wm::LifecycleState::Normal);
    assert!(!h.state.wm.app(w.app).unwrap().hidden);
    assert!(!h.state.shell.overview_open());
    assert!(h.state.session_lock.is_active());

    h.chord(&[KEY_SUPER, KEY_LEFTALT, KEY_S]);
    assert_eq!(
        h.state.session_reader.pid(),
        None,
        "Super+Alt+S stops the reader at the lock screen"
    );
    assert!(!h.state.accessibility.settings.screen_reader);
    let _ = pid;
}

/// Give the session a lock screen: a login process with the session lock role (never started here; the tests' lock
/// clients stand for it).
fn with_lock_screen(h: &mut Harness) {
    h.state
        .config
        .shell_processes
        .push(cupertino_server::config::ShellProcess {
            name: "login".into(),
            command: vec!["/usr/bin/false".into()],
            roles: vec![cupertino_server::authorization::ShellRole::SessionLock],
        });
}

#[test]
fn without_a_lock_screen_a_sleep_leaves_the_session_as_it_is() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, _) = mapped_toplevel(&mut h);
    // a nested run, or the login screen: nothing would ever take a covered session over
    h.state.system_going_to_sleep();
    assert!(
        !h.state.session_lock.holds_sleep(),
        "the sleep goes on at once"
    );
    assert!(!h.state.session_lock.is_active());
    assert_eq!(h.center_pixel("awake")[..3], [0, 0, 255][..]);
    with_lock_screen(&mut h);
    h.state.login_screen = true;
    h.state.system_going_to_sleep();
    assert!(!h.state.session_lock.holds_sleep());
    assert!(!h.state.session_lock.is_active());
}

#[test]
fn a_session_the_system_sleeps_on_before_it_is_locked_is_covered_until_a_lock_client_takes_it_over()
{
    let Some(mut h) = Harness::new() else { return };
    with_lock_screen(&mut h);
    let (toplevel, center) = mapped_toplevel(&mut h);
    assert_eq!(h.center_pixel("awake")[..3], [0, 0, 255][..]);

    // the lock client hung or crashed before it asked for the lock
    h.state.system_going_to_sleep();
    assert!(h.state.session_lock.is_active());
    assert!(
        h.state.session_lock.holds_sleep(),
        "the sleep waits for the cover"
    );
    h.wait_until("every output to show the cover", |s| {
        !s.session_lock.holds_sleep()
    });
    assert!(h.state.sleep_covered());
    assert_eq!(
        h.center_pixel("covered")[..3],
        [0, 0, 0][..],
        "the session is covered"
    );
    assert!(
        h.keyboard_focus().is_none(),
        "no keyboard focus under the cover"
    );
    assert!(
        h.surface_under(center).is_none(),
        "the pointer reaches nothing under the cover"
    );
    h.wait_until("the toplevel to be deactivated", |_| {
        toplevel.activations().last() == Some(&false)
    });

    // the next lock client (a login process that came back, or one that was slow) takes the covered session over
    let locker = Client::locker(common::session_agent(&h.state));
    h.wait_until("a lock client to take the lock", |_| locker.locked());
    assert_eq!(h.center_pixel("locked")[..3], [0, 255, 0][..]);
    locker.unlock();
    h.wait_until("the session to be unlocked", |s| {
        !s.session_lock.is_active()
    });
    assert_eq!(h.center_pixel("unlocked")[..3], [0, 0, 255][..]);
    h.wait_until("the toplevel to be active again", |_| {
        toplevel.activations().last() == Some(&true)
    });
}

#[test]
fn a_lock_client_that_hangs_before_presenting_and_then_dies_leaves_a_sleeping_session_covered() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, center) = mapped_toplevel(&mut h);
    // the model clock stands still: the lock stays pending however long the client takes
    h.state.fixed_time = Some(h.state.now());

    // without a sleep, a lock client that goes before it confirms leaves the session as it was
    let first = Client::hung_locker(common::session_agent(&h.state));
    h.wait_until("the lock to be pending", |s| s.session_lock.is_active());
    first.kill();
    h.wait_until("the session to come back", |s| !s.session_lock.is_active());

    let hung = Client::hung_locker(common::session_agent(&h.state));
    h.wait_until("the lock to be pending", |s| s.session_lock.is_active());
    h.state.system_going_to_sleep();
    h.wait_until("every output to show the cover", |s| {
        !s.session_lock.holds_sleep()
    });
    assert!(!hung.locked(), "the lock is still pending");
    assert_eq!(h.center_pixel("pending")[..3], [0, 0, 0][..]);

    let connected = client_count(&h.state);
    hung.kill();
    h.wait_until("the server to see the lock client gone", |s| {
        client_count(s) < connected
    });
    assert!(
        h.state.session_lock.is_active(),
        "the session opened when the lock client died after the system slept"
    );
    assert_eq!(
        h.center_pixel("orphaned")[..3],
        [0, 0, 0][..],
        "the session shows after the wake"
    );
    assert!(h.surface_under(center).is_none());

    let successor = Client::locker(common::session_agent(&h.state));
    h.wait_until("a new lock client to take the lock", |_| successor.locked());
}

#[test]
fn an_output_that_is_powered_off_holds_no_sleep_back_and_shows_the_cover_once_on() {
    let Some(mut h) = Harness::new() else { return };
    with_lock_screen(&mut h);
    let (_toplevel, _) = mapped_toplevel(&mut h);
    let output = h.state.outputs[0].id;
    // an output that is off is never drawn: no covered frame of it can reach the screen
    h.state.set_output_power(output, false).unwrap();
    h.state.system_going_to_sleep();
    assert!(h.state.session_lock.holds_sleep());
    h.wait_until("the sleep to go on", |s| !s.session_lock.holds_sleep());
    assert!(h.state.session_lock.is_active());

    h.state.set_output_power(output, true).unwrap();
    assert_eq!(
        h.center_pixel("powered on")[..3],
        [0, 0, 0][..],
        "the session shows on an output powered on after the sleep"
    );
}

#[test]
fn a_configure_held_back_by_a_shrinking_frame_activates_no_window_behind_the_lock() {
    let Some(mut h) = Harness::new() else { return };
    let (toplevel, _) = mapped_toplevel(&mut h);
    let id = h.state.wm.key_window().unwrap();
    h.wait_until("the toplevel to be active", |_| {
        toplevel.activations().last() == Some(&true)
    });
    let locker = Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());
    h.wait_until("the toplevel to be deactivated", |_| {
        toplevel.activations().last() == Some(&false)
    });
    let before = toplevel.activations().len();

    // the configure a shrinking frame held back (an unzoom asked for just before or behind the lock) goes out once the
    // animation ends
    let frame = h.state.wm.window(id).unwrap().geometry();
    h.state
        .windows
        .iter_mut()
        .find(|e| e.id == id)
        .unwrap()
        .shrink = cupertino_server::state::Shrink::Deferred {
        frame: cupertino_wm::Rect::new(frame.x, frame.y, frame.w - 10.0, frame.h - 10.0),
        states: cupertino_wm::ToplevelStates::default(),
    };
    h.wait_until("the held configure to go out", |_| {
        toplevel.activations().len() > before
    });
    assert_eq!(
        toplevel.activations()[before..],
        [false],
        "the held configure activated the window behind the lock"
    );
}

/// An application is told whether to keep its scroll bars shown: by the setting, and for an automatic one while a
/// mouse is connected. An observer below version 8 does not know the bit and is never sent it.
#[test]
fn an_application_learns_when_to_show_its_scroll_bars() {
    use cupertino_defaults::compositor::ScrollBarsName;
    const ALWAYS: u32 = 16;
    let Some(mut h) = Harness::new() else { return };
    let observer = Client::appearance_observer(h.socket_path(), 8);
    let old = Client::appearance_observer(h.socket_path(), 7);
    h.wait_until("the first options", |_| {
        !observer.display_options().is_empty() && !old.display_options().is_empty()
    });
    assert_eq!(
        observer.display_options(),
        [0],
        "automatic, with no mouse: over the content while it scrolls"
    );

    // a mouse that has been used
    let mut mouse = cupertino_server::events::PointerDevice::new(false, &h.state.config.pointer);
    assert!(mouse.note_use());
    h.state.pointer_devices.insert("mouse".into(), mouse);
    h.state.sync_scroll_bars();
    h.wait_until("the mouse to count", |_| {
        observer.display_options().len() == 2
    });
    assert_eq!(observer.display_options()[1], ALWAYS);
    // a touchpad does not count
    h.state.pointer_devices.remove("mouse");
    let mut touchpad = cupertino_server::events::PointerDevice::new(true, &h.state.config.pointer);
    touchpad.note_use();
    h.state.pointer_devices.insert("touchpad".into(), touchpad);
    h.state.sync_scroll_bars();
    h.wait_until("the mouse to go", |_| observer.display_options().len() == 3);
    assert_eq!(observer.display_options()[2], 0);

    h.state.config.show_scroll_bars = ScrollBarsName::Always;
    h.state.sync_scroll_bars();
    h.wait_until("always", |_| observer.display_options().len() == 4);
    assert_eq!(observer.display_options()[3], ALWAYS);
    // nothing changed: nothing to tell
    h.state.sync_scroll_bars();
    let late = Client::appearance_observer(h.socket_path(), 8);
    h.wait_until("the late observer", |_| !late.display_options().is_empty());
    assert_eq!(late.display_options(), [ALWAYS]);
    assert_eq!(observer.display_options().len(), 4);

    h.state.config.show_scroll_bars = ScrollBarsName::WhenScrolling;
    h.state.sync_scroll_bars();
    h.wait_until("when scrolling", |_| observer.display_options().len() == 5);
    assert_eq!(observer.display_options()[4], 0);

    h.state.config.show_scroll_bars = ScrollBarsName::Always;
    h.state.sync_scroll_bars();
    let old_late = Client::appearance_observer(h.socket_path(), 7);
    h.wait_until("an older observer", |_| {
        observer.display_options().len() == 6 && !old_late.display_options().is_empty()
    });
    assert_eq!(old.display_options(), [0], "an older observer was told");
    assert_eq!(
        old_late.display_options(),
        [0],
        "an older observer was told"
    );
}

/// An application is told to keep its text caret from blinking while the accessibility setting is on, with the other
/// display options and when only it changes. An observer below version 9 does not know the bit and is never sent it.
#[test]
fn an_application_learns_to_keep_its_text_caret_from_blinking() {
    use cupertino_defaults::accessibility::{AccessibilitySettings, DisplayOptions};
    const REDUCE_MOTION: u32 = 1;
    const STEADY: u32 = 32;
    let Some(mut h) = Harness::new() else { return };
    let observer = Client::appearance_observer(h.socket_path(), 9);
    let old = Client::appearance_observer(h.socket_path(), 8);
    h.wait_until("the first options", |_| {
        !observer.display_options().is_empty() && !old.display_options().is_empty()
    });
    assert_eq!(observer.display_options(), [0]);

    let steady = AccessibilitySettings {
        steady_caret: true,
        ..AccessibilitySettings::default()
    };
    h.state.set_accessibility(steady);
    h.wait_until("the steady caret", |_| {
        observer.display_options().len() == 2
    });
    assert_eq!(observer.display_options()[1], STEADY);

    h.state.set_accessibility(AccessibilitySettings {
        display: DisplayOptions {
            reduce_motion: true,
            ..DisplayOptions::default()
        },
        ..steady
    });
    h.wait_until("reduce motion", |_| {
        observer.display_options().len() == 3 && old.display_options().len() == 2
    });
    assert_eq!(observer.display_options()[2], REDUCE_MOTION | STEADY);
    assert_eq!(old.display_options(), [0, REDUCE_MOTION]);

    let late = Client::appearance_observer(h.socket_path(), 9);
    h.wait_until("the late observer", |_| !late.display_options().is_empty());
    assert_eq!(late.display_options(), [REDUCE_MOTION | STEADY]);

    h.state.set_accessibility(AccessibilitySettings::default());
    h.wait_until("all off", |_| {
        observer.display_options().len() == 4 && old.display_options().len() == 3
    });
    assert_eq!(observer.display_options()[3], 0);
    assert_eq!(old.display_options(), [0, REDUCE_MOTION, 0]);
}
