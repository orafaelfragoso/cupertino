//! The migrated scene composition against the builder it replaces, on the same committed state: the same lowered
//! element variants in the same order, with the same kind, geometry, source rectangle, opacity and commit
//! progression, and a consistent correspondence between element identities. Pixels are the goldens' business; this
//! catches what a settled headless frame cannot, such as a live surface becoming a texture. The comparison runs
//! inside the compositor (`legacy_scene::assert_parity`), so every other test under the feature checks it too.
#![cfg(feature = "scene-oracle")]

mod common;

use common::desktop::Desktop;
use cupertino_server::backend::Backend;
use smithay::backend::input::KeyState;

/// Every frame is compared inside the compositor while the `scene-oracle` feature is on, so these scenarios only
/// have to produce the frames; `compare` builds one and returns how many elements it has.
fn compare(d: &mut Desktop, _what: &str) -> usize {
    let mut backend = d.state.backend.take().unwrap();
    let Backend::Headless(ref mut h) = backend else {
        panic!("headless backend required")
    };
    let elements = d.state.build_output_elements(&mut h.renderer, 0);
    d.state.backend = Some(backend);
    elements.len()
}

#[test]
fn an_empty_desktop_matches() {
    for scale in [1.0, 1.5] {
        let mut d = Desktop::new(scale);
        d.at(1000);
        assert!(compare(&mut d, "empty desktop") > 0);
        d.at(1016);
        compare(&mut d, "empty desktop, next frame");
    }
}

#[test]
fn windows_at_rest_match_at_integer_and_fractional_scale() {
    for scale in [1.0, 1.25, 2.0] {
        let mut d = Desktop::new(scale);
        d.window();
        d.window();
        d.at(1000);
        compare(&mut d, "two windows opened");
        d.at(3000);
        assert!(compare(&mut d, "two windows at rest") > 4);
        d.at(3016);
        compare(&mut d, "two windows at rest, next frame");
    }
}

#[test]
fn a_window_matches_while_it_opens() {
    let mut d = Desktop::new(1.0);
    d.window();
    for ms in [0u32, 40, 80, 160, 320, 640] {
        d.at(ms);
        compare(&mut d, &format!("opening at {ms} ms"));
    }
}

#[test]
fn a_locked_session_matches() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.at(2000);
    compare(&mut d, "before the lock");
    let _locker = d.lock();
    d.at(2100);
    compare(&mut d, "locked");
    d.at(2200);
    compare(&mut d, "locked, next frame");
}

#[test]
fn the_overview_matches_while_it_opens_and_at_rest() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.window();
    d.at(2000);
    d.state.pointer_moved_to((500.0, 350.0).into(), 2000, None);
    // XKB keycodes are evdev + 8: Ctrl+Up enters the overview through the key router.
    for (code, pressed) in [(37u32, true), (111, true), (111, false), (37, false)] {
        d.state.keyboard_event_now(
            code.into(),
            if pressed {
                KeyState::Pressed
            } else {
                KeyState::Released
            },
            2100,
        );
    }
    assert!(
        d.state.shell.overview_open(),
        "the key sequence opens the overview"
    );
    for ms in [2100u32, 2160, 2260, 2460, 3200] {
        d.at(ms);
        compare(&mut d, &format!("overview at {ms} ms"));
    }
}

#[test]
fn the_pointer_matches_wherever_it_is() {
    for scale in [1.0, 1.5] {
        let mut d = Desktop::new(scale);
        d.window();
        d.at(2000);
        for (x, y) in [(10.0, 10.0), (500.0, 350.0), (999.0, 699.0)] {
            d.state.pointer_moved_to((x, y).into(), 2000, None);
            d.at(2016);
            compare(&mut d, &format!("pointer at {x},{y}"));
        }
    }
}

#[test]
fn the_input_source_hud_matches_while_it_shows_and_fades() {
    for scale in [1.0, 1.5] {
        let mut d = Desktop::new(scale);
        d.window();
        d.at(2000);
        d.state.overlays.inputs.hud = Some(cupertino_server::kbd::HudView {
            order: vec!["us".into(), "de".into()],
            labels: vec!["ABC".into(), "Deutsch".into()],
            selection: 1,
            fading_since: None,
        });
        d.at(2016);
        assert!(compare(&mut d, "hud shown") > 6);
        if let Some(hud) = d.state.overlays.inputs.hud.as_mut() {
            hud.fading_since = Some(2.016);
        }
        for ms in [2100u32, 2200] {
            d.at(ms);
            compare(&mut d, &format!("hud fading at {ms} ms"));
        }
    }
}

#[test]
fn the_activation_fade_matches_over_the_desktop_and_over_a_lock() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.at(2000);
    d.state.session_activated();
    for ms in [2000u32, 2100, 2300] {
        d.at(ms);
        compare(&mut d, &format!("fade over the desktop at {ms} ms"));
    }
    let _locker = d.lock();
    d.state.session_activated();
    for ms in [3000u32, 3100, 3300] {
        d.at(ms);
        compare(&mut d, &format!("fade over the lock at {ms} ms"));
    }
}
