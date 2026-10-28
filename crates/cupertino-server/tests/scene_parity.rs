//! The migrated scene composition against the builder it replaces, on the same committed state: the same lowered
//! element variants in the same order, with the same kind, geometry, source rectangle, opacity and commit
//! progression, and a consistent correspondence between element identities. Pixels are the goldens' business; this
//! catches what a settled headless frame cannot, such as a live surface becoming a texture.
#![cfg(feature = "scene-oracle")]

mod common;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cupertino_server::backend::{headless, Backend};
use cupertino_server::config::Config;
use cupertino_server::render::CupertinoElement;
use cupertino_server::Cupertino;
use smithay::backend::renderer::element::{Element, Id, Kind};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::{Physical, Rectangle, Scale};

const SIZE: (i32, i32) = (1000, 700);

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

struct Desktop {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
    clients: Vec<common::Client>,
    scale: f64,
    identities: Identities,
}

impl Desktop {
    fn new(scale: f64) -> Self {
        cupertino_server::render::legacy_scene::reset();
        let event_loop = EventLoop::try_new().unwrap();
        let mut state = Cupertino::new(
            Display::new().unwrap(),
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        state.fixed_time = Some(0.0);
        headless::init(&mut state, SIZE, scale)
            .expect("the parity tests require an offscreen EGL renderer");
        Self {
            state,
            event_loop,
            clients: Vec::new(),
            scale,
            identities: Identities::default(),
        }
    }

    fn pump(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(2),
        )
        .unwrap();
    }

    fn window(&mut self) {
        let expected = self.clients.len() + 1;
        let socket = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap())
            .join(self.state.socket_name.as_ref().unwrap());
        self.clients.push(common::Client::toplevel(socket));
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.state.wm.windows().count() != expected {
            self.pump();
            assert!(Instant::now() < deadline, "the test client did not map");
        }
    }

    fn at(&mut self, milliseconds: u32) {
        self.state.fixed_time = Some(f64::from(milliseconds) / 1000.0);
        self.state.tick();
        self.pump();
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
        let mut d = Desktop::new(scale);
        d.at(1000);
        assert!(d.compare("empty desktop") > 0);
        d.at(1016);
        d.compare("empty desktop, next frame");
    }
}

#[test]
fn windows_at_rest_match_at_integer_and_fractional_scale() {
    for scale in [1.0, 1.25, 2.0] {
        let mut d = Desktop::new(scale);
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
    let mut d = Desktop::new(1.0);
    d.window();
    for ms in [0u32, 40, 80, 160, 320, 640] {
        d.at(ms);
        d.compare(&format!("opening at {ms} ms"));
    }
}
