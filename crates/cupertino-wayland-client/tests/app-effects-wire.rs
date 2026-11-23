//! The app effects protocol against an in-process server: appearance and display options, backdrops, the title bar
//! style, the menu announcement and disabled window controls.

use crate::support;

use cupertino_app_protocols::server::app_effects::{
    cupertino_appearance_v1::DisplayOption, cupertino_backdrop_v1::Tone,
};
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_geometry::Rect;
use cupertino_wayland_client::app::{
    AppEffects, AppEvent, Backdrop, BackdropTone, Error, GlassStyle, MenuAddress, WindowControls,
    WindowStyle,
};
use cupertino_wayland_client::{Connection, SurfaceId};
use support::*;

struct Setup {
    server: Harness,
    conn: Connection,
    effects: AppEffects,
    surface: SurfaceId,
}

fn setup(version: u32) -> Setup {
    let (server, mut conn) = Harness::start(Offer {
        app_effects: Some(version),
        ..Offer::NONE
    });
    let surface = window(&mut conn).surface();
    let effects = AppEffects::bind(&mut conn).expect("the server offers app effects");
    Setup {
        server,
        conn,
        effects,
        surface,
    }
}

impl Setup {
    /// What the server received after the objects were made, once it has handled everything sent so far.
    fn sent(&mut self) -> Vec<String> {
        self.conn.roundtrip().unwrap();
        self.server
            .requests()
            .into_iter()
            .filter(|r| !r.starts_with("get_"))
            .collect()
    }

    fn backdrop(&self) -> Backdrop {
        self.effects.backdrop(&self.conn, self.surface).unwrap()
    }
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect::new(x, y, w, h)
}

fn address(app_menu: Option<&str>) -> MenuAddress {
    MenuAddress {
        bus_name: ":1.42".into(),
        application_object_path: "/app".into(),
        window_object_path: "/app/window/1".into(),
        menubar_object_path: "/app/menubar".into(),
        app_menu_object_path: app_menu.map(Into::into),
    }
}

#[test]
fn binding_takes_the_servers_version_up_to_22_and_reports_what_it_offers() {
    let (_server, mut conn) = Harness::start(Offer::NONE);
    assert!(
        AppEffects::bind(&mut conn).is_none(),
        "no global, no effects"
    );

    for version in [22, 9] {
        let s = setup(version);
        assert_eq!(
            s.conn.capabilities().extension("cupertino_app_effects_v1"),
            Some(version)
        );
    }

    // session lock (5), disabled controls (4), subsurface backdrops (11), buffers lost (21)
    let table = [
        (1, [false, false, false, false]),
        (4, [false, true, false, false]),
        (5, [true, true, false, false]),
        (11, [true, true, true, false]),
        (20, [true, true, true, false]),
        (21, [true, true, true, true]),
    ];
    for (version, expected) in table {
        let s = setup(version);
        let got = [
            s.effects.tells_session_lock(),
            s.effects.can_set_disabled_controls(),
            s.effects.draws_subsurface_backdrops(),
            s.effects.tells_buffers_lost(),
        ];
        assert_eq!(got, expected, "version {version}");
    }
}

#[test]
fn an_older_server_is_sent_the_nearest_title_bar_style_it_has() {
    use WindowStyle::*;
    for (version, style, granted, wire) in [
        (6, Titled, Titled, "Standard"),
        (6, Unified, Unified, "Unified"),
        (6, Sidebar, Sidebar, "Sidebar"),
        (6, UnifiedCompact, UnifiedCompact, "UnifiedCompact"),
        (5, Sidebar, Titled, "Standard"),
        (5, UnifiedCompact, Unified, "Unified"),
        (5, Unified, Unified, "Unified"),
        (5, Titled, Titled, "Standard"),
        (1, Titled, Titled, "Standard"),
    ] {
        let mut s = setup(version);
        assert_eq!(
            s.effects.granted_style(style),
            granted,
            "version {version} {style:?}"
        );
        s.effects
            .set_window_style(&s.conn, s.surface, style)
            .unwrap();
        let sent = s.sent();
        assert_eq!(sent.len(), 1);
        assert!(
            sent[0].starts_with("SetTitlebarStyle") && sent[0].contains(wire),
            "version {version} {style:?}: {}",
            sent[0]
        );
    }

    let mut s = setup(21);
    s.effects.unify_titlebar(&s.conn, s.surface).unwrap();
    let sent = s.sent();
    assert!(sent[0].contains("Unified"), "{}", sent[0]);
}

#[test]
fn a_destroyed_surface_is_an_error_for_every_request_that_names_it() {
    let mut s = setup(21);
    let gone = window(&mut s.conn).surface();
    s.conn.destroy(gone).unwrap();
    assert!(matches!(
        s.effects.unify_titlebar(&s.conn, gone),
        Err(cupertino_wayland_client::Error::UnknownSurface)
    ));
    assert!(s.effects.backdrop(&s.conn, gone).is_err());
    assert!(s.effects.menu(&s.conn, gone).is_err());
    assert!(s
        .effects
        .set_disabled_controls(&s.conn, gone, WindowControls::default())
        .is_err());
    assert!(s.sent().is_empty());
}

#[test]
fn disabled_controls_go_out_as_bits_from_version_four() {
    let mut s = setup(21);
    let both = WindowControls {
        minimize: true,
        zoom: true,
    };
    s.effects
        .set_disabled_controls(&s.conn, s.surface, both)
        .unwrap();
    s.effects
        .set_disabled_controls(&s.conn, s.surface, WindowControls::default())
        .unwrap();
    let sent = s.sent();
    assert_eq!(sent.len(), 2);
    assert!(
        sent[0].contains("Minimize") && sent[0].contains("Zoom"),
        "{}",
        sent[0]
    );
    assert!(
        !sent[1].contains("Minimize") && !sent[1].contains("Zoom"),
        "{}",
        sent[1]
    );

    let mut old = setup(3);
    old.effects
        .set_disabled_controls(&old.conn, old.surface, both)
        .unwrap();
    assert!(old.sent().is_empty());
}

#[test]
fn a_menu_announcement_needs_version_two_and_sends_its_address() {
    let s = setup(1);
    assert!(s.effects.menu(&s.conn, s.surface).unwrap().is_none());

    let mut s = setup(2);
    let menu = s.effects.menu(&s.conn, s.surface).unwrap().unwrap();
    menu.set_address(&address(None));
    menu.set_address(&address(Some("/app/appmenu")));
    let sent = s.sent();
    assert_eq!(
        sent,
        [
            "SetAddress { bus_name: \":1.42\", application_object_path: \"/app\", window_object_path: \"/app/window/1\", menubar_object_path: \"/app/menubar\", app_menu_object_path: \"\" }",
            "SetAddress { bus_name: \":1.42\", application_object_path: \"/app\", window_object_path: \"/app/window/1\", menubar_object_path: \"/app/menubar\", app_menu_object_path: \"/app/appmenu\" }",
        ]
    );
    drop(menu);
    s.conn.roundtrip().unwrap();
    s.server.wait_for("Destroy", 1);
}

#[test]
fn the_menu_bar_and_reader_events_name_the_surface() {
    let mut s = setup(3);
    let _menu = s.effects.menu(&s.conn, s.surface).unwrap().unwrap();
    s.sent();
    s.server.run(|server| {
        server.menus[0].menu_bar(1);
        server.menus[0].menu_bar(0);
        server.menus[0].menu_reader(":1.7".into());
        server.menus[0].menu_reader(String::new());
    });
    let surface = s.surface;
    let (events, _) = wait_app_events(&mut s.conn, |e| e.len() >= 4);
    assert_eq!(
        events,
        [
            AppEvent::MenuBar {
                surface,
                shown: true
            },
            AppEvent::MenuBar {
                surface,
                shown: false
            },
            AppEvent::MenuReader {
                surface,
                bus_name: Some(":1.7".into())
            },
            AppEvent::MenuReader {
                surface,
                bus_name: None
            },
        ]
    );
}

#[test]
fn a_server_value_outside_its_range_fails_the_connection() {
    type Send = fn(&mut Server);
    let rows: [(&str, Send); 5] = [
        ("a menu reader that is not a unique bus name", |s| {
            s.menus[0].menu_reader("org.example.Menus".into())
        }),
        ("a menu bar flag of 2", |s| s.menus[0].menu_bar(2)),
        ("an appearance of 2", |s| s.appearances[0].appearance(2)),
        ("display options with reserved bits", |s| {
            s.appearances[0].display_options(DisplayOption::from_bits_retain(0x100))
        }),
        ("a session lock of 5", |s| s.appearances[0].session_lock(5)),
    ];
    for (row, send) in rows {
        let mut s = setup(21);
        let _menu = s.effects.menu(&s.conn, s.surface).unwrap().unwrap();
        let _observer = s.effects.appearance(&s.conn);
        s.sent();
        s.server.run(send);
        let error = loop {
            if let Err(e) = s.conn.wait(Some(std::time::Duration::from_millis(100))) {
                break e;
            }
        };
        assert!(
            matches!(error, cupertino_wayland_client::Error::Protocol(_)),
            "{row}: {error}"
        );
    }
}

#[test]
fn a_backdrop_sends_its_shape_in_the_servers_units_its_alpha_and_its_material() {
    let mut s = setup(13);
    let whole = s.backdrop();
    whole.set_shape(&rect(1.4, 2.6, 100.5, 40.4), 12.5).unwrap();
    assert_eq!(
        s.sent(),
        ["SetShape { x: 1, y: 3, width: 101, height: 40, radius: 12.5 }"],
        "whole points before version 14"
    );

    let mut s = setup(14);
    let backdrop = s.backdrop();
    backdrop
        .set_shape(&rect(1.5, 2.25, 100.5, 40.0), 8.0)
        .unwrap();
    // a zero extent hides the backdrop
    backdrop.set_shape(&rect(5.0, 5.0, 0.0, 0.0), 0.0).unwrap();
    backdrop.set_alpha(0.0).unwrap();
    backdrop.set_alpha(1.0).unwrap();
    backdrop.set_material("Menu");
    backdrop.set_glass(GlassStyle::Regular, 1, 0.0).unwrap();
    assert_eq!(
        s.sent(),
        [
            "SetShapeFixed { x: 1.5, y: 2.25, width: 100.5, height: 40.0, radius: 8.0 }",
            "SetShapeFixed { x: 5.0, y: 5.0, width: 0.0, height: 0.0, radius: 0.0 }",
            "SetAlpha { alpha: 0.0 }",
            "SetAlpha { alpha: 1.0 }",
            "SetMaterial { name: \"Menu\" }",
            "SetGlass { style: Value(Regular), container: 1, spacing: 0.0 }",
        ]
    );
}

#[test]
fn a_backdrop_refuses_a_bad_shape_alpha_or_glass_spacing_and_sends_nothing() {
    let mut s = setup(21);
    let backdrop = s.backdrop();
    let mut results = Vec::new();
    for (r, radius) in [
        (rect(0.0, 0.0, -1.0, 10.0), 0.0),
        (rect(0.0, 0.0, 10.0, -1.0), 0.0),
        (rect(0.0, 0.0, 10.0, 10.0), -1.0),
        (rect(f64::NAN, 0.0, 10.0, 10.0), 0.0),
        (rect(0.0, f64::INFINITY, 10.0, 10.0), 0.0),
        (rect(0.0, 0.0, 10.0, 10.0), f64::NAN),
    ] {
        results.push((
            format!("shape {r:?} {radius}"),
            matches!(backdrop.set_shape(&r, radius), Err(Error::InvalidShape)),
        ));
    }
    for alpha in [-0.01, 1.01, f64::NAN, f64::INFINITY] {
        results.push((
            format!("alpha {alpha}"),
            matches!(backdrop.set_alpha(alpha), Err(Error::InvalidAlpha)),
        ));
    }
    for spacing in [-0.5, f64::NAN, f64::INFINITY] {
        results.push((
            format!("spacing {spacing}"),
            matches!(
                backdrop.set_glass(GlassStyle::Regular, 1, spacing),
                Err(Error::InvalidGlass)
            ),
        ));
    }
    for (row, refused) in results {
        assert!(refused, "{row}");
    }
    assert!(s.sent().is_empty());
}

#[test]
fn a_blur_ramp_goes_out_from_version_22_and_a_weight_outside_0_to_1_is_refused() {
    let mut s = setup(22);
    let backdrop = s.backdrop();
    assert!(backdrop.takes_blur_ramp());
    backdrop.set_blur_ramp(1.0, 0.03).unwrap();
    backdrop.set_blur_ramp(0.0, 1.0).unwrap();
    for (top, bottom) in [
        (-0.01, 1.0),
        (1.0, 1.01),
        (f64::NAN, 1.0),
        (1.0, f64::INFINITY),
    ] {
        assert!(
            matches!(backdrop.set_blur_ramp(top, bottom), Err(Error::InvalidRamp)),
            "{top} {bottom}"
        );
    }
    let sent = s.sent();
    // the wire carries 256ths, truncated: the lift of 0.03 goes as 7/256
    assert_eq!(
        sent,
        [
            "SetBlurRamp { top: 1.0, bottom: 0.02734375 }",
            "SetBlurRamp { top: 0.0, bottom: 1.0 }",
        ]
    );

    // an older server blurs evenly and is sent nothing
    let mut s = setup(21);
    let backdrop = s.backdrop();
    assert!(!backdrop.takes_blur_ramp());
    backdrop.set_blur_ramp(1.0, 0.0).unwrap();
    assert!(s.sent().is_empty());
}

/// The style the server receives for `style` at `version`.
fn glass_on(version: u32, style: GlassStyle) -> String {
    let mut s = setup(version);
    let backdrop = s.backdrop();
    backdrop.set_glass(style, 2, 6.0).unwrap();
    let sent = s.sent();
    assert_eq!(sent.len(), 1);
    let sent = &sent[0];
    if version < 6 {
        return sent.clone();
    }
    assert!(sent.ends_with("container: 2, spacing: 6.0 }"), "{sent}");
    sent.trim_start_matches("SetGlass { style: Value(")
        .split(')')
        .next()
        .unwrap()
        .to_owned()
}

#[test]
fn a_glass_style_falls_back_on_servers_that_lack_it() {
    assert_eq!(
        glass_on(5, GlassStyle::Clear),
        format!(
            "SetMaterial {{ name: {:?} }}",
            GlassStyle::Clear.fallback_material()
        ),
        "a server without glass draws a material instead"
    );
    let table = [
        (GlassStyle::Regular, 6, "Regular"),
        (GlassStyle::Clear, 6, "Clear"),
        (GlassStyle::LargePanel, 9, "Regular"),
        (GlassStyle::LargePanel, 10, "LargePanel"),
        (GlassStyle::Bar, 11, "Clear"),
        (GlassStyle::Bar, 12, "Bar"),
        (GlassStyle::Dense, 12, "Regular"),
        (GlassStyle::Dense, 13, "Dense"),
        (GlassStyle::Module, 12, "Regular"),
        (GlassStyle::Module, 13, "Module"),
        (GlassStyle::Card, 12, "Regular"),
        (GlassStyle::Card, 14, "Dense"),
        (GlassStyle::Card, 15, "Card"),
        (GlassStyle::Track, 14, "Clear"),
        (GlassStyle::Track, 15, "Track"),
        (GlassStyle::Popover, 15, "Regular"),
        (GlassStyle::Popover, 16, "Popover"),
        (GlassStyle::Switcher, 16, "Regular"),
        (GlassStyle::Switcher, 17, "Switcher"),
        (GlassStyle::Sidebar, 17, "Regular"),
        (GlassStyle::Sidebar, 18, "Sidebar"),
        (GlassStyle::Snippet, 17, "Regular"),
        (GlassStyle::Snippet, 18, "Snippet"),
        (GlassStyle::Veiled, 17, "Clear"),
        (GlassStyle::Veiled, 18, "Veiled"),
        (GlassStyle::SubduedModule, 12, "Regular"),
        (GlassStyle::SubduedModule, 13, "Module"),
        (GlassStyle::SubduedModule, 17, "Module"),
        (GlassStyle::SubduedModule, 18, "SubduedModule"),
        (GlassStyle::Toolbar, 18, "Regular"),
        (GlassStyle::Toolbar, 19, "Toolbar"),
        (GlassStyle::Menu, 12, "Regular"),
        (GlassStyle::Menu, 13, "Dense"),
        (GlassStyle::Menu, 19, "Dense"),
        (GlassStyle::Menu, 20, "Menu"),
    ];
    for (style, version, wire) in table {
        assert_eq!(glass_on(version, style), wire, "{style:?} at {version}");
    }
}

#[test]
fn backdrops_have_distinct_ids_tell_the_tone_behind_them_from_version_seven_and_go_when_dropped() {
    assert!(!setup(6).backdrop().tells_tone());
    let mut s = setup(7);
    let backdrop = s.backdrop();
    let other = s.backdrop();
    assert_ne!(backdrop.id(), other.id());
    assert_ne!(backdrop.id().0, 0);
    assert!(backdrop.tells_tone());
    s.sent();
    s.server.run(|server| {
        server.backdrops[0].tone(Tone::Dark);
        server.backdrops[0].tone(Tone::Light);
        server.backdrops[0].tone(Tone::Appearance);
    });
    let id = backdrop.id();
    let (events, _) = wait_app_events(&mut s.conn, |e| e.len() >= 3);
    let tone = |tone| AppEvent::BackdropTone { backdrop: id, tone };
    assert_eq!(
        events,
        [
            tone(BackdropTone::Dark),
            tone(BackdropTone::Light),
            tone(BackdropTone::Appearance)
        ]
    );

    drop(other);
    s.sent();
    s.server.wait_for("Destroy", 1);
}

#[test]
fn the_appearance_observer_reports_appearance_display_options_session_lock_and_lost_buffers() {
    let mut s = setup(21);
    let _observer = s.effects.appearance(&s.conn);
    s.sent();
    s.server.run(|server| {
        let appearance = &server.appearances[0];
        appearance.appearance(1);
        appearance.appearance(0);
        appearance.display_options(DisplayOption::ReduceMotion | DisplayOption::IncreaseContrast);
        appearance.session_lock(1);
        appearance.session_lock(0);
        appearance.buffers_lost();
    });
    let (events, _) = wait_app_events(&mut s.conn, |e| e.len() >= 8);
    assert_eq!(
        events,
        [
            AppEvent::Appearance { dark: true },
            AppEvent::Appearance { dark: false },
            AppEvent::DisplayOptions(DisplayOptions {
                reduce_motion: true,
                reduce_transparency: false,
                increase_contrast: true,
                differentiate_without_color: false,
            }),
            AppEvent::ScrollBars { always: false },
            AppEvent::SteadyCaret { on: false },
            AppEvent::SessionLock { locked: true },
            AppEvent::SessionLock { locked: false },
            AppEvent::BuffersLost
        ]
    );
}

#[test]
fn scroll_bar_and_caret_options_follow_their_versions() {
    for (version, expected) in [(7, 1), (8, 2), (9, 3)] {
        let mut s = setup(version);
        let _observer = s.effects.appearance(&s.conn);
        s.sent();
        s.server.run(|server| {
            server.appearances[0]
                .display_options(DisplayOption::AlwaysShowScrollBars | DisplayOption::SteadyCaret);
        });
        let (events, _) = wait_app_events(&mut s.conn, |e| !e.is_empty());
        // the events of one change arrive together, so wait for the transport to hand them all over
        assert_eq!(events.len(), expected, "version {version}: {events:?}");
        if version >= 8 {
            assert!(events.contains(&AppEvent::ScrollBars { always: true }));
        }
        if version >= 9 {
            assert!(events.contains(&AppEvent::SteadyCaret { on: true }));
        }
    }
}
