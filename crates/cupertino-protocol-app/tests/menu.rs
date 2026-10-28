//! The menu announcement against a minimal in-process server that offers `cupertino_app_effects_v1` at version 1 or 2.

use std::os::unix::net::UnixStream;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_app_protocols::server::app_effects::cupertino_app_effects_v1::{
    self, CupertinoAppEffectsV1,
};
use cupertino_app_protocols::server::app_effects::cupertino_app_menu_v1::{
    self, CupertinoAppMenuV1,
};
use cupertino_protocol_app::{AppEffects, AppEvent, MenuAddress};
use cupertino_wayland_client::{Connection, Error, Event, ExternalRole, SurfaceId};
use wayland_protocols::xdg::shell::server::xdg_wm_base::XdgWmBase;
use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::protocol::wl_compositor::{self, WlCompositor};
use wayland_server::protocol::wl_shm::WlShm;
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, New, Resource,
};

/// What the server received from the menu object.
#[derive(Debug, PartialEq, Eq)]
enum Seen {
    Address([String; 5]),
    Destroyed,
}

struct Mock {
    seen: Sender<Seen>,
    /// The `menu_bar` value sent when the menu object is made.
    shown: u32,
    /// A `menu_bar` value sent in answer to `set_address`.
    shown_on_address: Option<u32>,
    /// The `menu_reader` value sent when a version 3 menu object is made.
    reader: &'static str,
}

struct Data;
impl ClientData for Data {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

macro_rules! plain_global {
    ($($interface:ty),*) => {$(
        impl GlobalDispatch<$interface, ()> for Mock {
            fn bind(_: &mut Self, _: &DisplayHandle, _: &Client, r: New<$interface>, _: &(), init: &mut DataInit<'_, Self>) {
                init.init(r, ());
            }
        }
    )*};
}

plain_global!(WlCompositor, WlShm, XdgWmBase, CupertinoAppEffectsV1);

macro_rules! ignore_requests {
    ($($interface:ty),*) => {$(
        impl Dispatch<$interface, ()> for Mock {
            fn request(_: &mut Self, _: &Client, _: &$interface, _: <$interface as Resource>::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, Self>) {}
        }
    )*};
}

ignore_requests!(WlShm, XdgWmBase, WlSurface);

impl Dispatch<WlCompositor, ()> for Mock {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WlCompositor,
        request: wl_compositor::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_compositor::Request::CreateSurface { id } = request {
            init.init(id, ());
        }
    }
}

impl Dispatch<CupertinoAppEffectsV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &CupertinoAppEffectsV1,
        request: cupertino_app_effects_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let cupertino_app_effects_v1::Request::GetMenu { id, .. } = request {
            let menu = init.init(id, ());
            menu.menu_bar(state.shown);
            if menu.version() >= 3 {
                menu.menu_reader(state.reader.to_string());
            }
        }
    }
}

impl Dispatch<CupertinoAppMenuV1, ()> for Mock {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &CupertinoAppMenuV1,
        request: cupertino_app_menu_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let seen = match request {
            cupertino_app_menu_v1::Request::SetAddress {
                bus_name,
                application_object_path,
                window_object_path,
                menubar_object_path,
                app_menu_object_path,
            } => {
                if let Some(shown) = state.shown_on_address {
                    resource.menu_bar(shown);
                }
                Seen::Address([
                    bus_name,
                    application_object_path,
                    window_object_path,
                    menubar_object_path,
                    app_menu_object_path,
                ])
            }
            cupertino_app_menu_v1::Request::Destroy => Seen::Destroyed,
            _ => return,
        };
        state.seen.send(seen).unwrap();
    }
}

fn connect(
    effects_version: u32,
    shown: u32,
    shown_on_address: Option<u32>,
    reader: &'static str,
) -> (Connection, Receiver<Seen>) {
    let (client, server) = UnixStream::pair().unwrap();
    let (seen, received) = channel();
    std::thread::spawn(move || {
        let mut display = Display::<Mock>::new().unwrap();
        let mut dh = display.handle();
        dh.create_global::<Mock, WlCompositor, _>(6, ());
        dh.create_global::<Mock, WlShm, _>(1, ());
        dh.create_global::<Mock, XdgWmBase, _>(6, ());
        dh.create_global::<Mock, CupertinoAppEffectsV1, _>(effects_version, ());
        dh.insert_client(server, Arc::new(Data)).unwrap();
        let mut mock = Mock {
            seen,
            shown,
            shown_on_address,
            reader,
        };
        loop {
            if display.dispatch_clients(&mut mock).is_err() {
                return;
            }
            let _ = display.flush_clients();
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    (Connection::from_stream(client).unwrap(), received)
}

struct NoRole;

impl ExternalRole for NoRole {
    fn ack_configure(&self, _: u32) {}
    fn destroy(&self) {}
}

fn surface(conn: &mut Connection) -> SurfaceId {
    conn.external_surface(|_, _, _| NoRole)
}

fn menu_events(conn: &mut Connection) -> Vec<AppEvent> {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        let events: Vec<AppEvent> = conn
            .wait(Some(Duration::from_millis(50)))
            .unwrap()
            .into_iter()
            .filter_map(|e| match e {
                Event::Extension(e) => e.downcast::<AppEvent>().ok().map(|e| (*e).clone()),
                _ => None,
            })
            .collect();
        if !events.is_empty() {
            return events;
        }
    }
    Vec::new()
}

#[test]
fn a_window_announces_its_menus_and_hears_whether_the_bar_shows_them() {
    let (mut conn, seen) = connect(2, 1, None, ":1.7");
    let effects = AppEffects::bind(&mut conn).unwrap();
    let window = surface(&mut conn);
    let menu = effects.menu(&conn, window).unwrap().expect("version 2");
    assert_eq!(
        menu_events(&mut conn),
        [AppEvent::MenuBar {
            surface: window,
            shown: true
        }]
    );

    menu.set_address(&MenuAddress {
        bus_name: ":1.42".into(),
        application_object_path: "/org/example/Notes".into(),
        window_object_path: "/org/example/Notes/window/1".into(),
        menubar_object_path: "/org/example/Notes/window/1/menus/menubar".into(),
        app_menu_object_path: None,
    });
    drop(menu);
    conn.flush().unwrap();
    let timeout = Duration::from_secs(5);
    assert_eq!(
        seen.recv_timeout(timeout).unwrap(),
        Seen::Address(
            [
                ":1.42",
                "/org/example/Notes",
                "/org/example/Notes/window/1",
                "/org/example/Notes/window/1/menus/menubar",
                "",
            ]
            .map(String::from)
        )
    );
    assert_eq!(seen.recv_timeout(timeout).unwrap(), Seen::Destroyed);
}

#[test]
fn a_version_1_server_takes_no_menu_announcement() {
    let (mut conn, _seen) = connect(1, 1, None, ":1.7");
    let effects = AppEffects::bind(&mut conn).unwrap();
    let window = surface(&mut conn);
    assert!(effects.menu(&conn, window).unwrap().is_none());
}

#[test]
fn a_shown_value_other_than_0_or_1_fails_the_connection() {
    let (mut conn, seen) = connect(2, 0, Some(2), ":1.7");
    let effects = AppEffects::bind(&mut conn).unwrap();
    let window = surface(&mut conn);
    let menu = effects.menu(&conn, window).unwrap().expect("version 2");
    assert_eq!(
        menu_events(&mut conn),
        [AppEvent::MenuBar {
            surface: window,
            shown: false
        }]
    );

    menu.set_address(&MenuAddress {
        bus_name: ":1.42".into(),
        application_object_path: "/org/example/Notes".into(),
        window_object_path: "/org/example/Notes/window/1".into(),
        menubar_object_path: "/org/example/Notes/window/1/menus/menubar".into(),
        app_menu_object_path: None,
    });
    conn.flush().unwrap();
    assert!(matches!(
        seen.recv_timeout(Duration::from_secs(5)).unwrap(),
        Seen::Address(_)
    ));
    let end = Instant::now() + Duration::from_secs(5);
    let error = loop {
        match conn.wait(Some(Duration::from_millis(50))) {
            Err(error) => break error,
            Ok(_) => assert!(Instant::now() < end, "the connection did not fail"),
        }
    };
    assert!(matches!(error, Error::Protocol(_)), "{error}");
}

/// The menu events until `count` have arrived.
fn menu_events_until(conn: &mut Connection, count: usize) -> Vec<AppEvent> {
    let mut events = Vec::new();
    while events.len() < count {
        let batch = menu_events(conn);
        assert!(!batch.is_empty(), "only {events:?} arrived");
        events.extend(batch);
    }
    events
}

#[test]
fn a_version_3_window_is_told_which_connection_reads_its_menus() {
    for (reader, told) in [(":1.7", Some(":1.7".to_string())), ("", None)] {
        let (mut conn, _seen) = connect(3, 1, None, reader);
        let effects = AppEffects::bind(&mut conn).unwrap();
        let window = surface(&mut conn);
        let _menu = effects.menu(&conn, window).unwrap().expect("version 3");
        assert_eq!(
            menu_events_until(&mut conn, 2),
            [
                AppEvent::MenuBar {
                    surface: window,
                    shown: true
                },
                AppEvent::MenuReader {
                    surface: window,
                    bus_name: told
                },
            ]
        );
    }
}

#[test]
fn a_reader_that_is_not_a_unique_name_fails_the_connection() {
    let (mut conn, _seen) = connect(3, 1, None, "org.example.Bar");
    let effects = AppEffects::bind(&mut conn).unwrap();
    let window = surface(&mut conn);
    let _menu = effects.menu(&conn, window).unwrap().expect("version 3");
    let end = Instant::now() + Duration::from_secs(5);
    let error = loop {
        match conn.wait(Some(Duration::from_millis(50))) {
            Err(error) => break error,
            Ok(_) => assert!(Instant::now() < end, "the connection did not fail"),
        }
    };
    assert!(matches!(error, Error::Protocol(_)), "{error}");
}
