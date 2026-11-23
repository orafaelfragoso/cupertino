//! The keyboard's two panels in the on-screen displays (W6.12): what the server says of the input-source switcher and
//! the accent picker reaches \`Osd\` as its messages; the switcher's panel keeps one window through a switch, fades from
//! the switch's end or from its first frame when the end came before it, and goes when the fade ends; a close of
//! another picker leaves the open one; the picker lies by the text cursor inside its output; the switcher's window
//! takes no input, and a candidate picks only when the pointer is pressed and released on it.

use std::num::NonZeroU32;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{
    Application, Command, GlassModifiers, GlassVariant, Key, Role, RuntimeOptions, Window,
    WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    GlassStyle, Mods, MouseButton, PointerEvent, Timestamp, WindowId, WindowRequest,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::test::{
    Capabilities, FontIndex, ManualClock, PlaceholderIcons, TestDriver, WindowEvent, WindowStates,
};
use cupertino_kit::text::{GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_kit_shell::keyboard_panels::{Source, Update};
use cupertino_kit_shell::{Event, Program};
use cupertino_osd::panels::{source_code, Panels, MAX_CANDIDATES};
use cupertino_osd::panels_view::{accents_key, accents_window, sources_key, sources_window};
use cupertino_osd::{Message, Osd};

/// The runtime turns between two looks at the window.
const TURNS: u32 = 1000;
/// The output the picker is laid out on, in points.
const OUTPUT: Size = Size::new(800.0, 600.0);
/// The height of the menu bar the picker stays below.
const MENU_BAR: f64 = 24.0;
/// The width the tests give the switcher's widest label.
const WIDEST: f64 = 180.0;

fn sources(names: &[(&str, &str)], selected: usize) -> Update {
    Update::Sources {
        output: None,
        sources: names
            .iter()
            .map(|(id, name)| Source {
                id: (*id).to_owned(),
                name: (*name).to_owned(),
            })
            .collect(),
        selected,
    }
}

const TWO: [(&str, &str); 2] = [("us", "English"), ("de", "German")];

fn accents(serial: u32, anchor: Rect, count: usize, focused: Option<usize>) -> Update {
    Update::Accents {
        serial,
        output: None,
        anchor,
        top: MENU_BAR,
        candidates: ["è", "é", "ê", "ë", "ē", "ė", "ę", "ẽ", "ě", "ȩ", "ḙ"][..count]
            .iter()
            .map(|c| (*c).to_owned())
            .collect(),
        focused,
    }
}

#[test]
fn the_servers_updates_reach_the_component_as_its_messages() {
    for update in [
        sources(&TWO, 1),
        Update::SourcesClosed,
        accents(3, Rect::new(10.0, 40.0, 2.0, 18.0), 4, None),
        Update::AccentsClosed { serial: 3 },
    ] {
        let message = <Osd as Program>::event(Event::KeyboardPanels(update.clone()));
        assert!(
            matches!(&message, Some(Message::Panels(u)) if *u == update),
            "{update:?} reaches the component"
        );
    }
    let key = WindowKey(Key::from("osd-accents-3"));
    assert!(matches!(
        <Osd as Program>::event(Event::Closed(key.clone())),
        Some(Message::Closed(k)) if k == key
    ));
}

#[test]
fn a_switch_keeps_one_window_and_each_step_moves_its_selection() {
    let mut panels = Panels::default();
    panels.apply(sources(&TWO, 1), 0.0);
    let first = panels.sources.clone().expect("the list opens the panel");
    assert_eq!(first.selected, 1);
    assert!(first.wants_frames(), "it waits for its first frame");
    panels.sources_frame(0.05);
    assert!(!panels.sources.as_ref().unwrap().wants_frames());

    panels.apply(sources(&TWO, 0), 0.1);
    let step = panels.sources.clone().unwrap();
    assert_eq!(step.selected, 0);
    assert_eq!(
        sources_key(&step),
        sources_key(&first),
        "a step keeps the window"
    );
    // a selection past the list is its last row
    panels.apply(sources(&TWO, 7), 0.2);
    assert_eq!(panels.sources.as_ref().unwrap().selected, 1);
}

#[test]
fn the_end_of_a_switch_fades_the_panel_out_over_three_tenths_of_a_second() {
    let mut panels = Panels::default();
    panels.apply(sources(&TWO, 1), 0.0);
    panels.sources_frame(0.02);
    panels.apply(Update::SourcesClosed, 1.0);
    let panel = panels.sources.clone().unwrap();
    assert_eq!(panel.fading_since, Some(1.0));
    assert!(
        panel.wants_frames(),
        "it is drawn at every frame while it fades"
    );
    assert_eq!(panel.alpha(1.0), 1.0);
    assert!((panel.alpha(1.15) - 0.5).abs() < 1e-6, "constant speed");
    panels.sources_frame(1.2);
    assert!(panels.sources.is_some());
    panels.tick(1.31);
    assert_eq!(panels.sources, None, "the faded panel goes");

    // a new switch after the close opens a new window
    panels.apply(sources(&TWO, 0), 2.0);
    assert_ne!(
        sources_key(panels.sources.as_ref().unwrap()),
        sources_key(&panel)
    );
}

#[test]
fn a_switch_that_ends_before_the_first_frame_still_shows_the_choice_fading() {
    let mut panels = Panels::default();
    panels.apply(sources(&TWO, 1), 0.0);
    panels.apply(Update::SourcesClosed, 0.01);
    let panel = panels.sources.clone().expect("the quick tap's panel stays");
    assert!(panel.closed);
    assert_eq!(
        panel.fading_since, None,
        "its fade waits for its first frame"
    );
    assert_eq!(panel.alpha(5.0), 1.0);
    panels.tick(5.0);
    assert!(
        panels.sources.is_some(),
        "nothing takes it away before it showed"
    );

    panels.sources_frame(5.0);
    assert_eq!(panels.sources.as_ref().unwrap().fading_since, Some(5.0));
    // a second close changes nothing
    panels.apply(Update::SourcesClosed, 5.1);
    assert_eq!(panels.sources.as_ref().unwrap().fading_since, Some(5.0));
    panels.sources_frame(5.31);
    assert_eq!(panels.sources, None);
}

#[test]
fn only_the_open_pickers_close_takes_it_away() {
    let caret = Rect::new(400.0, 300.0, 2.0, 18.0);
    let mut panels = Panels::default();
    panels.apply(accents(4, caret, 7, Some(2)), 0.0);
    assert_eq!(panels.accents.as_ref().unwrap().focused, Some(2));
    panels.apply(Update::AccentsClosed { serial: 3 }, 0.1);
    assert!(panels.accents.is_some(), "another picker's close leaves it");
    // a focus past the candidates is none
    panels.apply(accents(4, caret, 7, Some(7)), 0.2);
    assert_eq!(panels.accents.as_ref().unwrap().focused, None);
    // a new picker replaces it, with a window of its own
    let old = accents_key(panels.accents.as_ref().unwrap());
    panels.apply(accents(5, caret, 3, None), 0.3);
    assert_ne!(accents_key(panels.accents.as_ref().unwrap()), old);
    panels.apply(Update::AccentsClosed { serial: 4 }, 0.4);
    assert!(panels.accents.is_some());
    panels.apply(Update::AccentsClosed { serial: 5 }, 0.5);
    assert_eq!(panels.accents, None);
}

/// Whether \`inner\` lies inside \`outer\`.
fn inside(inner: Rect, outer: Rect) -> bool {
    inner.min_x() >= outer.min_x() - 1e-9
        && inner.min_y() >= outer.min_y() - 1e-9
        && inner.max_x() <= outer.max_x() + 1e-9
        && inner.max_y() <= outer.max_y() + 1e-9
}

#[test]
fn the_picker_lies_by_the_caret_inside_the_output_and_below_the_menu_bar() {
    let screen = Rect::new(0.0, MENU_BAR, OUTPUT.width, OUTPUT.height - MENU_BAR);
    for caret in [
        Rect::new(400.0, 300.0, 2.0, 18.0),
        Rect::new(790.0, 580.0, 2.0, 18.0),
        Rect::new(5.0, 30.0, 2.0, 18.0),
    ] {
        let mut panels = Panels::default();
        panels.apply(accents(1, caret, 11, None), 0.0);
        let layout = panels.accents.as_ref().unwrap().layout(OUTPUT);
        assert!(inside(layout.rect, screen), "{caret:?}: {:?}", layout.rect);
        assert_eq!(
            layout.cells.len(),
            MAX_CANDIDATES,
            "one cell for each digit"
        );
        let local = Rect::new(0.0, 0.0, layout.rect.width(), layout.rect.height());
        assert!(layout.cells.iter().all(|c| inside(*c, local)));
        assert!(
            layout
                .cells
                .windows(2)
                .all(|w| w[0].max_x() <= w[1].min_x() + 1e-9),
            "the cells run left to right"
        );
    }
    // in the open, the picker is just above or below the caret, across it, and leaves it uncovered
    let caret = Rect::new(400.0, 300.0, 2.0, 18.0);
    let mut panels = Panels::default();
    panels.apply(accents(1, caret, 3, None), 0.0);
    let layout = panels.accents.as_ref().unwrap().layout(OUTPUT);
    let r = layout.rect;
    assert!(
        r.max_y() <= caret.min_y() || r.min_y() >= caret.max_y(),
        "{r:?}"
    );
    assert!(
        r.min_x() <= caret.min_x() && r.max_x() >= caret.max_x(),
        "{r:?}"
    );
    assert!(
        (r.min_y() - caret.max_y()).abs() < r.height()
            || (caret.min_y() - r.max_y()).abs() < r.height()
    );
    assert_eq!(layout.cells.len(), 3);
}

#[test]
fn a_source_icon_shows_the_first_two_letters_of_its_layout() {
    assert_eq!(source_code("us"), "US");
    assert_eq!(source_code("fr(nodeadkeys)"), "FR");
    assert_eq!(source_code("dvorak"), "DV");
    assert_eq!(source_code("x"), "X");
}

/// The panels' windows, and the candidates picked through them.
#[derive(Default)]
struct Shown {
    panels: Panels,
    picks: Vec<(u32, usize)>,
}

struct PanelsApp;

impl Application for PanelsApp {
    type Model = Shown;
    type Message = (u32, usize);

    fn init() -> (Shown, Vec<Command<(u32, usize)>>) {
        let mut shown = Shown::default();
        shown.panels.apply(sources(&TWO, 1), 0.0);
        shown.panels.apply(
            accents(6, Rect::new(100.0, 100.0, 2.0, 18.0), 7, Some(1)),
            0.0,
        );
        (shown, Vec::new())
    }

    fn update(shown: &mut Shown, pick: (u32, usize)) -> Vec<Command<(u32, usize)>> {
        shown.picks.push(pick);
        Vec::new()
    }

    fn view(shown: &Shown) -> Vec<Window<(u32, usize)>> {
        let sources = shown.panels.sources.as_ref().unwrap();
        let accents = shown.panels.accents.as_ref().unwrap();
        let serial = accents.serial;
        vec![
            sources_window(sources, sources.size(WIDEST), 1.0, None),
            accents_window(accents, &accents.layout(OUTPUT), move |i| (serial, i)),
        ]
    }
}

/// Options that draw on the CPU with the built-in font, in the neutral light theme.
fn options() -> RuntimeOptions {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
    let glyphs = Arc::clone(&text);
    RuntimeOptions {
        theme: Arc::new(
            ThemeTokens::neutral(ThemePreferences {
                appearance: Appearance::Light,
                reduce_transparency: false,
                reduce_motion: true,
            })
            .unwrap(),
        ),
        text,
        icons: Arc::new(PlaceholderIcons::default()),
        direction: LayoutDirection::Ltr,
        node_limit: 1000,
        renderers: Box::new(move || {
            Box::new(CpuRenderer::new(
                CpuConfig {
                    max_pixels: 1 << 22,
                    glyph_cache_bytes: 1 << 20,
                    image_cache_bytes: 1 << 20,
                    font_smoothing: true,
                },
                GlyphRaster::new(Arc::clone(&glyphs)),
            ))
        }),
        accessibility: None,
        clock: Arc::new(ManualClock::new(Timestamp(0))),
        on_window_error: None,
    }
}

fn turns() -> NonZeroU32 {
    NonZeroU32::new(TURNS).unwrap()
}

/// The panels' windows shown and drawn until they rest.
struct Driven {
    driver: TestDriver<PanelsApp>,
}

impl Driven {
    fn open() -> Self {
        let mut driver = TestDriver::<PanelsApp>::new(
            Capabilities::default(),
            options(),
            ManualClock::new(Timestamp(0)),
        );
        driver.run_until_idle(turns()).unwrap();
        for (_, window) in driver.runtime().windows() {
            driver
                .window_system_mut()
                .configure(window, None, WindowStates::default())
                .unwrap();
        }
        let mut driven = Driven { driver };
        driven.settle();
        driven
    }

    fn settle(&mut self) {
        for _ in 0..64 {
            self.driver.run_until_idle(turns()).unwrap();
            let windows: Vec<WindowId> = self
                .driver
                .runtime()
                .windows()
                .into_iter()
                .map(|(_, w)| w)
                .collect();
            let system = self.driver.window_system_mut();
            let mut asked = false;
            for window in windows {
                if system.frame_requested(window).unwrap() {
                    system.frame_ready(window, Timestamp(0)).unwrap();
                    asked = true;
                }
            }
            if !asked {
                return;
            }
        }
    }

    fn window(&self, key: &str) -> WindowId {
        self.driver
            .runtime()
            .windows()
            .into_iter()
            .find(|(k, _)| k.0 == Key::from(key.to_owned()))
            .map(|(_, w)| w)
            .unwrap_or_else(|| panic!("no window {key}"))
    }

    /// The role, the name and the bounds of each accessibility node of `window`.
    fn nodes(&self, window: WindowId) -> Vec<(Role, Option<String>, Rect)> {
        self.driver
            .runtime()
            .snapshot(window)
            .map(|s| {
                s.accessibility
                    .into_iter()
                    .map(|(_, n)| (n.role, n.name, n.bounds))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn button(&mut self, window: WindowId, position: Point, pressed: bool) {
        self.driver
            .window_system_mut()
            .enqueue_window(
                window,
                WindowEvent::Pointer(PointerEvent::Motion { position }, Timestamp(0)),
            )
            .unwrap();
        self.driver
            .window_system_mut()
            .enqueue_window(
                window,
                WindowEvent::Pointer(
                    PointerEvent::Button {
                        position,
                        button: MouseButton::Left,
                        pressed,
                        serial: 1,
                        mods: Mods::NONE,
                    },
                    Timestamp(0),
                ),
            )
            .unwrap();
        self.settle();
    }
}

#[test]
fn the_switchers_window_lists_its_sources_and_takes_no_input() {
    let driven = Driven::open();
    let shown = driven.driver.runtime().model();
    let panel = shown.panels.sources.as_ref().unwrap();
    let window = sources_window::<()>(panel, panel.size(WIDEST), 1.0, None);
    assert_eq!(
        window.spec.input_region,
        Some(Vec::new()),
        "the switcher takes no input"
    );
    assert_eq!(window.key.0, Key::from(sources_key(panel)));
    let picker = shown.panels.accents.as_ref().unwrap();
    let window = accents_window(picker, &picker.layout(OUTPUT), |_| ());
    assert_eq!(
        window.spec.input_region, None,
        "the whole picker takes the pointer"
    );

    let names: Vec<String> = driven
        .nodes(driven.window(&sources_key(panel)))
        .into_iter()
        .filter(|(role, ..)| *role == Role::StaticText)
        .filter_map(|(_, name, _)| name)
        .collect();
    assert_eq!(names, ["English", "German"]);
}

#[test]
fn a_candidate_picks_when_the_pointer_is_pressed_and_released_on_it() {
    let mut driven = Driven::open();
    let key = accents_key(
        driven
            .driver
            .runtime()
            .model()
            .panels
            .accents
            .as_ref()
            .unwrap(),
    );
    let window = driven.window(&key);
    let nodes = driven.nodes(window);
    let candidate = |name: &str| {
        nodes
            .iter()
            .find(|(role, n, _)| *role == Role::Button && n.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no candidate {name}"))
            .2
    };
    assert_eq!(
        nodes
            .iter()
            .filter(|(role, ..)| *role == Role::Button)
            .count(),
        7
    );
    let (second, third) = (candidate("é"), candidate("ê"));

    driven.button(window, second.center(), true);
    assert!(
        driven.driver.runtime().model().picks.is_empty(),
        "a press alone picks nothing"
    );
    driven.button(window, second.center(), false);
    assert_eq!(driven.driver.runtime().model().picks, [(6, 1)]);

    // pressed on one and released on another: nothing
    driven.button(window, second.center(), true);
    driven.button(window, third.center(), false);
    assert_eq!(driven.driver.runtime().model().picks, [(6, 1)]);
    // released on the third after a press on it
    driven.button(window, third.center(), true);
    driven.button(window, third.center(), false);
    assert_eq!(driven.driver.runtime().model().picks, [(6, 1), (6, 2)]);
}

#[test]
fn the_switcher_lies_on_regular_glass_and_the_picker_on_menu_glass_without_its_shadow() {
    let capabilities = Capabilities {
        app_effects: true,
        ..Capabilities::default()
    };
    let mut driver =
        TestDriver::<PanelsApp>::new(capabilities, options(), ManualClock::new(Timestamp(0)));
    driver.run_until_idle(turns()).unwrap();
    for (_, window) in driver.runtime().windows() {
        driver
            .window_system_mut()
            .configure(window, None, WindowStates::default())
            .unwrap();
        driver.run_until_idle(turns()).unwrap();
    }
    let windows = driver.runtime().windows();
    let system = driver.window_system_mut();
    let glass: Vec<(String, Vec<_>)> = windows
        .into_iter()
        .map(|(_, window)| {
            let title = system.spec(window).unwrap().title.clone();
            let glass = system
                .requests(window)
                .unwrap()
                .iter()
                .rev()
                .find_map(|r| match r {
                    WindowRequest::Materials(regions) => Some(
                        regions
                            .iter()
                            .filter_map(|r| r.glass)
                            .map(|g| (g.style, g.variant))
                            .collect(),
                    ),
                    _ => None,
                })
                .unwrap_or_default();
            (title, glass)
        })
        .collect();
    let of = |title: &str| {
        glass
            .iter()
            .find(|(t, _)| t == title)
            .map(|(_, g)| g.clone())
            .unwrap_or_else(|| panic!("no window {title}: {glass:?}"))
    };
    assert_eq!(
        of("Input sources"),
        [(
            GlassStyle::Regular,
            Some((GlassVariant::Regular, GlassModifiers::DEFAULT))
        )]
    );
    assert_eq!(
        of("Accents"),
        [(
            GlassStyle::Menu,
            Some((
                GlassVariant::Menu,
                GlassModifiers {
                    exclude_shadow: true,
                    ..GlassModifiers::DEFAULT
                }
            ))
        )]
    );
}
