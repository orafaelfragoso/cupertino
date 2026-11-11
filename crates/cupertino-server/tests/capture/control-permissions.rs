//! What the person decided for each app decides its requests: the app a run comes from is the program that started the
//! controller's peer (here, the test binary that started a child standing in for an agent's bridge), or the desktop
//! entry of the window that peer runs under. The remembered answers are a file of the test's own.
use super::*;
use cupertino_defaults::desktop_control::{
    self as remembered, Decision, DesktopControlConf, Service,
};
use cupertino_server::config::SettingsFiles;
use cupertino_server::desktop::{parse_entry, DesktopIndex};
use cupertino_server::live::SettingsPaths;
use smithay::reexports::wayland_server::Resource;
use std::hash::{Hash, Hasher};

/// The desktop entry of the window the agent runs under in [`an_undecided_app_is_asked_once_and_its_allow_is_remembered`].
const TERMINAL: &str = "org.example.terminal";
/// Evdev codes of the stop shortcut's keys.
const KEY_ESC: u32 = 1;
const KEY_LEFTCTRL: u32 = 29;
const KEY_LEFTALT: u32 = 56;

/// A process standing in for an agent's bridge: a child of the test, with no display connection of its own. It is
/// killed and reaped when dropped.
pub(super) struct Bridge {
    pub(super) pid: u32,
    pub(super) start: u64,
    child: std::process::Child,
}

impl Bridge {
    pub(super) fn spawn() -> Self {
        let child = std::process::Command::new("sleep")
            .arg("60")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let (_, start) = cupertino_server::process::parent_and_start(pid).unwrap();
        Self { pid, start, child }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The app every bridge of these tests belongs to unless a window says otherwise: the program that started it.
pub(super) fn this_program() -> remembered::App {
    remembered::App::Executable(std::env::current_exe().unwrap())
}

/// A remembered-answers file in a folder of the test's own, which the server reads instead of the person's.
pub(super) struct Store {
    dir: tempfile::TempDir,
}

impl Store {
    /// A file holding `answers` for `app`, read by the server once.
    pub(super) fn read_by(
        h: &mut Harness,
        app: &remembered::App,
        answers: &[(Service, Decision)],
    ) -> Self {
        let store = Self::holding(app, answers);
        h.state.reload_desktop_control(&store.path());
        store
    }

    /// A file holding `answers` for `app`, which the server watches and reads again whenever it changes.
    fn watched_by(h: &mut Harness, app: &remembered::App, answers: &[(Service, Decision)]) -> Self {
        let store = Self::holding(app, answers);
        h.state.start_settings_watch(
            SettingsFiles::default(),
            SettingsPaths {
                desktop_control: Some(store.path()),
                ..SettingsPaths::default()
            },
        );
        store
    }

    fn holding(app: &remembered::App, answers: &[(Service, Decision)]) -> Self {
        let store = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        if !answers.is_empty() {
            store.change(|conf| {
                for (service, decision) in answers {
                    conf.set(*service, app, *decision).unwrap();
                }
            });
        }
        store
    }

    pub(super) fn path(&self) -> PathBuf {
        self.dir.path().join(remembered::DESKTOP_CONTROL_FILE)
    }

    /// Change the file as the settings application does.
    pub(super) fn change(&self, change: impl FnOnce(&mut DesktopControlConf)) {
        DesktopControlConf::update(&self.path(), |conf| {
            change(conf);
            Ok(())
        })
        .unwrap();
    }

    /// [`Self::change`] a watched file and wait until the server read it.
    fn change_seen(
        &self,
        h: &mut Harness,
        c: &mut Controller,
        change: impl FnOnce(&mut DesktopControlConf),
    ) {
        let before = h.state.live.reloads;
        self.change(change);
        h.clients_until(
            &mut [&mut c.client],
            "the changed answers to be read",
            |s| s.live.reloads > before,
        );
    }

    fn answers(&self) -> DesktopControlConf {
        DesktopControlConf::load(&self.path()).unwrap().0
    }
}

/// Every service allowed.
pub(super) const ALLOW_ALL: [(Service, Decision); 2] = [
    (Service::Control, Decision::Allow),
    (Service::Capture, Decision::Allow),
];

/// A version 4 controller naming the process `pid` that started at `start`, whether or not it is one.
fn naming(h: &mut Harness, pid: u32, start: u64) -> Controller {
    let c = Controller::bound(h, 4);
    c.session.identify(pid, (start >> 32) as u32, start as u32);
    c
}

fn resume(h: &mut Harness, c: &mut Controller) -> Result<(), session::Refusal> {
    let n = c.serial();
    c.session.resume(n, c.hi, c.lo);
    c.ack(h, n)
}

/// Make the window of `app_id` the test process's only display connection, as the terminal an agent runs in; every
/// other connection stands for a process of its own.
fn runs_under(h: &mut Harness, app_id: &str) {
    let window = h
        .state
        .wm
        .windows()
        .find(|w| h.state.wm.app(w.app).is_some_and(|a| a.identity == app_id))
        .unwrap()
        .id;
    let client = h
        .state
        .entry(window)
        .unwrap()
        .surface()
        .client()
        .unwrap()
        .id();
    let pid = std::process::id();
    let (_, start) = cupertino_server::process::parent_and_start(pid).unwrap();
    h.state.process_of = Box::new(move |_, c| {
        if c.id() == client {
            return Some((pid, start));
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        c.id().hash(&mut hasher);
        Some((0, hasher.finish()))
    });
}

#[test]
fn a_remembered_allow_starts_the_run_at_once_without_a_panel() {
    let Some(mut h) = Harness::new() else { return };
    let (_target, _) = mapped_app(&mut h);
    let bridge = Bridge::spawn();
    let _store = Store::read_by(&mut h, &this_program(), &ALLOW_ALL);
    // no consent panel is connected at all
    let mut c = Controller::named(&mut h, &bridge);
    assert!(!h.state.control_capturing());
    assert_eq!(c.start(&mut h, 15), Ok(()));
    c.phase(&mut h, 2);
    assert!(
        h.state.control_capturing(),
        "the capture indicator shows while the run can capture"
    );
    let source = c.source();
    let capture = c.client.session(&mut h, &source, false);
    c.client.request_frame(&capture);
    h.state.request_redraw();
    h.pump_until(&mut c.client, "a frame of the allowed run", |s| {
        s.frame.ready
    });
    assert!(capture.pixels().contains(&RED));
    c.session.stop();
    c.phase(&mut h, 4);
    assert!(!h.state.control_capturing());
}

#[test]
fn a_service_turned_off_refuses_without_asking_and_the_other_still_starts() {
    let Some(mut h) = Harness::new() else { return };
    let (mut app, _authority) = authority(&mut h);
    let bridge = Bridge::spawn();
    let _store = Store::read_by(
        &mut h,
        &this_program(),
        &[
            (Service::Control, Decision::Allow),
            (Service::Capture, Decision::Deny),
        ],
    );
    let mut c = Controller::named(&mut h, &bridge);
    assert_eq!(c.start(&mut h, 15), Err(session::Refusal::Denied));
    assert_eq!(c.start(&mut h, 2), Err(session::Refusal::Denied));
    h.idle(&mut app, 5);
    assert!(app.state.control_request.is_none(), "nobody was asked");
    assert_eq!(c.client.state.control_seen.phase, 0);
    assert_eq!(c.start(&mut h, 1 | 4 | 8), Ok(()));
    c.phase(&mut h, 2);
    h.idle(&mut app, 5);
    assert!(app.state.control_request.is_none());
    let source = c.source();
    c.stopped_source(&mut h, &source);
}

#[test]
fn an_undecided_app_is_asked_once_and_its_allow_is_remembered() {
    let Some(mut h) = Harness::new() else { return };
    let entry = "[Desktop Entry]\nType=Application\nName=Terminal\nExec=terminal\n";
    h.state.desktop = DesktopIndex {
        entries: vec![parse_entry(TERMINAL, std::path::Path::new("x"), entry).unwrap()],
    };
    let (mut terminal, _) = mapped_app_named(&mut h, TERMINAL, None);
    h.idle(&mut terminal, 3);
    runs_under(&mut h, TERMINAL);
    let (mut app, authority) = authority(&mut h);
    let bridge = Bridge::spawn();
    let store = Store::read_by(&mut h, &this_program(), &[]);
    let mut c = Controller::named(&mut h, &bridge);
    assert_eq!(c.start(&mut h, 15), Ok(()));
    c.phase(&mut h, 1);
    h.pump_until(&mut app, "the app of the request", |s| {
        s.control_app.is_some()
    });
    assert_eq!(
        app.state.control_app,
        Some((
            c.client.state.control_seen.epoch,
            true,
            TERMINAL.to_string(),
            "Terminal".to_string(),
            3,
            true
        ))
    );
    approve(&mut h, &mut app, &authority, &mut c);
    let terminal = remembered::App::Desktop(TERMINAL.into());
    for service in Service::ALL {
        assert_eq!(
            store.answers().decision(service, &terminal),
            Some(Decision::Allow)
        );
    }
    assert_eq!(
        store.answers().decision(Service::Control, &this_program()),
        None
    );
    c.session.stop();
    c.phase(&mut h, 4);
    app.state.control_request = None;
    app.state.control_app = None;
    let mut next = Controller::named(&mut h, &bridge);
    assert_eq!(next.start(&mut h, 15), Ok(()));
    next.phase(&mut h, 2);
    h.idle(&mut app, 5);
    assert!(
        app.state.control_request.is_none(),
        "a remembered answer is not asked again"
    );
    assert!(app.state.control_app.is_none());
}

#[test]
fn a_program_is_named_by_its_path_and_only_a_refusal_the_person_chose_is_remembered() {
    let Some(mut h) = Harness::new() else { return };
    let (mut app, authority) = authority(&mut h);
    let bridge = Bridge::spawn();
    let store = Store::read_by(&mut h, &this_program(), &[]);
    let mut c = Controller::named(&mut h, &bridge);
    assert_eq!(c.start(&mut h, 2), Ok(()));
    c.phase(&mut h, 1);
    h.pump_until(&mut app, "the app of the request", |s| {
        s.control_app.is_some()
    });
    let program = std::env::current_exe().unwrap();
    let (_, desktop, identity, name, services, rememberable) =
        app.state.control_app.clone().unwrap();
    assert!(!desktop);
    assert_eq!(identity, program.to_str().unwrap());
    assert_eq!(name, program.file_name().unwrap().to_str().unwrap());
    assert_eq!(services, 2, "only capture is asked");
    assert!(rememberable);
    // closing the panel answers this run only
    authority.answer(c.hi, c.lo, 0, 0);
    app.pump();
    c.phase(&mut h, 0);
    assert_eq!(store.answers(), DesktopControlConf::default());
    // so does a refusal the person's own input did not give
    app.state.control_request = None;
    let mut c = Controller::named(&mut h, &bridge);
    assert_eq!(c.start(&mut h, 2), Ok(()));
    c.phase(&mut h, 1);
    h.pump_until(&mut app, "the second request", |s| {
        s.control_request.is_some()
    });
    authority.answer(c.hi, c.lo, 2, 0);
    app.pump();
    c.phase(&mut h, 0);
    assert_eq!(store.answers(), DesktopControlConf::default());
    // a refusal the person chose is remembered, for the service asked only
    app.state.control_request = None;
    let mut c = Controller::named(&mut h, &bridge);
    assert_eq!(c.start(&mut h, 2), Ok(()));
    c.phase(&mut h, 1);
    answer(&mut h, &mut app, &authority, &mut c, 2);
    c.phase(&mut h, 0);
    assert_eq!(
        store.answers().decision(Service::Capture, &this_program()),
        Some(Decision::Deny)
    );
    assert_eq!(
        store.answers().decision(Service::Control, &this_program()),
        None
    );
    app.state.control_request = None;
    let mut c = Controller::named(&mut h, &bridge);
    assert_eq!(c.start(&mut h, 2), Err(session::Refusal::Denied));
    h.idle(&mut app, 5);
    assert!(
        app.state.control_request.is_none(),
        "a remembered refusal is not asked again"
    );
}

#[test]
fn an_answer_that_cannot_be_written_holds_for_its_run_only() {
    use std::os::unix::fs::PermissionsExt;
    let Some(mut h) = Harness::new() else { return };
    let (mut app, authority) = authority(&mut h);
    let bridge = Bridge::spawn();
    let store = Store::holding(&this_program(), &ALLOW_ALL);
    // a file others may write is refused: it allows nothing and nothing is written over it
    std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o666)).unwrap();
    let text = std::fs::read_to_string(store.path()).unwrap();
    h.state.reload_desktop_control(&store.path());
    let mut c = Controller::named(&mut h, &bridge);
    assert_eq!(c.start(&mut h, 15), Ok(()));
    c.phase(&mut h, 1);
    approve(&mut h, &mut app, &authority, &mut c);
    assert_eq!(std::fs::read_to_string(store.path()).unwrap(), text);
    c.session.stop();
    c.phase(&mut h, 4);
    app.state.control_request = None;
    let mut next = Controller::named(&mut h, &bridge);
    assert_eq!(next.start(&mut h, 15), Ok(()));
    next.phase(&mut h, 1);
    h.pump_until(&mut app, "the request asked again", |s| {
        s.control_request.is_some()
    });
}

#[test]
fn turning_a_service_off_ends_the_run_and_turning_one_on_starts_nothing() {
    let Some(mut h) = Harness::new() else { return };
    let (mut app, _authority) = authority(&mut h);
    let bridge = Bridge::spawn();
    let program = this_program();
    let store = Store::watched_by(&mut h, &program, &ALLOW_ALL);
    let mut c = Controller::named(&mut h, &bridge);
    assert_eq!(c.start(&mut h, 15), Ok(()));
    c.phase(&mut h, 2);
    let other = remembered::App::Desktop("org.example.other".into());
    store.change_seen(&mut h, &mut c, |conf| {
        conf.set(Service::Capture, &other, Decision::Deny).unwrap()
    });
    h.idle(&mut c.client, 3);
    assert_eq!(
        c.client.state.control_seen.phase, 2,
        "another app's answer changes nothing"
    );
    store.change_seen(&mut h, &mut c, |conf| {
        conf.set(Service::Capture, &program, Decision::Deny)
            .unwrap()
    });
    c.phase(&mut h, 4);
    // removing the app's answer ends a run that holds it as well: the app would be asked again
    store.change_seen(&mut h, &mut c, |conf| {
        conf.set(Service::Capture, &program, Decision::Allow)
            .unwrap()
    });
    let mut c = Controller::named(&mut h, &bridge);
    assert_eq!(c.start(&mut h, 15), Ok(()));
    c.phase(&mut h, 2);
    store.change_seen(&mut h, &mut c, |conf| {
        conf.forget(Service::Control, &program);
    });
    c.phase(&mut h, 4);
    // a run that waits for the person is not started by a service turned on meanwhile, and is refused when it is
    // turned off
    let mut c = Controller::named(&mut h, &bridge);
    assert_eq!(c.start(&mut h, 15), Ok(()));
    c.phase(&mut h, 1);
    h.pump_until(&mut app, "the app of the request", |s| {
        s.control_app.is_some()
    });
    assert_eq!(
        app.state.control_app.as_ref().unwrap().4,
        1,
        "only control is asked"
    );
    store.change_seen(&mut h, &mut c, |conf| {
        conf.set(Service::Control, &program, Decision::Allow)
            .unwrap()
    });
    h.idle(&mut c.client, 5);
    assert_eq!(c.client.state.control_seen.phase, 1);
    store.change_seen(&mut h, &mut c, |conf| {
        conf.set(Service::Control, &program, Decision::Deny)
            .unwrap()
    });
    c.phase(&mut h, 0);
}

#[test]
fn a_run_turned_off_in_settings_stays_ended_and_a_fresh_session_on_the_connection_starts_again() {
    let Some(mut h) = Harness::new() else { return };
    let (mut app, _authority) = authority(&mut h);
    let bridge = Bridge::spawn();
    let program = this_program();
    let store = Store::watched_by(&mut h, &program, &ALLOW_ALL);
    let mut c = Controller::named(&mut h, &bridge);
    assert_eq!(c.start(&mut h, 15), Ok(()));
    c.phase(&mut h, 2);
    let first = c.client.state.control_seen.epoch;
    assert_eq!(
        c.start(&mut h, 15),
        Err(session::Refusal::Busy),
        "a run that started is not started again"
    );
    store.change_seen(&mut h, &mut c, |conf| {
        conf.set(Service::Control, &program, Decision::Deny)
            .unwrap()
    });
    c.phase(&mut h, 4);
    // turned off: the ended run's session is refused with denied, never with busy or stale_epoch
    assert_eq!(c.start(&mut h, 15), Err(session::Refusal::Denied));
    store.change_seen(&mut h, &mut c, |conf| {
        conf.set(Service::Control, &program, Decision::Allow)
            .unwrap()
    });
    // turned on again: the ended run stays ended
    assert_eq!(c.start(&mut h, 15), Err(session::Refusal::StaleEpoch));
    assert_eq!(c.client.state.control_seen.phase, 4);
    // a fresh session on the same connection has a fresh run, which starts at once without a panel
    c.renew(&mut h, &bridge);
    assert_ne!(c.client.state.control_seen.epoch, first);
    assert_eq!(c.start(&mut h, 15), Ok(()));
    c.phase(&mut h, 2);
    h.idle(&mut app, 5);
    assert!(app.state.control_request.is_none(), "nobody was asked");
    // turned off again: the fresh session of the ended run is refused with denied
    store.change_seen(&mut h, &mut c, |conf| {
        conf.set(Service::Control, &program, Decision::Deny)
            .unwrap()
    });
    c.phase(&mut h, 4);
    c.renew(&mut h, &bridge);
    assert_eq!(c.start(&mut h, 15), Err(session::Refusal::Denied));
    assert_eq!(c.client.state.control_seen.phase, 0);
}

#[test]
fn the_person_input_never_pauses_a_run_and_the_prompt_suspends_it_until_it_closes() {
    let Some(mut h) = Harness::new() else { return };
    let (mut target, _) = mapped_app_named(&mut h, "org.example.controlled", None);
    let qh = target.qh();
    let seat: wl_seat::WlSeat = target.globals.bind(&qh, 1..=7, ()).unwrap();
    let _keyboard = seat.get_keyboard(&qh, ());
    h.idle(&mut target, 3);
    let bridge = Bridge::spawn();
    let _store = Store::read_by(&mut h, &this_program(), &ALLOW_ALL);
    let mut c = Controller::named(&mut h, &bridge);
    assert_eq!(c.start(&mut h, 15), Ok(()));
    c.phase(&mut h, 2);
    for event in [
        common::absolute::motion(0.5, 0.5),
        common::absolute::button(true),
        common::absolute::button(false),
        common::absolute::key(30, true),
        common::absolute::key(30, false),
    ] {
        h.state.process_input_event(event);
    }
    h.idle(&mut c.client, 5);
    assert_eq!(
        c.client.state.control_seen.phase, 2,
        "the person's input does not pause the run"
    );
    assert_eq!(c.key(&mut h, 48, true), Ok(()));
    assert_eq!(c.key(&mut h, 48, false), Ok(()));
    let prompt = common::Client::prompt(common::prompt_agent(&h.state));
    h.clients_until(&mut [&mut c.client], "the prompt to be shown", |s| {
        s.prompt_shown()
    });
    c.phase(&mut h, 3);
    assert!(!h.state.control_capturing());
    assert_eq!(c.key(&mut h, 30, true), Err(session::Refusal::NotGranted));
    assert_eq!(resume(&mut h, &mut c), Err(session::Refusal::Blocked));
    prompt.unlock();
    // nobody is asked: no consent panel is even connected
    c.phase(&mut h, 2);
    assert!(h.state.control_capturing());
    assert_eq!(
        resume(&mut h, &mut c),
        Ok(()),
        "a run that resumed is resumed"
    );
    assert_eq!(c.key(&mut h, 30, true), Ok(()));
    assert_eq!(c.key(&mut h, 30, false), Ok(()));
    h.pump_until(&mut target, "the run's keys after the prompt", |s| {
        s.control_keys.ends_with(&[(30, true), (30, false)])
    });
}

#[test]
fn locking_ends_a_run_and_after_the_unlock_an_allowed_app_starts_at_once() {
    let Some(mut h) = Harness::new() else { return };
    let bridge = Bridge::spawn();
    let _store = Store::read_by(&mut h, &this_program(), &ALLOW_ALL);
    let mut c = Controller::named(&mut h, &bridge);
    assert_eq!(c.start(&mut h, 15), Ok(()));
    c.phase(&mut h, 2);
    let locker = locked(&mut h);
    c.phase(&mut h, 4);
    let mut during = Controller::named(&mut h, &bridge);
    assert_eq!(during.start(&mut h, 15), Err(session::Refusal::Blocked));
    locker.unlock();
    h.clients_until(&mut [&mut during.client], "the unlock", |s| {
        !s.session_lock.is_active()
    });
    let mut next = Controller::named(&mut h, &bridge);
    assert_eq!(next.start(&mut h, 15), Ok(()));
    next.phase(&mut h, 2);
}

#[test]
fn the_stop_shortcut_ends_the_run_and_keeps_what_the_person_decided() {
    let Some(mut h) = Harness::new() else { return };
    let bridge = Bridge::spawn();
    let store = Store::read_by(&mut h, &this_program(), &ALLOW_ALL);
    let before = store.answers();
    let mut c = Controller::named(&mut h, &bridge);
    assert_eq!(c.start(&mut h, 15), Ok(()));
    c.phase(&mut h, 2);
    for (code, pressed) in [
        (KEY_LEFTCTRL, true),
        (KEY_LEFTALT, true),
        (KEY_ESC, true),
        (KEY_ESC, false),
        (KEY_LEFTALT, false),
        (KEY_LEFTCTRL, false),
    ] {
        h.state
            .process_input_event(common::absolute::key(code, pressed));
    }
    c.phase(&mut h, 4);
    assert_eq!(store.answers(), before);
    let mut next = Controller::named(&mut h, &bridge);
    assert_eq!(next.start(&mut h, 15), Ok(()));
    next.phase(&mut h, 2);
}

#[test]
fn an_allowed_app_still_cannot_reach_the_consent_panel_or_a_session_panel() {
    let Some(mut h) = Harness::new() else { return };
    let (mut target, _) = mapped_app_named(&mut h, "org.example.controlled", None);
    h.idle(&mut target, 3);
    let target_window = h.state.wm.key_window().unwrap();
    let (mut app, _authority) = authority(&mut h);
    let bridge = Bridge::spawn();
    let _store = Store::read_by(&mut h, &this_program(), &ALLOW_ALL);
    let mut c = Controller::named(&mut h, &bridge);
    assert_eq!(c.start(&mut h, 15), Ok(()));
    c.phase(&mut h, 2);
    // the consent panel holds the keyboard
    assert_eq!(c.key(&mut h, 30, true), Err(session::Refusal::Blocked));
    h.idle(&mut app, 3);
    assert!(app.state.control_keys.is_empty());
    // another session panel taking the keyboard suspends the run until it lets go
    h.idle(&mut target, 3);
    let stream =
        shell_process_connection(&h.state.display_handle, &[ShellRole::SessionLock]).unwrap();
    let (mut panel, _) = mapped_app_stream(&mut h, stream, "org.example.panel", None);
    c.phase(&mut h, 3);
    assert_eq!(c.key(&mut h, 30, true), Err(session::Refusal::NotGranted));
    h.idle(&mut panel, 3);
    assert!(panel.state.control_keys.is_empty());
    h.state.command(cupertino_wm::Command::FocusWindow {
        window: target_window,
    });
    c.phase(&mut h, 2);
}

#[test]
fn a_controller_must_name_the_process_it_serves_once_and_truly() {
    let Some(mut h) = Harness::new() else { return };
    let (mut app, _authority) = authority(&mut h);
    let bridge = Bridge::spawn();
    let _store = Store::read_by(&mut h, &this_program(), &ALLOW_ALL);
    let mut c = naming(&mut h, bridge.pid, bridge.start + 1);
    assert_eq!(
        c.start(&mut h, 15),
        Err(session::Refusal::Unidentified),
        "another start is another process"
    );
    let gone = Bridge::spawn();
    let (pid, start) = (gone.pid, gone.start);
    drop(gone);
    let mut c = naming(&mut h, pid, start);
    assert_eq!(
        c.start(&mut h, 15),
        Err(session::Refusal::Unidentified),
        "a process that is gone"
    );
    let mut c = Controller::bound(&mut h, 4);
    assert_eq!(
        c.start(&mut h, 15),
        Err(session::Refusal::Unidentified),
        "a controller that names nobody"
    );
    h.idle(&mut app, 5);
    assert!(app.state.control_request.is_none());
    // a second name is a protocol error
    let mut c = Controller::named(&mut h, &bridge);
    c.session
        .identify(bridge.pid, (bridge.start >> 32) as u32, bridge.start as u32);
    let end = Instant::now() + common::HANG_GUARD;
    while c.client.conn.protocol_error().is_none() {
        assert!(Instant::now() < end, "a second name was accepted");
        let _ = c.client.conn.flush();
        h.step();
        if let Some(guard) = c.client.queue.prepare_read() {
            let _ = guard.read();
        }
        let _ = c.client.queue.dispatch_pending(&mut c.client.state);
    }
    let error = c.client.conn.protocol_error().unwrap();
    assert_eq!(error.object_interface, "cupertino_control_session_v1");
    assert_eq!(error.code, session::Error::Identity as u32);
}
