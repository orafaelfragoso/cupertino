//! Two Kit instances in one process, each on a UI thread of its own on the headless window system and drawing with the
//! text system and icons every instance of the process shares (`app::environment`): each draws its window and takes
//! its pointer input apart from the other, and when one panics the panic is that instance's and the other goes on
//! taking input and drawing with the same caches.

#[allow(dead_code)]
mod support;

use cupertino_diagnostic_events::{self as diag, Record};
use cupertino_kit::controls::WINDOW_CONTENT_INSET;
use cupertino_kit::core::CommandRegistry;
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::{DecorationMode, MouseButton, PointerEvent, Timestamp, WindowId};
use cupertino_kit::prelude::*;
use cupertino_kit::test::{TestDriver, WindowEvent, WindowStates};
use cupertino_kit::text::TextSystem;
use cupertino_kit::{app, Instance};
use std::num::NonZeroU32;
use std::sync::{mpsc, Arc};

/// The time between two frames the test hands a window, in nanoseconds: long enough for a press to finish animating.
const FRAME_STEP: u64 = 500_000_000;

/// The most frames a window may ask for before it rests.
const FRAMES: usize = 64;

/// The press on which [`Fragile`] panics.
const BREAKS_AT: u32 = 2;

#[derive(Clone, Debug)]
struct Press;

/// One window with one button that counts its presses in its label.
fn counter(presses: u32) -> Vec<Window<Press>> {
    vec![Window {
        key: WindowKey("counter".into()),
        spec: WindowSpec {
            title: "Counter".into(),
            app_id: "org.cupertino.tests.Counter".into(),
            size: Size::new(240.0, 80.0),
            min_size: None,
            max_size: None,
            decorations: DecorationMode::Preferred,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background: WindowBackground::Theme,
        popup: None,
        content: Element::new(Stack {
            axis: Axis::Vertical,
            padding: WINDOW_CONTENT_INSET,
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![Element::new(Button {
                label: format!("Pressed {presses} times"),
                role: ButtonRole::Normal,
                style: ButtonStyle::Bordered,
                enabled: true,
                busy: false,
                on_press: Some(Press),
            })],
        }),
        menus: None,
    }]
}

/// Counts its presses.
struct Steady;

impl Application for Steady {
    type Model = u32;
    type Message = Press;

    fn init() -> (u32, Vec<Command<Press>>) {
        (0, Vec::new())
    }

    fn update(presses: &mut u32, Press: Press) -> Vec<Command<Press>> {
        *presses += 1;
        Vec::new()
    }

    fn view(presses: &u32) -> Vec<Window<Press>> {
        counter(*presses)
    }
}

/// Counts its presses and panics on the press [`BREAKS_AT`].
struct Fragile;

impl Application for Fragile {
    type Model = u32;
    type Message = Press;

    fn init() -> (u32, Vec<Command<Press>>) {
        (0, Vec::new())
    }

    fn update(presses: &mut u32, Press: Press) -> Vec<Command<Press>> {
        *presses += 1;
        assert!(*presses < BREAKS_AT, "the counter broke");
        Vec::new()
    }

    fn view(presses: &u32) -> Vec<Window<Press>> {
        counter(*presses)
    }
}

/// An application's window on the headless window system, and the time its frames have reached.
struct Shown<A: Application> {
    driver: TestDriver<A>,
    window: WindowId,
    at: u64,
}

impl<A: Application> Shown<A> {
    /// Opens the application's window on the process's shared text system and icons, configured and drawn until it
    /// rests.
    fn open() -> Self {
        let mut driver = support::driver_in::<A>(app::environment().unwrap());
        driver.run_until_idle(turns()).unwrap();
        let window = driver.runtime().windows()[0].1;
        let active = WindowStates {
            activated: true,
            ..WindowStates::default()
        };
        driver
            .window_system_mut()
            .configure(window, None, active)
            .unwrap();
        let mut shown = Self {
            driver,
            window,
            at: 0,
        };
        shown.settle();
        shown
    }

    /// Hands the window the frames it asks for until it asks for none.
    fn settle(&mut self) {
        for _ in 0..FRAMES {
            self.driver.run_until_idle(turns()).unwrap();
            if !self
                .driver
                .window_system_mut()
                .frame_requested(self.window)
                .unwrap()
            {
                return;
            }
            self.at += FRAME_STEP;
            self.driver.advance_to(Timestamp(self.at)).unwrap();
            self.driver
                .window_system_mut()
                .frame_ready(self.window, Timestamp(self.at))
                .unwrap();
        }
        panic!("the window keeps asking for frames");
    }

    /// The pixels of the frame the window presented last.
    fn drawn(&self) -> Vec<u8> {
        let frames = self
            .driver
            .runtime()
            .window_system()
            .frames(self.window)
            .unwrap();
        frames.last().expect("a presented frame").pixels.clone()
    }

    /// Clicks the window's button and lets the window draw what follows; returns the pixels it shows then.
    fn press(&mut self) -> Vec<u8> {
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        let button = snapshot
            .nodes
            .iter()
            .find(|n| n.widget.contains("::Button<"))
            .expect("the button is laid out");
        let position = button.rect.center();
        let click = |pressed| PointerEvent::Button {
            position,
            button: MouseButton::Left,
            pressed,
            serial: 1,
            mods: Default::default(),
        };
        let at = Timestamp(self.at);
        for event in [PointerEvent::Motion { position }, click(true), click(false)] {
            self.driver
                .window_system_mut()
                .enqueue_window(self.window, WindowEvent::Pointer(event, at))
                .unwrap();
        }
        self.settle();
        self.drawn()
    }
}

fn turns() -> NonZeroU32 {
    NonZeroU32::new(1000).unwrap()
}

#[test]
fn two_instances_draw_and_take_input_apart_and_a_panic_in_one_leaves_the_other_drawing() {
    diag::init(diag::component!(), "kit_test", diag::Sink::Memory);
    // the built-in font alone, whatever packs the machine has
    std::env::set_var(cupertino_assets::SEARCH_PATH_ENV, "");
    let (fragile_drawn, drawn) = mpsc::channel::<Arc<TextSystem>>();
    let (go_on, steady_waits) = mpsc::channel::<()>();
    let fragile = std::thread::Builder::new()
        .name("fragile-ui".into())
        .spawn(move || {
            let _fragile = Instance::new("fragile").enter();
            let mut shown = Shown::<Fragile>::open();
            let rest = shown.drawn();
            assert_ne!(shown.press(), rest, "the first press is drawn");
            assert_eq!(*shown.driver.runtime().model(), 1);
            fragile_drawn
                .send(app::environment().unwrap().text)
                .unwrap();
            shown.press();
        })
        .unwrap();
    let steady = std::thread::Builder::new()
        .name("steady-ui".into())
        .spawn(move || {
            let _steady = Instance::new("steady").enter();
            let mut shown = Shown::<Steady>::open();
            let mut last = shown.drawn();
            let mut frames = Vec::new();
            for wait in [false, true, false, false] {
                if wait {
                    steady_waits.recv().unwrap();
                }
                let now = shown.press();
                assert_ne!(now, last, "each press is drawn");
                last = now;
                frames.push(
                    shown
                        .driver
                        .runtime()
                        .window_system()
                        .frames(shown.window)
                        .unwrap()
                        .len(),
                );
            }
            let text = app::environment().unwrap().text;
            (*shown.driver.runtime().model(), frames, text)
        })
        .unwrap();
    let fragile_text = drawn
        .recv()
        .expect("the fragile instance drew its first press");
    let payload = fragile.join().expect_err("the fragile instance panics");
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
    assert_eq!(message, Some("the counter broke"));
    let panics: Vec<Record> = diag::recent()
        .iter()
        .map(|line| Record::parse(line.trim_end()).unwrap())
        .filter(|r| r.id.ends_with(".panic"))
        .collect();
    assert_eq!(panics.len(), 1, "{panics:?}");
    assert_eq!(panics[0].id, "fragile.panic");
    assert_eq!(
        panics[0].fields.get("thread").and_then(|v| v.as_str()),
        Some("fragile-ui")
    );
    go_on.send(()).unwrap();
    let (presses, frames, steady_text) = steady.join().expect("the steady instance goes on");
    assert!(Arc::ptr_eq(&fragile_text, &steady_text), "one text system");
    assert_eq!(presses, 4);
    assert!(frames.windows(2).all(|w| w[0] < w[1]), "{frames:?}");
}
