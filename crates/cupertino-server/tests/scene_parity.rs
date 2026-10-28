//! The migrated scene composition against the builder it replaces, on the same committed state: the same lowered
//! element variants in the same order, with the same kind, geometry, source rectangle, opacity and commit
//! progression, and a consistent correspondence between element identities. Pixels are the goldens' business; this
//! catches what a settled headless frame cannot, such as a live surface becoming a texture.
#![cfg(feature = "scene-oracle")]

mod common;

use std::collections::HashMap;

use common::desktop::Desktop;
use cupertino_server::backend::Backend;
use cupertino_server::render::CupertinoElement;
use smithay::backend::input::KeyState;
use smithay::backend::renderer::element::{Element, Id, Kind};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::utils::{Physical, Rectangle, Scale};

#[derive(Debug, PartialEq)]
struct Shape {
    variant: std::mem::Discriminant<CupertinoElement>,
    kind: String,
    geometry: Rectangle<i32, Physical>,
    src: (f64, f64, f64, f64),
    alpha: f32,
    commit: usize,
}

fn kind_name(kind: Kind) -> String {
    format!("{kind:?}")
}

fn shape(element: &CupertinoElement, scale: f64) -> Shape {
    let src = element.src();
    Shape {
        variant: std::mem::discriminant(element),
        kind: kind_name(element.kind()),
        geometry: element.geometry(Scale::from(scale)),
        src: (src.loc.x, src.loc.y, src.size.w, src.size.h),
        alpha: element.alpha(),
        commit: element
            .current_commit()
            .distance(Some(CommitCounter::default()))
            .unwrap_or(0),
    }
}

/// Which identity of the migrated list stands for which identity of the oracle, over every frame compared.
#[derive(Default)]
struct Identities {
    forward: HashMap<Id, Id>,
    backward: HashMap<Id, Id>,
}

impl Identities {
    fn pair(&mut self, legacy: &Id, migrated: &Id, at: &str) {
        let known = self
            .forward
            .entry(legacy.clone())
            .or_insert_with(|| migrated.clone());
        assert_eq!(known, migrated, "{at}: one oracle identity maps to two");
        let known = self
            .backward
            .entry(migrated.clone())
            .or_insert_with(|| legacy.clone());
        assert_eq!(known, legacy, "{at}: two oracle identities map to one");
    }
}

struct Parity {
    desktop: Desktop,
    identities: Identities,
}

impl std::ops::Deref for Parity {
    type Target = Desktop;
    fn deref(&self) -> &Desktop {
        &self.desktop
    }
}

impl std::ops::DerefMut for Parity {
    fn deref_mut(&mut self) -> &mut Desktop {
        &mut self.desktop
    }
}

impl Parity {
    fn new(scale: f64) -> Self {
        cupertino_server::render::legacy_scene::reset();
        Self {
            desktop: Desktop::new(scale),
            identities: Identities::default(),
        }
    }

    /// Build the frame; with the oracle feature the compositor also runs the old builder on every frame, so both
    /// element caches have seen the same sequence.
    fn compare(&mut self, what: &str) -> usize {
        let mut backend = self.state.backend.take().unwrap();
        let Backend::Headless(ref mut h) = backend else {
            panic!("headless backend required")
        };
        let migrated = self.state.build_output_elements(&mut h.renderer, 0);
        let legacy = cupertino_server::render::legacy_scene::take_recorded();
        self.state.backend = Some(backend);
        assert_eq!(legacy.len(), migrated.len(), "{what}: element count");
        for (at, (old, new)) in legacy.iter().zip(&migrated).enumerate() {
            let place = format!("{what}: element {at}");
            assert_eq!(shape(old, self.scale), shape(new, self.scale), "{place}");
            self.identities.pair(old.id(), new.id(), &place);
        }
        legacy.len()
    }
}

#[test]
fn an_empty_desktop_matches() {
    for scale in [1.0, 1.5] {
        let mut d = Parity::new(scale);
        d.at(1000);
        assert!(d.compare("empty desktop") > 0);
        d.at(1016);
        d.compare("empty desktop, next frame");
    }
}

#[test]
fn windows_at_rest_match_at_integer_and_fractional_scale() {
    for scale in [1.0, 1.25, 2.0] {
        let mut d = Parity::new(scale);
        d.window();
        d.window();
        d.at(1000);
        d.compare("two windows opened");
        d.at(3000);
        assert!(d.compare("two windows at rest") > 4);
        d.at(3016);
        d.compare("two windows at rest, next frame");
    }
}

#[test]
fn a_window_matches_while_it_opens() {
    let mut d = Parity::new(1.0);
    d.window();
    for ms in [0u32, 40, 80, 160, 320, 640] {
        d.at(ms);
        d.compare(&format!("opening at {ms} ms"));
    }
}

#[test]
fn a_locked_session_matches() {
    let mut d = Parity::new(1.0);
    d.window();
    d.at(2000);
    d.compare("before the lock");
    let _locker = d.lock();
    d.at(2100);
    d.compare("locked");
    d.at(2200);
    d.compare("locked, next frame");
}

#[test]
fn the_overview_matches_while_it_opens_and_at_rest() {
    let mut d = Parity::new(1.0);
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
        d.compare(&format!("overview at {ms} ms"));
    }
}

#[test]
fn the_pointer_matches_wherever_it_is() {
    for scale in [1.0, 1.5] {
        let mut d = Parity::new(scale);
        d.window();
        d.at(2000);
        for (x, y) in [(10.0, 10.0), (500.0, 350.0), (999.0, 699.0)] {
            d.state.pointer_moved_to((x, y).into(), 2000, None);
            d.at(2016);
            d.compare(&format!("pointer at {x},{y}"));
        }
    }
}

#[test]
fn the_input_source_hud_matches_while_it_shows_and_fades() {
    for scale in [1.0, 1.5] {
        let mut d = Parity::new(scale);
        d.window();
        d.at(2000);
        d.state.overlays.inputs.hud = Some(cupertino_server::kbd::HudView {
            order: vec!["us".into(), "de".into()],
            labels: vec!["ABC".into(), "Deutsch".into()],
            selection: 1,
            fading_since: None,
        });
        d.at(2016);
        assert!(d.compare("hud shown") > 6);
        if let Some(hud) = d.state.overlays.inputs.hud.as_mut() {
            hud.fading_since = Some(2.016);
        }
        for ms in [2100u32, 2200] {
            d.at(ms);
            d.compare(&format!("hud fading at {ms} ms"));
        }
    }
}

#[test]
fn the_activation_fade_matches_over_the_desktop_and_over_a_lock() {
    let mut d = Parity::new(1.0);
    d.window();
    d.at(2000);
    d.state.session_activated();
    for ms in [2000u32, 2100, 2300] {
        d.at(ms);
        d.compare(&format!("fade over the desktop at {ms} ms"));
    }
    let _locker = d.lock();
    d.state.session_activated();
    for ms in [3000u32, 3100, 3300] {
        d.at(ms);
        d.compare(&format!("fade over the lock at {ms} ms"));
    }
}
