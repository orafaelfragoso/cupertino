//! Semantic publication against an in-process server: interest, sealed snapshots, routed actions and replies.

mod support;

use cupertino_app_protocols::semantics::{
    seal, Action, ActionCall, Bounds, Node, Outcome, Position, Snapshot, States, MAX_FRAME,
};
use cupertino_wayland_client::app::{SemanticSurface, SemanticWake, Semantics};
use cupertino_wayland_client::{Connection, Event};
use std::os::fd::AsFd;
use std::time::Duration;
use support::*;

struct Setup {
    server: Harness,
    conn: Connection,
    surface: SemanticSurface,
    _semantics: Semantics,
}

fn setup() -> Setup {
    let (server, mut conn) = Harness::start(Offer {
        semantics: Some(2),
        ..Offer::NONE
    });
    let window = window(&mut conn);
    let semantics = Semantics::bind(&mut conn).expect("the server offers semantics");
    let surface = semantics.surface(&conn, window.surface()).unwrap();
    // the server has the surface before a test sends it anything
    conn.roundtrip().unwrap();
    Setup {
        server,
        conn,
        surface,
        _semantics: semantics,
    }
}

impl Setup {
    /// Handle the connection until the application is woken, which the server's events do.
    fn woken(&mut self) {
        for _ in 0..100 {
            let woken = self
                .conn
                .wait(Some(Duration::from_millis(50)))
                .unwrap()
                .iter()
                .any(|e| matches!(e, Event::Extension(x) if x.downcast_ref::<SemanticWake>().is_some()));
            if woken {
                return;
            }
        }
        panic!("the application was not woken within 5 s");
    }

    /// Make the server tell the surface that `epoch` is (not) of interest.
    fn interest(&mut self, epoch: u64, active: bool) {
        self.server.run(move |server| {
            server.semantic_surfaces[0].interest(
                (epoch >> 32) as u32,
                epoch as u32,
                u32::from(active),
            );
        });
        self.woken();
        assert_eq!(self.surface.active(), active);
    }

    /// Everything the server handled, after the client's latest requests.
    fn sent(&mut self) -> Vec<String> {
        self.conn.roundtrip().unwrap();
        self.server
            .requests()
            .into_iter()
            .filter(|r| !r.starts_with("get_"))
            .collect()
    }

    fn perform(&mut self, call: &ActionCall, grab_serial: Option<u32>) {
        let call = call.clone();
        self.server.run(move |server| {
            let surface = &server.semantic_surfaces[0];
            if let Some(serial) = grab_serial {
                surface.grab_serial(serial);
            }
            let (file, len) = seal(&call, MAX_FRAME).unwrap();
            surface.perform(file.as_fd(), len);
        });
    }
}

fn node(id: u64, children: &[u64]) -> Node {
    Node {
        truncated: false,
        position: Position::default(),
        id,
        role: "group".into(),
        name: None,
        description: None,
        value: None,
        states: States::default(),
        bounds: Bounds {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        },
        actions: vec![],
        children: children.to_vec(),
    }
}

fn snapshot(revision: u64) -> Snapshot {
    Snapshot {
        revision,
        root: 1,
        focus: None,
        nodes: vec![node(1, &[])],
    }
}

fn call(id: u64, epoch: u64) -> ActionCall {
    ActionCall {
        id,
        epoch,
        revision: 1,
        node: 1,
        action: Action::Click,
        data: None,
    }
}

const EPOCH: u64 = (7 << 32) | 9;

fn wait_for_requests(s: &mut Setup) -> Vec<cupertino_wayland_client::app::SemanticRequest> {
    for _ in 0..100 {
        let requests = s.surface.requests();
        if !requests.is_empty() {
            return requests;
        }
        s.conn.wait(Some(Duration::from_millis(50))).unwrap();
    }
    panic!("no action arrived within 5 s");
}

/// Lets the connection take what the server sent, without the application looking.
fn settle(s: &mut Setup) {
    for _ in 0..10 {
        s.conn.wait(Some(Duration::from_millis(20))).unwrap();
    }
}

#[test]
fn a_server_without_the_global_gives_none() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    assert!(Semantics::bind(&mut conn).is_none());
}

#[test]
fn a_snapshot_is_published_sealed_only_while_the_server_is_interested_once_per_revision_and_epoch()
{
    let mut s = setup();
    assert!(!s.surface.active());
    s.surface.publish(&snapshot(1));
    assert!(s.sent().is_empty(), "nothing before interest");
    assert!(s.server.publications.lock().unwrap().is_empty());

    s.interest(EPOCH, true);
    s.surface.publish(&snapshot(1));
    s.surface.publish(&snapshot(1));
    s.surface.publish(&snapshot(2));
    s.surface.publish(&snapshot(2));
    s.sent();
    s.server.wait_for("publish", 2);
    {
        let published = s.server.publications.lock().unwrap();
        let got: Vec<_> = published
            .iter()
            .map(|(epoch, p)| (*epoch, p.as_ref().unwrap().clone()))
            .collect();
        assert_eq!(got, [(EPOCH, snapshot(1)), (EPOCH, snapshot(2))]);
    }

    // a new epoch asks for the current revision again
    s.interest(EPOCH + 1, true);
    s.surface.publish(&snapshot(2));
    s.sent();
    let epochs: Vec<_> = s
        .server
        .publications
        .lock()
        .unwrap()
        .iter()
        .map(|(e, _)| *e)
        .collect();
    assert_eq!(epochs, [EPOCH, EPOCH, EPOCH + 1]);

    let before = s.sent().len();
    s.interest(EPOCH + 1, false);
    s.surface.publish(&snapshot(3));
    assert_eq!(s.sent().len(), before, "losing interest stops publication");
}

#[test]
fn a_snapshot_that_cannot_be_published_is_reported_unavailable() {
    let mut broken = snapshot(1);
    broken.root = 99;
    assert!(broken.validate().is_err());
    let mut big = snapshot(1);
    big.nodes[0].name = Some("x".repeat(MAX_FRAME + 1));
    for (row, snapshot) in [("invalid", broken), ("over the frame limit", big)] {
        let mut s = setup();
        s.interest(EPOCH, true);
        s.surface.publish(&snapshot);
        assert_eq!(s.sent(), [format!("unavailable {EPOCH}")], "{row}");
        assert!(s.server.publications.lock().unwrap().is_empty(), "{row}");
    }
}

#[test]
fn an_action_wakes_the_application_which_takes_it() {
    let mut s = setup();
    s.interest(EPOCH, true);
    s.perform(&call(1, EPOCH), None);
    s.woken();
    let requests = wait_for_requests(&mut s);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].call, call(1, EPOCH));
    assert_eq!(requests[0].grab_serial, None);
    assert!(s.surface.requests().is_empty(), "taken once");
}

#[test]
fn an_action_of_another_epoch_or_that_is_not_a_call_is_dropped() {
    type Send = fn(&mut Setup);
    let rows: [(&str, Send); 2] = [
        ("another epoch", |s| s.perform(&call(1, EPOCH + 1), None)),
        ("not a call", |s| {
            s.server.run(|server| {
                let (file, len) = seal(&snapshot(1), MAX_FRAME).unwrap();
                server.semantic_surfaces[0].perform(file.as_fd(), len);
            })
        }),
    ];
    for (row, send) in rows {
        let mut s = setup();
        s.interest(EPOCH, true);
        send(&mut s);
        s.perform(&call(2, EPOCH), None);
        let ids: Vec<_> = wait_for_requests(&mut s)
            .iter()
            .map(|r| r.call.id)
            .collect();
        assert_eq!(ids, [2], "{row}");
    }
}

#[test]
fn one_action_waits_at_a_time_and_a_change_of_interest_discards_it() {
    let mut s = setup();
    s.interest(EPOCH, true);
    s.perform(&call(1, EPOCH), None);
    s.perform(&call(2, EPOCH), None);
    settle(&mut s);
    let ids: Vec<_> = s.surface.requests().iter().map(|r| r.call.id).collect();
    assert_eq!(ids, [1], "the second is dropped while the first waits");
    s.perform(&call(3, EPOCH), None);
    assert_eq!(
        wait_for_requests(&mut s)[0].call.id,
        3,
        "once taken, the next is accepted"
    );

    s.perform(&call(4, EPOCH), None);
    settle(&mut s);
    s.interest(EPOCH, false);
    assert!(s.surface.requests().is_empty());
}

#[test]
fn a_grab_serial_belongs_to_the_action_right_after_it_even_a_dropped_one() {
    let mut s = setup();
    s.interest(EPOCH, true);
    s.perform(&call(1, EPOCH), Some(41));
    assert_eq!(wait_for_requests(&mut s)[0].grab_serial, Some(41));
    s.perform(&call(2, EPOCH), None);
    assert_eq!(wait_for_requests(&mut s)[0].grab_serial, None);

    s.perform(&call(3, EPOCH + 5), Some(42));
    s.perform(&call(4, EPOCH), None);
    let requests = wait_for_requests(&mut s);
    assert_eq!(requests[0].call.id, 4);
    assert_eq!(
        requests[0].grab_serial, None,
        "the dropped action spent the serial"
    );
}

#[test]
fn a_completion_is_sent_for_the_current_epoch_only() {
    let mut s = setup();
    s.surface.complete(&call(1, 0), Outcome::Denied, 1);
    assert!(s.sent().is_empty(), "nothing is completed while inactive");

    s.interest(EPOCH, true);
    s.surface.complete(&call(1, EPOCH), Outcome::Applied, 4);
    s.surface.complete(&call(2, EPOCH + 1), Outcome::Applied, 4);
    s.sent();
    s.server.wait_for("complete", 1);
    let replies = s.server.replies.lock().unwrap().clone();
    assert_eq!(replies.len(), 1);
    assert!(
        replies[0].contains("id: 1")
            && replies[0].contains("revision: 4")
            && replies[0].contains("Applied"),
        "{}",
        replies[0]
    );
}

#[test]
fn dropping_the_surface_destroys_it() {
    let Setup {
        server,
        mut conn,
        surface,
        ..
    } = setup();
    drop(surface);
    conn.roundtrip().unwrap();
    server.wait_for("Destroy", 1);
}
