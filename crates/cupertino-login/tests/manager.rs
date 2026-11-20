//! The login screen's connection to the session manager (`cupertino_login::manager`) against a socket of the test's
//! own: it greets the manager first, hands over the events in the order they came, writes the requests in the order they
//! were sent, and says when the manager went away.
#![cfg(feature = "app")]

use cupertino_login::manager::{Connection, Error};
use cupertino_login_ipc::message::{MAX_EVENT_FRAME, MAX_REQUEST_FRAME};
use cupertino_login_ipc::{decode, encode, Decoder, Event, Request, VERSION};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Mutex;

/// The next request the screen wrote on `stream`.
fn request(stream: &mut UnixStream, decoder: &mut Decoder) -> Request {
    let mut buffer = [0u8; 256];
    loop {
        let wanted = decoder.wanted().min(buffer.len());
        let count = stream.read(&mut buffer[..wanted]).unwrap();
        assert_ne!(count, 0, "the screen keeps the connection");
        if let Some(body) = decoder.push(&buffer[..count]).unwrap() {
            return decode(&body).unwrap();
        }
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "the test waits for the connection on its own thread"
)]
fn wait<T>(future: impl std::future::Future<Output = T>) -> T {
    async_io::block_on(future)
}

#[test]
fn the_screen_greets_relays_and_hears_the_manager_go() {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("login-manager-{}.sock", std::process::id()));
    let listener = UnixListener::bind(&path).unwrap();
    let (connection, pump) = wait(Connection::connect(&path)).expect("the socket accepts");
    connection.send(&Request::Cancel {}).unwrap();

    #[expect(
        clippy::disallowed_methods,
        reason = "the manager's side of the test runs beside the screen's"
    )]
    let manager = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut decoder = Decoder::new(MAX_REQUEST_FRAME);
        let first = request(&mut stream, &mut decoder);
        let second = request(&mut stream, &mut decoder);
        for event in [
            Event::Hello { version: VERSION },
            Event::Notice {
                error: false,
                text: "hello".into(),
            },
        ] {
            stream
                .write_all(&encode(&event, MAX_EVENT_FRAME).unwrap())
                .unwrap();
        }
        // the manager goes away
        (first, second)
    });

    let heard = Mutex::new(Vec::new());
    let ended = wait(pump.run(|event| heard.lock().unwrap().push(format!("{event:?}"))));
    let (first, second) = manager.join().unwrap();
    assert!(
        matches!(first, Request::Hello { version } if version == VERSION),
        "{first:?}"
    );
    assert!(matches!(second, Request::Cancel {}), "{second:?}");
    let heard = heard.into_inner().unwrap();
    assert_eq!(heard.len(), 2, "{heard:?}");
    assert!(heard[0].starts_with("Hello"), "{heard:?}");
    assert!(heard[1].contains("hello"), "{heard:?}");
    assert!(matches!(ended, Error::Closed), "{ended}");
    assert!(
        connection.send(&Request::Cancel {}).is_err(),
        "nothing is sent once the pump ended"
    );
}
