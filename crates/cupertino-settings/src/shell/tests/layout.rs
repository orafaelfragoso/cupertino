//! Where a pane's sections lie: a footnote wider than its section wraps to as many lines as it needs, the section keeps
//! room for every line, and the next section starts below it, at every width the window allows and at each scale.
//! Footnotes and second lines are set in the caption size; a one-line caption of each form gives its line height.

use cupertino_kit::geometry::Rect;
use cupertino_kit::platform::DecorationMode;

use super::*;
use crate::controls::Control;
use crate::pages::Block;
use crate::panes::{Backend, Row, Section};

/// The scales the layout is checked at.
const SCALES: [f64; 4] = [1.0, 1.25, 1.5, 2.0];
/// A wide window, the default width and the least, widest first.
const WIDTHS: [f64; 3] = [1200.0, metrics::DEFAULT_WIDTH, MIN_SIDEBAR + MIN_CONTENT];
/// A rounding slack for comparing laid-out edges.
const SLACK: f64 = 0.5;

/// A footnote short enough for one line at every width: its height is a caption line's.
const SHORT_FOOTNOTE: &str = "Kurz.";

/// Long translated-style text: long words, a hyphenated compound and no break the layout could lean on.
const LONG_FOOTNOTE: &str = "Stimmen Sie diesen Wert auf die DPI-Einstellung Ihrer Maus ab, damit sich die \
    Zeigergeschwindigkeit bei jeder angeschlossenen Maus gleich anfühlt, auch nach einem Neustart des Rechners \
    oder wenn Sie zwischen verschiedenen Arbeitsplätzen wechseln.";

fn switch_row(id: ControlId, label: &str) -> Row {
    Row {
        id,
        label: label.into(),
        control: Control::Switch { on: false },
        backend: Backend::Service("Layout"),
        error: None,
        disabled: None,
        icon: None,
        detail: None,
    }
}

/// A section with a long footnote, then another section.
fn two_sections() -> Vec<Section> {
    vec![
        Section {
            header: Some("Zeiger".into()),
            rows: vec![switch_row(
                ControlId::MouseAcceleration,
                "Zeigerbeschleunigung",
            )],
            footer: Some(LONG_FOOTNOTE.into()),
        },
        Section {
            header: Some("Scrollen".into()),
            rows: vec![switch_row(
                ControlId::MouseNatural,
                "Natürliche Scrollrichtung",
            )],
            footer: Some(SHORT_FOOTNOTE.into()),
        },
    ]
}

/// A long translated-style label and second line of a row.
const LONG_LABEL: &str = "Mauszeiger beim Schütteln der Maus vorübergehend vergrößern";
const LONG_DETAIL: &str = "Liest den Bildschirminhalt laut vor. Mit Super+Alt+S schalten Sie die \
    Bildschirmausgabe jederzeit ein oder aus, auch im Anmeldebildschirm.";

/// A section of a row with a long label and second line, then a row with a short one.
fn long_rows() -> Vec<Section> {
    let mut long = switch_row(ControlId::ShakeToLocate, LONG_LABEL);
    long.detail = Some(LONG_DETAIL.into());
    vec![Section {
        header: Some("Zeiger".into()),
        rows: vec![long, switch_row(ControlId::MouseNatural, "Scrollen")],
        footer: Some(SHORT_FOOTNOTE.into()),
    }]
}

static FORM: Mutex<Vec<Section>> = Mutex::new(Vec::new());

/// The form of a pane alone in a window, inset as the Settings page is.
struct FormApp;

impl Application for FormApp {
    type Model = Vec<Section>;
    type Message = Message;

    fn init() -> (Vec<Section>, Vec<Command<Message>>) {
        (FORM.lock().unwrap().clone(), Vec::new())
    }

    fn update(_: &mut Vec<Section>, _: Message) -> Vec<Command<Message>> {
        Vec::new()
    }

    fn view(sections: &Vec<Section>) -> Vec<Window<Message>> {
        vec![Window {
            key: WindowKey("form".into()),
            spec: WindowSpec {
                title: "Form".into(),
                app_id: APP_ID.into(),
                size: Size::new(MIN_CONTENT, metrics::DEFAULT_HEIGHT),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            content: Element::new(Stack {
                axis: Axis::Vertical,
                padding: PAGE_INSET,
                spacing: 0.0,
                align: Align::Stretch,
                children: vec![form::page(
                    None,
                    &sections
                        .iter()
                        .cloned()
                        .map(Block::Section)
                        .collect::<Vec<_>>(),
                    sections,
                    &form::View {
                        open: None,
                        icons: &std::collections::HashMap::new(),
                        thumbnails: &std::collections::HashMap::new(),
                    },
                )],
            }),
            popup: None,
            menus: None,
        }]
    }
}

/// Size `window` to `width` at `scale` and let it lay out.
fn resize<A: Application>(driver: &mut TestDriver<A>, window: WindowId, width: f64, scale: f64) {
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    let system = driver.window_system_mut();
    system
        .configure(
            window,
            Some(Size::new(width, metrics::DEFAULT_HEIGHT)),
            active,
        )
        .unwrap();
    system
        .enqueue_window(window, WindowEvent::Scale(scale))
        .unwrap();
    settle(driver, window);
}

/// The laid-out bounds of the node named `name`.
fn bounds<A: Application>(driver: &TestDriver<A>, window: WindowId, name: &str) -> Rect {
    driver
        .runtime()
        .snapshot(window)
        .unwrap()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("a node named {name:?}"))
        .1
        .bounds
}

/// The laid-out bounds of the node with `role` named `name`.
fn bounds_of<A: Application>(
    driver: &TestDriver<A>,
    window: WindowId,
    role: Role,
    name: &str,
) -> Rect {
    driver
        .runtime()
        .snapshot(window)
        .unwrap()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.role == role && n.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("a {role:?} named {name:?}"))
        .1
        .bounds
}

/// A window of [`FormApp`] showing `sections`.
fn form_window(sections: Vec<Section>) -> (TestDriver<FormApp>, WindowId) {
    *FORM.lock().unwrap() = sections;
    let mut driver = TestDriver::<FormApp>::new(
        Capabilities::default(),
        runtime_options(),
        ManualClock::new(Timestamp(0)),
    );
    driver.run_until_idle(budget()).unwrap();
    let window = driver.runtime().windows()[0].1;
    (driver, window)
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x() < b.max_x() - SLACK
        && b.x() < a.max_x() - SLACK
        && a.y() < b.max_y() - SLACK
        && b.y() < a.max_y() - SLACK
}

struct FakeMice;

#[zbus::interface(name = "org.cupertino.Session1")]
impl FakeMice {
    #[zbus(property)]
    fn mice(&self) -> Vec<(String, String, u32, bool)> {
        vec![
            ("1:1:Mouse".into(), "USB Optical Mouse".into(), 800, false),
            ("2:2:Mouse".into(), "Wireless Mouse".into(), 1600, true),
        ]
    }
}

struct FakeCentre;

#[zbus::interface(name = "org.cupertino.NotificationCenter1")]
impl FakeCentre {
    fn applications(&self) -> Vec<(String, String)> {
        vec![
            ("org.example.Calendar".into(), "Calendar".into()),
            ("org.example.Mail".into(), "Mail".into()),
        ]
    }
}
