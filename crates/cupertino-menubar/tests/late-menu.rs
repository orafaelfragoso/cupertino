//! A window's menu whose exporter answers a little after its address was announced is still loaded: the follower
//! looks the object up again instead of failing at once. Run on a private session bus.

// a test owns its thread: it blocks on the bus and looks up the daemon's binary
#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use async_io::Timer;
use cupertino_menubar::source::{KdeAppMenu, MenuSource};
use cupertino_menubar::worker::{Event, Request, Worker};
use futures_util::future::{select, Either};
use zbus::connection::Builder;
use zbus::zvariant::OwnedValue;

const PATH: &str = "/com/canonical/menu/2";
/// How long the exporter's object is missing after the address was announced: past the first wait of the follower.
const EXPORT_DELAY: Duration = Duration::from_millis(700);
/// The longest the menu may take to arrive once exported.
const ARRIVAL: Duration = Duration::from_secs(10);

struct Bus(Child);

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn session_bus() -> (Bus, String) {
    let daemon = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join("dbus-daemon"))
        .find(|p| p.is_file())
        .expect("dbus-daemon is installed");
    let mut process = Command::new(daemon)
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut address = String::new();
    BufReader::new(process.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    (Bus(process), address.trim().to_owned())
}

/// A menu of no items.
struct Exporter;

type Layout = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Exporter {
    fn get_layout(&self, _parent: i32, _depth: i32, _names: Vec<String>) -> (u32, Layout) {
        (1, (0, HashMap::new(), Vec::new()))
    }

    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn status(&self) -> String {
        "normal".into()
    }
}

#[test]
fn a_window_menu_exported_after_it_was_announced_is_loaded() {
    let (_bus, address) = session_bus();
    async_io::block_on(async {
        let bar = Builder::address(address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        let app = Builder::address(address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        // another window's menu, so the process answers calls to the object that is not there yet
        app.object_server()
            .at("/com/canonical/menu/1", Exporter)
            .await
            .unwrap();
        let service = app.unique_name().unwrap().to_string();
        let (worker, events) = Worker::start(bar).await.unwrap();
        let menu = KdeAppMenu {
            service,
            path: PATH.into(),
        };
        // the window's menu is announced: the object is not there yet
        worker.send(Request::FetchDbusMenu(menu.clone())).unwrap();
        Timer::after(EXPORT_DELAY).await;
        app.object_server().at(PATH, Exporter).await.unwrap();
        let outcome = async {
            loop {
                match events.recv().await.unwrap() {
                    Event::Layout { source, .. } => break Ok(source),
                    Event::LayoutFailed { source } => break Err(source),
                    _ => {}
                }
            }
        };
        let arrived = select(Box::pin(outcome), Timer::after(ARRIVAL)).await;
        match arrived {
            Either::Left((Ok(source), _)) => assert_eq!(source, MenuSource::Kde(menu)),
            Either::Left((Err(source), _)) => panic!("the menu was given up: {source:?}"),
            Either::Right(_) => panic!("no menu within {ARRIVAL:?}"),
        }
    });
}
