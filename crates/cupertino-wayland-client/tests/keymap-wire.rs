//! The keymap a compositor sends, on connections on threads of their own as a shell host's components have: each
//! translates keys through the keymap of its own text with a keyboard state of its own, connections sent the same text
//! share one compiled keymap, and one connection ending leaves the others typing.

use crate::support;

use cupertino_wayland_client::{Connection, Event, KeyboardEvent};
use std::sync::{Arc, Barrier};
use std::time::Duration;
use support::*;
use xkbcommon::xkb;

/// The evdev code of the key right of T: Y on a US keyboard, Z on a German one.
const KEY_Y: u32 = 21;

/// The keymap of `layout` as a compositor sends it: compiled from its names and written out as text.
fn keymap(layout: &str) -> String {
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    xkb::Keymap::new_from_names(
        &context,
        "evdev",
        "pc105",
        layout,
        "",
        None,
        xkb::COMPILE_NO_FLAGS,
    )
    .expect("the layout compiles")
    .get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1)
}

/// A connection with a window and a keyboard, sent the keymap `text`.
fn keyboard(text: &str) -> (Harness, Connection) {
    let (server, mut conn) = Harness::start(Offer {
        keyboard: true,
        ..Offer::NONE
    });
    let _window = window(&mut conn);
    let text = text.to_owned();
    server.run(move |server| server.keymap(&text));
    (server, conn)
}

/// The character the key `code` types on `conn`, pressed through `server`.
fn types(server: &Harness, conn: &mut Connection, code: u32) -> Option<char> {
    server.run(move |server| server.type_key(code));
    for _ in 0..100 {
        for event in conn.wait(Some(Duration::from_millis(50))).unwrap() {
            if let Event::Keyboard {
                event:
                    KeyboardEvent::Key {
                        code: typed,
                        pressed: true,
                        character,
                        ..
                    },
                ..
            } = event
            {
                assert_eq!(typed, code);
                return character;
            }
        }
    }
    panic!("the key did not arrive within 5 s");
}

#[test]
fn connections_type_through_the_keymap_of_their_own_text_and_one_ending_leaves_the_others_typing() {
    let (us, german) = (keymap("us"), keymap("de"));
    let together = Arc::new(Barrier::new(3));
    let ended = Arc::new(Barrier::new(2));
    std::thread::scope(|threads| {
        let first = threads.spawn(|| {
            let (server, mut conn) = keyboard(&us);
            together.wait();
            assert_eq!(types(&server, &mut conn, KEY_Y), Some('y'));
            drop((conn, server));
            ended.wait();
        });
        let second = threads.spawn(|| {
            let (server, mut conn) = keyboard(&us);
            together.wait();
            assert_eq!(types(&server, &mut conn, KEY_Y), Some('y'));
            ended.wait();
            assert_eq!(
                types(&server, &mut conn, KEY_Y),
                Some('y'),
                "typing after the first connection ended"
            );
        });
        let third = threads.spawn(|| {
            let (server, mut conn) = keyboard(&german);
            together.wait();
            assert_eq!(types(&server, &mut conn, KEY_Y), Some('z'));
        });
        for thread in [first, second, third] {
            thread.join().unwrap();
        }
    });
    // every connection on the US keymap ended: a new one compiles it again
    let (server, mut conn) = keyboard(&us);
    assert_eq!(types(&server, &mut conn, KEY_Y), Some('y'));
}
