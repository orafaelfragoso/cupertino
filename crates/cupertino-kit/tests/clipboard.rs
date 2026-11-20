//! `Command::SetClipboardData`: the data goes on the clipboard in every MIME type it is given, and the application
//! hears the message it named once another application's selection replaces it, never after it put something else on
//! the clipboard itself.

#[allow(dead_code)]
mod support;

use cupertino_kit::core::{Application, Command, Window};
use cupertino_kit::platform::ClipboardData;
use cupertino_ui_core::platform::ClipboardKind;
use std::num::NonZeroU32;

/// The first bytes of a PNG file, standing in for a picture.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

fn picture() -> Vec<ClipboardData> {
    vec![
        ClipboardData {
            mime: "image/png".into(),
            bytes: PNG.to_vec(),
        },
        ClipboardData {
            mime: "text/plain;charset=utf-8".into(),
            bytes: b"Screenshot".to_vec(),
        },
    ]
}

/// Puts a picture on the clipboard and counts how often another application's copy replaced it; after the first, it
/// puts the picture there again and then text of its own in its place.
struct Copier;

#[derive(Clone, Debug)]
struct Replaced;

impl Application for Copier {
    type Model = u32;
    type Message = Replaced;
    const RESIDENT: bool = true;

    fn init() -> (u32, Vec<Command<Replaced>>) {
        let copy = Command::SetClipboardData {
            data: picture(),
            replaced: Some(Replaced),
        };
        (0, vec![copy])
    }

    fn update(replaced: &mut u32, Replaced: Replaced) -> Vec<Command<Replaced>> {
        *replaced += 1;
        vec![
            Command::SetClipboardData {
                data: picture(),
                replaced: Some(Replaced),
            },
            Command::SetClipboard("mine".into()),
        ]
    }

    fn view(_: &u32) -> Vec<Window<Replaced>> {
        Vec::new()
    }
}

fn idle(driver: &mut cupertino_kit::test::TestDriver<Copier>) {
    driver
        .run_until_idle(NonZeroU32::new(1000).unwrap())
        .unwrap();
}

#[test]
fn data_goes_on_the_clipboard_in_each_type_and_its_replacement_is_heard_once() {
    let mut driver = support::driver::<Copier>(support::text_system());
    idle(&mut driver);
    let clipboard = ClipboardKind::Clipboard;
    assert_eq!(driver.window_system_mut().clipboard(clipboard), picture());
    assert_eq!(*driver.runtime().model(), 0);

    let other = ClipboardData {
        mime: "text/plain".into(),
        bytes: b"another application's".to_vec(),
    };
    driver
        .window_system_mut()
        .offer_clipboard(clipboard, Some(other.clone()));
    support::run_until(&mut driver, |replaced| *replaced == 1);

    idle(&mut driver);
    let own = driver.window_system_mut().clipboard(clipboard).to_vec();
    assert_eq!(own.len(), 1, "the text the application put there last");
    assert_eq!(own[0].bytes, b"mine");
    driver
        .window_system_mut()
        .offer_clipboard(clipboard, Some(other));
    idle(&mut driver);
    assert_eq!(
        *driver.runtime().model(),
        1,
        "data the application replaced itself is not heard of again"
    );
}
