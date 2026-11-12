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

#[test]
fn a_footnote_wider_than_its_section_wraps_and_the_next_section_starts_below_every_line() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let (mut driver, window) = form_window(two_sections());
    let mut heights = Vec::new();
    for width in [MIN_CONTENT, 500.0, 900.0] {
        for scale in SCALES {
            resize(&mut driver, window, width, scale);
            let at = format!("{width} pt at {scale}x");
            let footer = bounds(&driver, window, LONG_FOOTNOTE);
            let row = bounds(&driver, window, "Zeigerbeschleunigung");
            let next = bounds(&driver, window, "Scrollen");
            let next_row = bounds(&driver, window, "Natürliche Scrollrichtung");
            let line = bounds(&driver, window, SHORT_FOOTNOTE).height();
            assert!(line > 0.0, "{at}: a laid-out caption");
            let (left, right) = (PAGE_INSET.left, width - PAGE_INSET.right);
            assert!(
                footer.x() >= left - SLACK && footer.max_x() <= right + SLACK,
                "{at}: the footnote stays in the section's width, {footer:?}"
            );
            assert!(
                footer.height() >= 2.0 * line - SLACK,
                "{at}: the footnote wraps, {footer:?} for lines of {line}"
            );
            assert!(
                footer.y() >= row.max_y() + form::FOOTER_GAP - SLACK,
                "{at}: the footnote starts below its section's group, {footer:?} after {row:?}"
            );
            assert!(
                (next.y() - (footer.max_y() + form::SECTION_GAP + form::SECTION_TITLE_TOP)).abs()
                    <= SLACK,
                "{at}: the next section's title starts a section gap and its own space below the footnote's last \
                 line, {next:?} after {footer:?}"
            );
            assert!(
                next_row.y() >= next.max_y() - SLACK && !overlaps(&footer, &next_row),
                "{at}: the next section's rows follow its header"
            );
            if scale == 1.0 {
                driver
                    .look(window, &format!("settings-long-footnote-{width}"))
                    .unwrap();
            }
            heights.push((width, scale, footer.height()));
        }
    }
    for scale in SCALES {
        let at: Vec<f64> = heights
            .iter()
            .filter(|(_, s, _)| *s == scale)
            .map(|(_, _, h)| *h)
            .collect();
        assert!(
            at.windows(2).all(|pair| pair[0] >= pair[1]) && at[0] > at[at.len() - 1],
            "at {scale}x a narrower section gives the footnote more lines: {at:?}"
        );
    }
}

#[test]
fn a_row_whose_label_is_wider_than_its_section_wraps_and_keeps_its_control_in_the_window() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let (mut driver, window) = form_window(long_rows());
    for width in [MIN_CONTENT, 500.0, 900.0] {
        for scale in SCALES {
            resize(&mut driver, window, width, scale);
            let at = format!("{width} pt at {scale}x");
            let (left, right) = (PAGE_INSET.left, width - PAGE_INSET.right);
            let label = bounds_of(&driver, window, Role::StaticText, LONG_LABEL);
            let detail = bounds_of(&driver, window, Role::StaticText, LONG_DETAIL);
            let switch = bounds_of(&driver, window, Role::Switch, LONG_LABEL);
            let next = bounds_of(&driver, window, Role::StaticText, "Scrollen");
            let line = bounds_of(&driver, window, Role::StaticText, SHORT_FOOTNOTE).height();
            assert!(
                switch.x() >= left - SLACK && switch.max_x() <= right + SLACK,
                "{at}: the switch stays in the section, {switch:?}"
            );
            for text in [&label, &detail] {
                assert!(
                    text.x() >= left - SLACK && text.max_x() <= switch.x() - form::ROW_GAP + SLACK,
                    "{at}: the label ends a row gap before its switch, {text:?} and {switch:?}"
                );
            }
            assert!(
                detail.y() >= label.max_y() - SLACK,
                "{at}: the second line follows the label"
            );
            if width < 900.0 {
                assert!(
                    detail.height() >= 2.0 * line - SLACK,
                    "{at}: the second line wraps, {detail:?} for lines of {line}"
                );
            }
            assert!(
                next.y() >= detail.max_y().max(switch.max_y()) + form::ROW_GAP - SLACK,
                "{at}: the next row starts below every line of this one, {next:?} after {detail:?}"
            );
            if scale == 1.0 {
                driver
                    .look(window, &format!("settings-long-row-{width}"))
                    .unwrap();
            }
        }
    }
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

#[test]
fn the_mouse_resolution_footnote_wraps_without_clipping_or_overlapping_at_every_width_and_scale() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let Some(bus) = cupertino_test_bus::PrivateBus::start() else {
        eprintln!("skipped: dbus-daemon is not available");
        return;
    };
    let _session = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .serve_at("/org/cupertino/Session1", FakeMice)
        .unwrap()
        .name("org.cupertino.Session1")
        .unwrap()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let services = Services::on(Bus::address(NOWHERE), Bus::at(bus.connect()));
    // the mice and their footnote are on the Devices tab
    let (mut driver, window) = start_state(dir.path(), PaneId::Mouse, services, |state| {
        state.live.tab = 2;
    });
    let footnote = crate::panes::MOUSE_RESOLUTION_FOOTER;
    // a footnote's line at each scale, read where the footnote fits on one line: the widest window, which comes first
    let mut caption_line = [0.0; SCALES.len()];
    for width in WIDTHS {
        for (s, scale) in SCALES.into_iter().enumerate() {
            resize(&mut driver, window, width, scale);
            let at = format!("{width} pt at {scale}x");
            let footer = bounds(&driver, window, footnote);
            let snapshot = driver.runtime().snapshot(window).unwrap();
            // the pane's scroll view, right of the sidebar; the sidebar folds away at the least width
            let page = snapshot
                .accessibility
                .iter()
                .filter(|(_, n)| n.role == Role::ScrollView)
                .map(|(_, n)| n.bounds)
                .max_by(|a, b| a.x().total_cmp(&b.x()))
                .expect("the pane's scroll view");
            assert!(
                (page.max_x() - width).abs() <= SLACK,
                "{at}: the pane runs to the window's edge, {page:?}"
            );
            let content_left = page.x();
            assert!(
                footer.x() >= content_left + PAGE_INSET.left - SLACK
                    && footer.max_x() <= width - PAGE_INSET.right + SLACK,
                "{at}: the footnote stays in the content, {footer:?} in {page:?}"
            );
            // the wide window has room for the whole footnote on one line; from the default width on it wraps
            let wraps = width <= metrics::DEFAULT_WIDTH;
            if !wraps {
                caption_line[s] = footer.height();
            }
            let line = caption_line[s];
            assert!(line > 0.0, "{at}: the footnote's line was read first");
            let lines = (footer.height() / line).round();
            assert!(
                lines == if wraps { 2.0 } else { 1.0 } && footer.height() >= lines * line - SLACK,
                "{at}: the footnote has room for each of its lines, {footer:?} for lines of {line}"
            );
            let content: Vec<(String, Rect)> = snapshot
                .accessibility
                .iter()
                .filter(|(_, n)| {
                    matches!(
                        n.role,
                        Role::StaticText | Role::Switch | Role::Slider | Role::PopUpButton
                    )
                })
                .filter(|(_, n)| n.bounds.x() >= content_left && n.bounds.y() >= page.y())
                .map(|(_, n)| (n.name.clone().unwrap_or_default(), n.bounds))
                .collect();
            for (i, (a, ra)) in content.iter().enumerate() {
                assert!(
                    ra.max_x() <= width + SLACK,
                    "{at}: {a:?} is not cut off at the window's edge, {ra:?}"
                );
                for (b, rb) in &content[i + 1..] {
                    assert!(
                        !overlaps(ra, rb),
                        "{at}: {a:?} {ra:?} and {b:?} {rb:?} overlap"
                    );
                }
            }
            let last_row = content
                .iter()
                .filter(|(name, _)| name != footnote)
                .map(|(_, r)| r.max_y())
                .fold(0.0, f64::max);
            assert!(
                footer.y() >= last_row - SLACK,
                "{at}: the footnote follows the last row"
            );
            if width == metrics::DEFAULT_WIDTH && scale == 1.0 {
                driver.look(window, "settings-mouse-resolution").unwrap();
            }
        }
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

#[test]
fn the_notifications_pane_lays_out_each_app_without_clipping_or_overlapping_at_every_width_and_scale(
) {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let Some(bus) = cupertino_test_bus::PrivateBus::start() else {
        eprintln!("skipped: dbus-daemon is not available");
        return;
    };
    let _centre = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .serve_at("/org/cupertino/NotificationCenter1", FakeCentre)
        .unwrap()
        .name("org.cupertino.NotificationCenter1")
        .unwrap()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("notifications.conf"),
        "show_previews = when-unlocked\napp.org.example.Mail.show_previews = never\n",
    )
    .unwrap();
    let services = Services::on(Bus::address(NOWHERE), Bus::at(bus.connect()));
    let (mut driver, window) = start_on(dir.path(), PaneId::Notifications, None, services);
    for width in WIDTHS {
        for scale in SCALES {
            resize(&mut driver, window, width, scale);
            let at = format!("{width} pt at {scale}x");
            let snapshot = driver.runtime().snapshot(window).unwrap();
            let page = snapshot
                .accessibility
                .iter()
                .filter(|(_, n)| n.role == Role::ScrollView)
                .map(|(_, n)| n.bounds)
                .max_by(|a, b| a.x().total_cmp(&b.x()))
                .expect("the pane's scroll view");
            let content: Vec<(String, Rect)> = snapshot
                .accessibility
                .iter()
                .filter(|(_, n)| {
                    matches!(n.role, Role::StaticText | Role::Switch | Role::PopUpButton)
                })
                .filter(|(_, n)| n.bounds.x() >= page.x() && n.bounds.y() >= page.y())
                .map(|(_, n)| (n.name.clone().unwrap_or_default(), n.bounds))
                .collect();
            for name in ["Calendar", "Mail", "Application Notifications"] {
                assert!(
                    content.iter().any(|(n, _)| n == name),
                    "{at}: {name:?} shows"
                );
            }
            for (i, (a, ra)) in content.iter().enumerate() {
                assert!(
                    ra.max_x() <= width + SLACK,
                    "{at}: {a:?} is not cut off at the window's edge, {ra:?}"
                );
                for (b, rb) in &content[i + 1..] {
                    assert!(
                        !overlaps(ra, rb),
                        "{at}: {a:?} {ra:?} and {b:?} {rb:?} overlap"
                    );
                }
            }
            if width == metrics::DEFAULT_WIDTH && scale == 1.0 {
                driver.look(window, "settings-notifications").unwrap();
            }
        }
    }
}

#[test]
fn each_apps_notification_popups_read_a_name_of_their_own_with_the_apps_name() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let Some(bus) = cupertino_test_bus::PrivateBus::start() else {
        eprintln!("skipped: dbus-daemon is not available");
        return;
    };
    let _centre = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .serve_at("/org/cupertino/NotificationCenter1", FakeCentre)
        .unwrap()
        .name("org.cupertino.NotificationCenter1")
        .unwrap()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let services = Services::on(Bus::address(NOWHERE), Bus::at(bus.connect()));
    let (driver, window) = start_on(dir.path(), PaneId::Notifications, None, services);
    let mut popups: Vec<String> = driver
        .runtime()
        .snapshot(window)
        .unwrap()
        .accessibility
        .into_iter()
        .filter(|(_, n)| n.role == Role::PopUpButton)
        .filter_map(|(_, n)| n.name)
        .collect();
    popups.sort();
    assert_eq!(
        popups,
        [
            "Alert style, Calendar",
            "Alert style, Mail",
            "Show on lock screen, Calendar",
            "Show on lock screen, Mail",
            "Show previews, Calendar",
            "Show previews, Lock Screen",
            "Show previews, Mail",
        ],
        "a screen reader tells the popups of one app from another's"
    );
}
