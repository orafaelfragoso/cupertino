//! The worker's `com.canonical.AppMenu.Registrar` on a private session bus: its signals, its answers for a window
//! registered and one that is not, its list of every window, and the registrations of a connection that leaves.

// a test owns its thread: it blocks on the bus
#![allow(clippy::disallowed_methods)]

use zbus::zvariant::OwnedObjectPath;
use zbus::{Connection, Message};

use cupertino_menubar::dbus::registrar::{REGISTRAR_NAME, REGISTRAR_PATH};
use cupertino_menubar::worker::Worker;

use crate::bus::{next_signal, signals, soon, Bus};

const INTERFACE: &str = REGISTRAR_NAME;

/// Call `$method` of the registrar served by `$registrar` with `$body`.
macro_rules! call {
    ($conn:expr, $registrar:expr, $method:expr, $body:expr) => {
        $conn
            .call_method(
                Some(AsRef::<str>::as_ref(&$registrar)),
                REGISTRAR_PATH,
                Some(INTERFACE),
                $method,
                $body,
            )
            .await
    };
}

type Menus = Vec<(u32, String, OwnedObjectPath)>;

async fn register(conn: &Connection, registrar: &str, window: u32, path: &str) {
    let path = OwnedObjectPath::try_from(path).unwrap();
    call!(conn, registrar, "RegisterWindow", &(window, path)).unwrap();
}

async fn menus(conn: &Connection, registrar: &str) -> Menus {
    let reply = call!(conn, registrar, "GetMenus", &()).unwrap();
    reply.body().deserialize().unwrap()
}

async fn menu_for(conn: &Connection, registrar: &str, window: u32) -> zbus::Result<Message> {
    call!(conn, registrar, "GetMenuForWindow", &(window,))
}

fn registered(msg: &Message) -> (u32, String, OwnedObjectPath) {
    msg.body().deserialize().unwrap()
}

#[test]
fn a_registration_is_signalled_answered_and_listed() {
    let bus = Bus::start();
    async_io::block_on(async {
        let (bar, app, watch) = (
            bus.connect().await,
            bus.connect().await,
            bus.connect().await,
        );
        let registrar = bar.unique_name().unwrap().to_string();
        let mut heard = signals(&watch, INTERFACE).await;
        let (_worker, _events) = Worker::start(bar).await.unwrap();
        let service = app.unique_name().unwrap().to_string();

        register(&app, &registrar, 7, "/menu/7").await;
        let msg = next_signal(&mut heard, "WindowRegistered").await;
        let path = OwnedObjectPath::try_from("/menu/7").unwrap();
        assert_eq!(registered(&msg), (7, service.clone(), path.clone()));

        let reply = menu_for(&watch, &registrar, 7).await.unwrap();
        let menu: (String, OwnedObjectPath) = reply.body().deserialize().unwrap();
        assert_eq!(menu, (service.clone(), path.clone()));
        assert_eq!(
            menus(&watch, &registrar).await,
            [(7, service.clone(), path)]
        );

        // registered again at the same place: nothing changed, nothing is told
        register(&app, &registrar, 7, "/menu/7").await;
        assert!(soon(next_signal(&mut heard, "WindowRegistered"))
            .await
            .is_none());

        call!(app, registrar, "UnregisterWindow", &(7u32,)).unwrap();
        let msg = next_signal(&mut heard, "WindowUnregistered").await;
        assert_eq!(msg.body().deserialize::<u32>().unwrap(), 7);
        assert!(menus(&watch, &registrar).await.is_empty());
    });
}

#[test]
fn an_unknown_window_is_an_invalid_argument() {
    let bus = Bus::start();
    async_io::block_on(async {
        let (bar, client) = (bus.connect().await, bus.connect().await);
        let registrar = bar.unique_name().unwrap().to_string();
        let (_worker, _events) = Worker::start(bar).await.unwrap();
        match menu_for(&client, &registrar, 8).await {
            Err(zbus::Error::MethodError(name, _, _)) => {
                assert_eq!(name.as_str(), "org.freedesktop.DBus.Error.InvalidArgs")
            }
            other => panic!("an unknown window answered {other:?}"),
        }
    });
}

#[test]
fn a_connection_that_leaves_takes_its_windows_and_the_next_stands() {
    let bus = Bus::start();
    async_io::block_on(async {
        let (bar, watch) = (bus.connect().await, bus.connect().await);
        let (first, second) = (bus.connect().await, bus.connect().await);
        let registrar = bar.unique_name().unwrap().to_string();
        let mut heard = signals(&watch, INTERFACE).await;
        let (_worker, _events) = Worker::start(bar).await.unwrap();
        let two = second.unique_name().unwrap().to_string();

        register(&first, &registrar, 5, "/first").await;
        register(&first, &registrar, 6, "/only").await;
        assert_eq!(
            registered(&next_signal(&mut heard, "WindowRegistered").await).0,
            5
        );
        assert_eq!(
            registered(&next_signal(&mut heard, "WindowRegistered").await).0,
            6
        );
        // a second connection's registration of window 5 does not stand while the first's does
        register(&second, &registrar, 5, "/second").await;
        assert!(soon(next_signal(&mut heard, "WindowRegistered"))
            .await
            .is_none());

        drop(first);
        let gone = next_signal(&mut heard, "WindowUnregistered").await;
        assert_eq!(gone.body().deserialize::<u32>().unwrap(), 6);
        let (window, service, path) =
            registered(&next_signal(&mut heard, "WindowRegistered").await);
        assert_eq!(
            (window, service.as_str(), path.as_str()),
            (5, two.as_str(), "/second")
        );
        let reply = menu_for(&watch, &registrar, 5).await.unwrap();
        let menu: (String, OwnedObjectPath) = reply.body().deserialize().unwrap();
        assert_eq!(menu.0, two);
        let windows: Vec<u32> = menus(&watch, &registrar)
            .await
            .iter()
            .map(|m| m.0)
            .collect();
        assert_eq!(windows, [5]);
    });
}
