//! A menu a desktop-control action shows grabs as one a press opened would. With each action routed to a publisher of
//! version 2 comes a serial its popup may grab with: once, from that client, for a popup of the action's window, while
//! the action is handled and a moment after it is answered as applied. The grab then holds the keyboard and a press
//! anywhere else, in another application too, ends it. The serial stands for no press or input event, so a move, a
//! drag or an activation token that names it gets nothing; a lock ends the menu and withdraws a serial not used yet.
//! An older publisher is sent none.
use super::*;
use cupertino_protocols::client::semantics::{
    cupertino_semantic_surface_v1::{self as publication, CupertinoSemanticSurfaceV1},
    cupertino_semantics_v1::CupertinoSemanticsV1,
};
use cupertino_semantics as wire;
use cupertino_server::events::BTN_LEFT;
use smithay::backend::input::ButtonState;
use smithay::wayland::xdg_activation::XdgActivationToken;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::{wl_data_device, wl_data_device_manager, wl_data_source};
use wayland_protocols::xdg::activation::v1::client::{xdg_activation_token_v1, xdg_activation_v1};
use wayland_protocols::xdg::shell::client::{xdg_popup, xdg_positioner};

/// The publisher's window and its menus, in points.
const WINDOW: (i32, i32) = (240, 160);
const MENU: (i32, i32) = (100, 80);
/// The node the publisher offers the menu action on.
const OPENER: u64 = 2;
/// Evdev code of Escape.
const ESCAPE: u32 = 1;

/// An application that publishes its window's semantics and opens menus when told.
#[derive(Default)]
struct Publisher {
    app_id: String,
    shm: Option<wl_shm::WlShm>,
    files: Vec<std::fs::File>,
    seat: Option<wl_seat::WlSeat>,
    compositor: Option<wl_compositor::WlCompositor>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    window: Option<(
        wl_surface::WlSurface,
        xdg_surface::XdgSurface,
        xdg_toplevel::XdgToplevel,
    )>,
    publication: Option<CupertinoSemanticSurfaceV1>,
    /// The epoch of the active interest, and the one published under last.
    epoch: Option<u64>,
    published: Option<u64>,
    /// Every event of the publication after the interest, in order.
    heard: Vec<Heard>,
    /// The popups the server dismissed.
    done: Vec<ObjectId>,
    /// The surface that has the keyboard, and the serial of every keyboard enter.
    keyboard: Option<ObjectId>,
    enters: Vec<u32>,
    /// Keys pressed, with the surface that had the keyboard.
    keys: Vec<(Option<ObjectId>, u32)>,
    token: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum Heard {
    GrabSerial(u32),
    Perform(wire::ActionCall),
}

impl Publisher {
    /// The grab serial that came right before the latest perform.
    fn grab_serial(&self) -> Option<u32> {
        let n = self.heard.len();
        match self.heard.get(n.wrapping_sub(2)..) {
            Some([Heard::GrabSerial(serial), Heard::Perform(_)]) => Some(*serial),
            _ => None,
        }
    }
    fn calls(&self) -> Vec<wire::ActionCall> {
        self.heard
            .iter()
            .filter_map(|h| match h {
                Heard::Perform(call) => Some(call.clone()),
                Heard::GrabSerial(_) => None,
            })
            .collect()
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Publisher {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
ignore_events!(Publisher: wl_compositor::WlCompositor, wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_buffer::WlBuffer,
    wl_surface::WlSurface, xdg_toplevel::XdgToplevel, xdg_positioner::XdgPositioner, wl_seat::WlSeat,
    wl_pointer::WlPointer, CupertinoSemanticsV1, xdg_activation_v1::XdgActivationV1,
    wl_data_device_manager::WlDataDeviceManager, wl_data_device::WlDataDevice, wl_data_source::WlDataSource);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Publisher {
    fn event(
        _: &mut Self,
        base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}

/// An xdg surface is drawn at its size, in one colour, at each configure.
impl Dispatch<xdg_surface::XdgSurface, (wl_surface::WlSurface, (i32, i32))> for Publisher {
    fn event(
        state: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        (surface, size): &(wl_surface::WlSurface, (i32, i32)),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            let (buffer, file) = shm_buffer(state.shm.as_ref().unwrap(), *size, |_, _| RED, qh);
            state.files.push(file);
            surface.attach(Some(&buffer), 0, 0);
            surface.damage_buffer(0, 0, size.0, size.1);
            surface.commit();
        }
    }
}

impl Dispatch<xdg_popup::XdgPopup, ()> for Publisher {
    fn event(
        state: &mut Self,
        popup: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_popup::Event::PopupDone = event {
            state.done.push(popup.id());
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for Publisher {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Enter {
                serial, surface, ..
            } => {
                state.keyboard = Some(surface.id());
                state.enters.push(serial);
            }
            wl_keyboard::Event::Leave { .. } => state.keyboard = None,
            wl_keyboard::Event::Key {
                key,
                state: wayland_client::WEnum::Value(wl_keyboard::KeyState::Pressed),
                ..
            } => state.keys.push((state.keyboard.clone(), key)),
            _ => {}
        }
    }
}

impl Dispatch<CupertinoSemanticSurfaceV1, ()> for Publisher {
    fn event(
        state: &mut Self,
        _: &CupertinoSemanticSurfaceV1,
        event: publication::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            publication::Event::Interest {
                epoch_hi,
                epoch_lo,
                active,
            } => {
                state.epoch =
                    (active != 0).then_some((u64::from(epoch_hi) << 32) | u64::from(epoch_lo));
            }
            publication::Event::GrabSerial { serial } => {
                state.heard.push(Heard::GrabSerial(serial))
            }
            publication::Event::Perform { payload, length } => {
                let call = wire::read(payload, length, wire::MAX_FRAME).unwrap();
                state.heard.push(Heard::Perform(call));
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, ()> for Publisher {
    fn event(
        state: &mut Self,
        _: &xdg_activation_token_v1::XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            state.token = Some(token);
        }
    }
}

/// A menu the publisher opened.
struct Menu {
    popup: xdg_popup::XdgPopup,
    surface: wl_surface::WlSurface,
}

impl TestClient<Publisher> {
    /// A publisher of `app_id` whose window is mapped, with a keyboard and a pointer, publishing at `version`.
    fn publisher(
        h: &mut Harness,
        app_id: &str,
        version: u32,
        others: &mut [&mut dyn Pump],
    ) -> Self {
        let stream = UnixStream::connect(h.socket_path()).unwrap();
        let mut p = TestClient::connect(
            h,
            stream,
            Publisher {
                app_id: app_id.to_owned(),
                ..Publisher::default()
            },
        );
        let qh = p.qh();
        p.state.shm = Some(p.globals.bind(&qh, 1..=1, ()).unwrap());
        let compositor: wl_compositor::WlCompositor = p.globals.bind(&qh, 4..=6, ()).unwrap();
        let wm_base: xdg_wm_base::XdgWmBase = p.globals.bind(&qh, 1..=5, ()).unwrap();
        let seat: wl_seat::WlSeat = p.globals.bind(&qh, 1..=7, ()).unwrap();
        seat.get_keyboard(&qh, ());
        seat.get_pointer(&qh, ());
        let surface = compositor.create_surface(&qh, ());
        let xdg = wm_base.get_xdg_surface(&surface, &qh, (surface.clone(), WINDOW));
        let toplevel = xdg.get_toplevel(&qh, ());
        toplevel.set_app_id(app_id.to_owned());
        surface.commit();
        let semantics: CupertinoSemanticsV1 = p.globals.bind(&qh, version..=version, ()).unwrap();
        p.state.publication = Some(semantics.get_surface(&surface, &qh, ()));
        p.state.window = Some((surface, xdg, toplevel));
        p.state.seat = Some(seat);
        p.state.compositor = Some(compositor);
        p.state.wm_base = Some(wm_base);
        let mut clients: Vec<&mut dyn Pump> = vec![&mut p];
        clients.extend(others.iter_mut().map(|c| &mut **c as &mut dyn Pump));
        h.clients_until(&mut clients, "the publisher's window to map", |s| {
            s.wm.windows().any(|w| {
                s.entry(w.id).is_some_and(|e| e.mapped)
                    && s.wm.app(w.app).is_some_and(|a| a.identity == app_id)
            })
        });
        let time = h.state.now() + 5.0;
        common::set_time_with(&mut h.state, time, |state| {
            p.pump();
            headless::step(state, &mut h.event_loop, Duration::from_millis(2)).unwrap();
            p.pump();
        });
        h.state.tick();
        p
    }

    fn surface(&self) -> &wl_surface::WlSurface {
        &self.state.window.as_ref().unwrap().0
    }

    /// The publisher's window, as the server knows it.
    fn window(&self, h: &Harness) -> cupertino_wm::WindowId {
        h.state
            .wm
            .windows()
            .find(|w| {
                h.state
                    .wm
                    .app(w.app)
                    .is_some_and(|a| a.identity == self.state.app_id)
            })
            .map(|w| w.id)
            .unwrap()
    }

    /// Whether the publisher's window holds the popup grab.
    fn grabbing(&self, h: &Harness) -> bool {
        let surface = h.state.entry(self.window(h)).unwrap().surface();
        h.state.popup_grab_root() == Some(surface)
    }

    /// Publish a window with one button that shows a menu, at `revision`.
    fn publish(&mut self, h: &mut Harness, revision: u64) {
        h.pump_until(self, "a new active interest", |s| {
            s.epoch.is_some() && s.epoch != s.published
        });
        let node =
            |id: u64, role: &str, actions: Vec<wire::Action>, children: Vec<u64>| wire::Node {
                truncated: false,
                position: wire::Position::default(),
                id,
                role: role.into(),
                name: Some(role.into()),
                description: None,
                value: None,
                states: wire::States::default(),
                bounds: wire::Bounds {
                    x: 0.0,
                    y: 0.0,
                    width: 40.0,
                    height: 20.0,
                },
                actions,
                children,
            };
        let snapshot = wire::Snapshot {
            revision,
            root: 1,
            focus: None,
            nodes: vec![
                node(1, "window", vec![], vec![OPENER]),
                node(OPENER, "button", vec![wire::Action::ShowMenu], vec![]),
            ],
        };
        let (file, len) = wire::seal(&snapshot, wire::MAX_FRAME).unwrap();
        let epoch = self.state.epoch.unwrap();
        self.state.published = Some(epoch);
        self.state.publication.as_ref().unwrap().publish(
            (epoch >> 32) as u32,
            epoch as u32,
            file.as_fd(),
            len,
        );
        h.idle(self, 5);
    }

    /// Answer the latest action with `outcome`.
    fn complete(&mut self, h: &mut Harness, outcome: wire::Outcome, revision: u64) {
        let call = self.state.calls().pop().unwrap();
        let reply = wire::ActionReply {
            id: call.id,
            epoch: call.epoch,
            revision,
            outcome,
        };
        let (file, len) = wire::seal(&reply, wire::MAX_FRAME).unwrap();
        self.state
            .publication
            .as_ref()
            .unwrap()
            .complete(file.as_fd(), len);
        h.idle(self, 3);
    }

    /// Open a menu of the window that grabs with `serial`.
    fn menu(&mut self, h: &mut Harness, serial: u32) -> Menu {
        let qh = self.qh();
        let s = &self.state;
        let positioner = s.wm_base.as_ref().unwrap().create_positioner(&qh, ());
        positioner.set_size(MENU.0, MENU.1);
        positioner.set_anchor_rect(10, 10, 1, 1);
        positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
        positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
        let surface = s.compositor.as_ref().unwrap().create_surface(&qh, ());
        let xdg =
            s.wm_base
                .as_ref()
                .unwrap()
                .get_xdg_surface(&surface, &qh, (surface.clone(), MENU));
        let parent = &s.window.as_ref().unwrap().1;
        let popup = xdg.get_popup(Some(parent), &positioner, &qh, ());
        popup.grab(s.seat.as_ref().unwrap(), serial);
        surface.commit();
        positioner.destroy();
        h.idle(self, 5);
        Menu { popup, surface }
    }

    fn dismissed(&self, menu: &Menu) -> bool {
        self.state.done.contains(&menu.popup.id())
    }
}

impl Controller {
    /// Ask for the menu of the publisher's window; the publisher hears the action.
    fn show_menu(&mut self, h: &mut Harness, p: &mut TestClient<Publisher>, revision: u64) -> u32 {
        let entry = p.window(h);
        let invocation = wire::Invocation {
            window: entry.0 * 2,
            revision,
            node: OPENER,
            action: wire::Action::ShowMenu,
            data: None,
        };
        let (file, len) = wire::seal(&invocation, wire::MAX_FRAME).unwrap();
        let n = self.serial();
        self.session.perform(n, self.hi, self.lo, file.as_fd(), len);
        self.client.pump();
        let before = p.state.calls().len();
        h.pump_until(p, "the action at the publisher", |s| {
            s.calls().len() > before
        });
        n
    }
}

/// A desktop-control run that observes and acts, with a publisher of `version` whose window shows a menu.
struct Acting {
    _consent: (TestClient<App>, Authority),
    p: TestClient<Publisher>,
    c: Controller,
}

fn acting(h: &mut Harness, version: u32) -> Acting {
    let (mut app, authority) = authority(h);
    let mut p = TestClient::publisher(h, "org.example.menus", version, &mut [&mut app]);
    let mut c = Controller::bound(h, 3);
    // observe and semantic actions
    c.start(h, 1 | 16).unwrap();
    approve(h, &mut app, &authority, &mut c);
    p.publish(h, 1);
    Acting {
        _consent: (app, authority),
        p,
        c,
    }
}

/// A press and release at `at`, in logical points.
fn click(h: &mut Harness, at: (f64, f64)) {
    h.state.pointer_moved_to(at.into(), 0, None);
    h.step();
    h.state.pointer_button(BTN_LEFT, ButtonState::Pressed, 0);
    h.step();
    h.state.pointer_button(BTN_LEFT, ButtonState::Released, 0);
    h.step();
}

#[test]
fn a_menu_an_action_shows_grabs_holds_the_keyboard_and_a_press_in_another_application_ends_it() {
    let Some(mut h) = Harness::new() else { return };
    let Acting {
        _consent,
        mut p,
        mut c,
    } = acting(&mut h, 2);
    let stream = UnixStream::connect(h.socket_path()).unwrap();
    let (mut other, frame) =
        mapped_app_beside(&mut h, stream, "org.example.other", None, &mut [&mut p]);
    let qh = other.qh();
    let seat: wl_seat::WlSeat = other.globals.bind(&qh, 1..=7, ()).unwrap();
    seat.get_pointer(&qh, ());
    h.idle(&mut other, 3);

    let action = c.show_menu(&mut h, &mut p, 1);
    // the publisher never had a press: the grab serial, sent right before its action, is all it grabs with
    let grab = serial_of(&p);
    let menu = p.menu(&mut h, grab);
    assert!(!p.dismissed(&menu), "the action's serial grabs");
    assert!(p.grabbing(&h), "the menu holds the grab");
    p.complete(&mut h, wire::Outcome::Applied, 1);
    assert_eq!(c.ack(&mut h, action), Ok(()));
    // the grab holds the keyboard: its keys go to the menu, entered under a serial of their own
    h.idle(&mut p, 3);
    assert_eq!(p.state.keyboard, Some(menu.surface.id()));
    assert!(!p.state.enters.contains(&grab));
    h.state
        .process_input_event(common::absolute::key(ESCAPE, true));
    h.state
        .process_input_event(common::absolute::key(ESCAPE, false));
    h.idle(&mut p, 3);
    assert_eq!(
        p.state.keys.last(),
        Some(&(Some(menu.surface.id()), ESCAPE))
    );

    // a press on another application's window ends the grab, dismisses the menu and reaches nothing
    let presses = other.state.control_buttons.len();
    click(&mut h, (frame.x + frame.w - 10.0, frame.y + frame.h - 10.0));
    h.idle(&mut other, 3);
    h.idle(&mut p, 3);
    assert!(p.dismissed(&menu), "the press elsewhere dismissed the menu");
    assert!(h.state.popup_grab_root().is_none());
    assert_eq!(
        other.state.control_buttons.len(),
        presses,
        "the press that closed the menu reached no window"
    );

    // the serial is spent: a second menu that names it is dismissed at once and grabs nothing
    let again = p.menu(&mut h, grab);
    assert!(p.dismissed(&again), "a spent serial grabs nothing");
    assert!(h.state.popup_grab_root().is_none());
}

/// The grab serial of the latest action the publisher heard.
fn serial_of(p: &TestClient<Publisher>) -> u32 {
    p.state
        .grab_serial()
        .expect("the action came right after a grab serial")
}

#[test]
fn an_action_grab_serial_stands_for_no_press_or_input_event() {
    let Some(mut h) = Harness::new() else { return };
    let Acting {
        _consent,
        mut p,
        mut c,
    } = acting(&mut h, 2);
    let qh = p.qh();
    let action = c.show_menu(&mut h, &mut p, 1);
    let grab = serial_of(&p);

    // an activation token made with it answers no input event
    let activation: xdg_activation_v1::XdgActivationV1 = p.globals.bind(&qh, 1..=1, ()).unwrap();
    let token = activation.get_activation_token(&qh, ());
    token.set_serial(grab, p.state.seat.as_ref().unwrap());
    token.set_surface(p.surface());
    token.commit();
    h.pump_until(&mut p, "the activation token", |s| s.token.is_some());
    let made = XdgActivationToken::from(p.state.token.clone().unwrap());
    let valid = h
        .state
        .xdg_activation_state
        .data_for_token(&made)
        .and_then(|data| {
            data.user_data
                .get::<cupertino_server::handlers::TokenGrant>()
        })
        .unwrap()
        .valid;
    assert!(!valid, "a token of the action's serial is not valid");

    // while its menu is open, a move, a resize or a drag that names it starts nothing
    let menu = p.menu(&mut h, grab);
    assert!(p.grabbing(&h));
    let seat = p.state.seat.clone().unwrap();
    let (_, _, toplevel) = p.state.window.as_ref().unwrap();
    toplevel._move(&seat, grab);
    toplevel.resize(&seat, grab, xdg_toplevel::ResizeEdge::BottomRight);
    let manager: wl_data_device_manager::WlDataDeviceManager =
        p.globals.bind(&qh, 1..=3, ()).unwrap();
    let device = manager.get_data_device(&seat, &qh, ());
    let source = manager.create_data_source(&qh, ());
    source.offer("text/plain".into());
    let icon = p.state.compositor.as_ref().unwrap().create_surface(&qh, ());
    device.start_drag(Some(&source), &menu.surface, Some(&icon), grab);
    h.idle(&mut p, 5);
    assert!(!h.state.wm.is_interacting(), "no move or resize began");
    assert!(h.state.drag_icon.is_none(), "no drag began");
    assert!(!p.dismissed(&menu), "and the menu keeps its grab");
    assert!(p.grabbing(&h));
    p.complete(&mut h, wire::Outcome::Applied, 1);
    assert_eq!(c.ack(&mut h, action), Ok(()));
}

#[test]
fn an_action_grab_serial_is_refused_to_another_client_soon_after_the_answer_and_after_the_deadline()
{
    let Some(mut h) = Harness::new() else { return };
    let Acting {
        _consent: (mut app, _authority),
        mut p,
        mut c,
    } = acting(&mut h, 2);
    // another application that publishes too: it is not sent the serial, and may not use it
    let mut thief = TestClient::publisher(&mut h, "org.example.thief", 2, &mut [&mut p, &mut app]);
    let first = c.show_menu(&mut h, &mut p, 1);
    let grab = serial_of(&p);
    assert!(
        thief.state.heard.is_empty(),
        "only the action's publisher hears it"
    );
    let stolen = thief.menu(&mut h, grab);
    assert!(
        thief.dismissed(&stolen),
        "another client's grab with it is refused"
    );
    assert!(h.state.popup_grab_root().is_none());
    // which leaves it to its own client
    let menu = p.menu(&mut h, grab);
    assert!(!p.dismissed(&menu));
    assert!(p.grabbing(&h));
    menu.popup.destroy();
    p.complete(&mut h, wire::Outcome::Applied, 1);
    assert_eq!(c.ack(&mut h, first), Ok(()));
    h.idle(&mut p, 3);
    assert!(h.state.popup_grab_root().is_none());

    // an action answered as applied leaves its serial good a moment, for a menu its work shows a little later
    let second = c.show_menu(&mut h, &mut p, 1);
    let answered = serial_of(&p);
    assert_ne!(answered, grab, "each action has a serial of its own");
    p.complete(&mut h, wire::Outcome::Applied, 1);
    assert_eq!(c.ack(&mut h, second), Ok(()));
    let soon = p.menu(&mut h, answered);
    assert!(
        !p.dismissed(&soon),
        "a menu shown right after the answer grabs"
    );
    assert!(p.grabbing(&h));
    soon.popup.destroy();
    h.idle(&mut p, 3);
    assert!(h.state.popup_grab_root().is_none());
    // but not once that moment is past
    let third = c.show_menu(&mut h, &mut p, 1);
    let answered = serial_of(&p);
    p.complete(&mut h, wire::Outcome::Applied, 1);
    assert_eq!(c.ack(&mut h, third), Ok(()));
    let moment = Instant::now() + Duration::from_millis(1200);
    while Instant::now() < moment {
        h.idle(&mut p, 1);
        std::thread::sleep(Duration::from_millis(20));
    }
    let late = p.menu(&mut h, answered);
    assert!(
        p.dismissed(&late),
        "the serial of an action answered a while ago is refused"
    );
    assert!(h.state.popup_grab_root().is_none());
    // an answer of any other outcome takes the serial at once
    let fourth = c.show_menu(&mut h, &mut p, 1);
    let unavailable = serial_of(&p);
    p.complete(&mut h, wire::Outcome::Unavailable, 1);
    assert_eq!(c.ack(&mut h, fourth), Ok(()));
    let refused = p.menu(&mut h, unavailable);
    assert!(
        p.dismissed(&refused),
        "the serial of an action not applied is refused"
    );
    assert!(h.state.popup_grab_root().is_none());

    // an action left unanswered past its deadline ends without an outcome, and its serial with it
    let fifth = c.show_menu(&mut h, &mut p, 1);
    let expired = serial_of(&p);
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        h.idle(&mut p, 1);
        app.pump();
        thief.pump();
        c.client.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(c.ack(&mut h, fifth), Err(session::Refusal::UnknownOutcome));
    let stale = p.menu(&mut h, expired);
    assert!(p.dismissed(&stale), "an expired serial is refused");
    assert!(h.state.popup_grab_root().is_none());
}

/// Lock the session; the publisher hears what the lock does.
fn lock_session(h: &mut Harness, p: &mut TestClient<Publisher>) -> common::Client {
    let locker = locked(h);
    h.idle(p, 3);
    locker
}

/// Unlock the session and wait until the server has.
fn unlock(h: &mut Harness, p: &mut TestClient<Publisher>, locker: common::Client) {
    locker.unlock();
    let end = Instant::now() + common::HANG_GUARD;
    while h.state.session_lock.is_active() && Instant::now() < end {
        h.idle(p, 1);
    }
    assert!(!h.state.session_lock.is_active(), "the session unlocked");
}

#[test]
fn a_lock_ends_a_menu_an_action_showed_and_withdraws_a_serial_not_used_yet() {
    let Some(mut h) = Harness::new() else { return };
    let Acting {
        _consent: (mut app, authority),
        mut p,
        mut c,
    } = acting(&mut h, 2);
    c.show_menu(&mut h, &mut p, 1);
    let menu = p.menu(&mut h, serial_of(&p));
    assert!(p.grabbing(&h));
    let locker = lock_session(&mut h, &mut p);
    h.pump_until(&mut p, "the lock to dismiss the menu", |s| {
        s.done.contains(&menu.popup.id())
    });
    assert!(h.state.popup_grab_root().is_none());
    unlock(&mut h, &mut p, locker);

    // the lock ended the run; a new one hands out a serial, and a lock before it is used withdraws it
    let mut c = Controller::bound(&mut h, 3);
    c.start(&mut h, 1 | 16).unwrap();
    approve(&mut h, &mut app, &authority, &mut c);
    p.publish(&mut h, 2);
    c.show_menu(&mut h, &mut p, 2);
    let unused = serial_of(&p);
    let locker = lock_session(&mut h, &mut p);
    let early = p.menu(&mut h, unused);
    assert!(
        h.state.popup_grab_root().is_none(),
        "nothing grabs under the lock"
    );
    unlock(&mut h, &mut p, locker);
    early.popup.destroy();
    let after = p.menu(&mut h, unused);
    assert!(p.dismissed(&after), "the lock withdrew the serial");
    assert!(h.state.popup_grab_root().is_none());
}

#[test]
fn an_older_publisher_is_sent_no_grab_serial() {
    let Some(mut h) = Harness::new() else { return };
    let Acting {
        _consent,
        mut p,
        mut c,
    } = acting(&mut h, 1);
    let action = c.show_menu(&mut h, &mut p, 1);
    assert_eq!(p.state.heard.len(), 1);
    assert!(matches!(p.state.heard[0], Heard::Perform(_)));
    p.complete(&mut h, wire::Outcome::Applied, 1);
    assert_eq!(c.ack(&mut h, action), Ok(()));
}
