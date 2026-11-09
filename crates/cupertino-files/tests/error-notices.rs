//! Failures as notifications against a fake notification service on a private bus: what one notice sends, and that
//! several go in order.
#![cfg(feature = "app")]

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use cupertino_files::notice::{self, Action, Notices};
use cupertino_files::operations::DropAction;
use cupertino_files::rename::NameError;
use cupertino_test_bus::{missing_tool, PrivateBus};
use zbus::blocking::Connection;
use zbus::zvariant::OwnedValue;

struct Notifications(Mutex<mpsc::Sender<String>>);

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let transient = hints
            .get("transient")
            .and_then(|v| bool::try_from(&**v).ok());
        let entry = hints
            .get("desktop-entry")
            .and_then(|v| String::try_from(v.try_clone().ok()?).ok());
        let _ = self.0.lock().unwrap().send(format!(
            "{app_name} {replaces_id} {app_icon:?} {summary} / {body} {actions:?} \
             transient {transient:?} entry {entry:?} expires {expire_timeout}"
        ));
        1
    }
}

fn serve(bus: &PrivateBus) -> (Connection, mpsc::Receiver<String>) {
    let (sent, received) = mpsc::channel();
    let service = bus
        .blocking_builder()
        .name("org.freedesktop.Notifications")
        .unwrap()
        .serve_at(
            "/org/freedesktop/Notifications",
            Notifications(Mutex::new(sent)),
        )
        .unwrap()
        .build()
        .unwrap();
    (service, received)
}

fn next(received: &mpsc::Receiver<String>) -> String {
    received
        .recv_timeout(Duration::from_secs(5))
        .expect("a notification within 5 s")
}

#[test]
fn a_failure_is_one_transient_notification_from_files_with_its_text_escaped() {
    let Some(bus) = PrivateBus::start() else {
        missing_tool("dbus-daemon");
        return;
    };
    let (_service, received) = serve(&bus);
    let notices = Notices::start(Some(bus.connect()));

    notices.post(notice::failure(
        Action::Open,
        &[Path::new("/home/ada/R&D <b>.odt")],
        &"<a href=\"file:///etc\">no</a> application",
    ));
    assert_eq!(
        next(&received),
        "Files 0 \"\" “R&D <b>.odt” couldn't be opened / \
         &lt;a href=\"file:///etc\"&gt;no&lt;/a&gt; application. [] \
         transient Some(true) entry Some(\"cupertino-files\") expires -1"
    );
    notices.flush(Duration::from_secs(5));
    assert!(received.try_recv().is_err(), "one notification");
}

#[test]
fn notices_arrive_in_the_order_they_were_posted() {
    let Some(bus) = PrivateBus::start() else {
        missing_tool("dbus-daemon");
        return;
    };
    let (_service, received) = serve(&bus);
    let notices = Notices::start(Some(bus.connect()));
    let denied = io::Error::from_raw_os_error(13);

    notices.post(notice::failure(Action::Paste, &[], &"no text offered"));
    notices.post(notice::failure(
        Action::Rename,
        &[Path::new("/a/Notes.txt")],
        &denied,
    ));
    notices.post(notice::failure(
        Action::PutBack,
        &[Path::new("/t/x"), Path::new("/t/y")],
        &"The Trash has no record of where it came from",
    ));
    notices.post(notice::failure(
        Action::Drop(DropAction::Copy),
        &[Path::new("/a/b")],
        &denied,
    ));
    notices.post(notice::bad_name("a/b", &NameError::ContainsSlash));
    let summaries: Vec<String> = (0..5)
        .map(|_| {
            let line = next(&received);
            let summary = line.split(" / ").next().unwrap();
            summary.trim_start_matches("Files 0 \"\" ").to_string()
        })
        .collect();
    assert_eq!(
        summaries,
        [
            "The text couldn't be pasted",
            "“Notes.txt” couldn't be renamed",
            "2 items couldn't be put back",
            "“b” couldn't be copied",
            "“a/b” can't be used as a name",
        ]
    );
}
