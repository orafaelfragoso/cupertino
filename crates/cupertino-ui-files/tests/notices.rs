//! Notices posted on a peer connection to a stand-in notification service: their order, a click back, and the wait
//! for them to be sent.

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cupertino_ui_files::notice::{Click, Notice, Notices};
use futures_lite::future::{block_on, or};
use zbus::zvariant::OwnedValue;
use zbus::Connection;

const PATH: &str = "/org/freedesktop/Notifications";
const INTERFACE: &str = "org.freedesktop.Notifications";
/// How long the stand-in may take to hear a notice or a click to come back.
const HEARD: Duration = Duration::from_secs(5);

/// A notice the stand-in was asked to show: its summary and actions, and the id it answered.
type Shown = (String, Vec<String>, u32);

/// The notification service, as far as the notices use it.
struct Stand {
    shown: Arc<Mutex<Vec<Shown>>>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Stand {
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        _app: String,
        _replaces: u32,
        _icon: String,
        summary: String,
        _body: String,
        actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _timeout: i32,
    ) -> u32 {
        let mut shown = self.shown.lock().unwrap();
        let id = shown.len() as u32 + 1;
        shown.push((summary, actions, id));
        id
    }
}

/// Notices on a peer connection to a stand-in service, the posting future run on a thread of the test's; the
/// service's connection and what it was asked to show.
fn connected() -> (Notices, JoinHandle<()>, Connection, Arc<Mutex<Vec<Shown>>>) {
    let shown = Arc::new(Mutex::new(Vec::new()));
    let (ours, theirs) = UnixStream::pair().unwrap();
    let stand = Stand {
        shown: Arc::clone(&shown),
    };
    let (service, session) = block_on(async {
        let service = zbus::connection::Builder::async_io_unix_stream(theirs)
            .server(zbus::Guid::generate())
            .unwrap()
            .p2p()
            .serve_at(PATH, stand)
            .unwrap()
            .build();
        let session = zbus::connection::Builder::async_io_unix_stream(ours)
            .p2p()
            .build();
        futures_lite::future::zip(service, session).await
    });
    let (notices, posting) = Notices::on(session.unwrap());
    let posting = std::thread::spawn(move || block_on(posting));
    (notices, posting, service.unwrap(), shown)
}

fn notice(summary: &str) -> Notice {
    Notice {
        summary: summary.into(),
        body: String::new(),
        icon: "",
    }
}

/// Wait for `future`, at most [`HEARD`].
fn within<T>(future: impl std::future::Future<Output = T>) -> Option<T> {
    block_on(or(async { Some(future.await) }, async {
        async_io::Timer::after(HEARD).await;
        None
    }))
}

#[test]
fn notices_are_posted_in_order_and_sent_ends_once_they_are() {
    let (notices, posting, _service, shown) = connected();
    for summary in ["first", "second", "third"] {
        notices.post(notice(summary));
    }
    assert!(within(notices.sent()).is_some(), "sent never ended");
    let summaries: Vec<String> = shown.lock().unwrap().iter().map(|s| s.0.clone()).collect();
    assert_eq!(summaries, ["first", "second", "third"]);
    drop(notices);
    posting.join().unwrap();
    // notices that are logged have nothing to wait for
    assert!(within(Notices::logged().sent()).is_some());
}

#[test]
fn a_click_on_a_clickable_notice_comes_back_with_its_tag_and_token() {
    let (notices, _posting, service, shown) = connected();
    notices.post(notice("plain"));
    notices.post_clickable(notice("clickable"), 7);
    assert!(within(notices.sent()).is_some(), "sent never ended");
    let (actions, id) = {
        let shown = shown.lock().unwrap();
        assert!(shown[0].1.is_empty(), "a plain notice has no action");
        (shown[1].1.clone(), shown[1].2)
    };
    assert_eq!(actions, ["default", "Show"]);
    block_on(async {
        service
            .emit_signal(
                None::<&str>,
                PATH,
                INTERFACE,
                "ActivationToken",
                &(id, "token-7"),
            )
            .await
            .unwrap();
        service
            .emit_signal(
                None::<&str>,
                PATH,
                INTERFACE,
                "ActionInvoked",
                &(id, "default"),
            )
            .await
            .unwrap();
    });
    let clicks = notices.clicks().expect("clicks on a bus").clone();
    let click = within(clicks.recv()).expect("no click came back").unwrap();
    assert_eq!(
        click,
        Click {
            tag: 7,
            token: Some("token-7".into())
        }
    );
}
