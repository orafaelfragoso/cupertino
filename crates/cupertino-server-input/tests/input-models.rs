//! Public-boundary tests for `cupertino-server-input` models: key bindings & router, keyboard
//! settings and input sources, SuperTap and key filters, hung-client liveness, pointer constraints,
//! shortcut-inhibit notices, global shortcuts helpers, accent picker UI, localed layout parsing,
//! and pointer/axis event models.

use cupertino_geometry::Rect as GRect;
use cupertino_keyboard::accent::AccentTable;
use cupertino_server_input::events::{
    axis_index, AxisCarry, AxisInput, MouseIdentity, PointerAccelSettings, PointerDevice, BTN_LEFT,
    BTN_RIGHT,
};
use cupertino_server_input::global_shortcuts::{
    is_modifier_key, modifier_bits, modifiers_of, reserved,
};
use cupertino_server_input::hung::{Liveness, PING_INTERVAL, PONG_TIMEOUT};
use cupertino_server_input::input::{
    parse_chord, ActionContext, Chord, KeyBindings, KeyEvent, KeyOutcome, KeyRouter, Mods,
};
use cupertino_server_input::kbd::{AccentSettings, InputSources, KeyboardSettings, SourceSpec};
use cupertino_server_input::kbd_filters::{KeyFilters, SuperTap};
use cupertino_server_input::kbd_ui::{
    accent_layout, has_input_source_bindings, source_code, AccentUi, TextOp,
};
use cupertino_server_input::localed::SystemLayout;
use cupertino_server_input::overlay_types::{is_modifier_sym, OverlayKey};
use cupertino_server_input::pointer_constraints::confine;
use cupertino_server_input::shortcut_inhibit::{chord_label, notice_name, notice_text};
use smithay::backend::input::{Axis, AxisRelativeDirection, AxisSource};
use smithay::input::keyboard::keysyms;
use smithay::utils::{Point, Rectangle};

#[test]
fn key_router_matches_bindings_and_honours_inhibit() {
    let mut bindings = KeyBindings::defaults();
    assert!(!has_input_source_bindings(&bindings));
    bindings.bind(
        parse_chord("Ctrl+space").expect("Ctrl+space parses"),
        cupertino_server_input::input::Action::InputSourceNext,
    );
    assert!(has_input_source_bindings(&bindings));
    let mut router = KeyRouter::new(bindings);
    let chord = parse_chord("Super+q").expect("Super+q parses");
    assert_eq!(chord_label(&chord), "Super+q");

    let ctx = ActionContext::default();
    let press = KeyEvent {
        sym: keysyms::KEY_q,
        mods: Mods {
            logo: true,
            ..Mods::NONE
        },
        pressed: true,
        repeat: false,
        shifted: None,
        alt_gr: false,
    };
    assert!(matches!(router.key(press, &ctx), KeyOutcome::Consumed(_)));

    // Release of a consumed chord is swallowed as a consumed empty plan.
    let release = KeyEvent {
        pressed: false,
        ..press
    };
    assert!(matches!(router.key(release, &ctx), KeyOutcome::Consumed(_)));

    // While inhibited, ordinary bindings forward to the focused client.
    let inhibited = ActionContext {
        input_held: true,
        ..ActionContext::default()
    };
    assert!(matches!(router.key(press, &inhibited), KeyOutcome::Forward));
}

#[test]
fn global_shortcuts_modifiers_and_reserved() {
    let mods = Mods {
        shift: true,
        ctrl: true,
        alt: false,
        logo: true,
    };
    let bits = modifier_bits(mods);
    assert_eq!(modifiers_of(bits.bits()), mods);
    assert!(is_modifier_key(keysyms::KEY_Shift_L));
    assert!(is_modifier_key(keysyms::KEY_Caps_Lock));
    assert!(!is_modifier_key(keysyms::KEY_a));
    assert!(is_modifier_sym(keysyms::KEY_Control_L));
    assert!(!is_modifier_sym(keysyms::KEY_a));

    let res = reserved(&KeyBindings::default());
    // Must contain Ctrl+Alt+F1 and Ctrl+Alt+Escape without duplicates.
    let escape = Chord {
        mods: Mods {
            ctrl: true,
            alt: true,
            ..Mods::NONE
        },
        sym: keysyms::KEY_Escape,
    };
    assert_eq!(res.iter().filter(|c| **c == escape).count(), 1);
}

#[test]
fn localed_and_input_sources_and_super_tap() {
    let layout = SystemLayout {
        layout: "us, br".into(),
        variant: ", abnt2".into(),
        options: "ctrl:nocaps".into(),
    };
    let specs = layout.sources();
    assert_eq!(specs.len(), 2);
    assert_eq!(specs[0], SourceSpec::parse("us").unwrap());
    assert_eq!(specs[1], SourceSpec::parse("br(abnt2)").unwrap());

    let mut sources = InputSources::new(specs);
    let ev = sources.press(false, cupertino_keyboard::switcher::Step::Next, false);
    assert_eq!(
        sources.apply(ev, 1.0, |id| id.to_owned()).as_deref(),
        Some("br(abnt2)")
    );
    assert!(!sources.hud_open());

    let mut tap = SuperTap::default();
    assert!(!tap.key(125, true, 100, false));
    assert!(tap.key(125, false, 300, false));
    // Disturbed by pointer click cancels tap.
    assert!(!tap.key(125, true, 1000, false));
    tap.disturb();
    assert!(!tap.key(125, false, 1100, false));

    let mut filters = KeyFilters::new(&KeyboardSettings::default());
    filters.pass_unfiltered(42);
    assert!(filters.unfiltered.contains(&42));
}

#[test]
fn hung_liveness_ping_timeout_and_recovery() {
    let mut live = Liveness::default();
    assert!(!live.ping_due(1.0, None));
    assert!(live.ping_due(1.0, Some(1.0)));
    live.pinged(1.0);
    assert!(live.awaiting_pong());
    assert_eq!(live.next_deadline(Some(1.0)), Some(1.0 + PONG_TIMEOUT));
    assert!(!live.check(1.0 + PONG_TIMEOUT - 0.1));
    assert!(live.check(1.0 + PONG_TIMEOUT));
    assert!(live.is_unresponsive());
    assert!(live.ponged());
    assert!(!live.is_unresponsive());
    assert_eq!(live.next_deadline(Some(1.0)), Some(1.0 + PING_INTERVAL));
}

#[test]
fn pointer_confine_and_shortcut_inhibit_notice() {
    let rect = Rectangle::new((10, 10).into(), (100, 100).into());
    let from = Point::from((50.0, 50.0));
    let out = Point::from((250.0, 60.0));
    let clamped = confine(from, out, &[rect], |p| {
        p.x >= 10.0 && p.x <= 109.0 && p.y >= 10.0 && p.y <= 109.0
    });
    assert_eq!(clamped, Point::from((109.0, 60.0)));
    assert_eq!(confine(from, out, &[], |_| false), from);

    assert_eq!(notice_name("\u{202e}Bad\nApp\x07"), "BadApp");
    assert_eq!(notice_name("   "), "An application");
    let long = "A".repeat(64);
    let trimmed = notice_name(&long);
    assert!(trimmed.ends_with('…'));
    assert_eq!(trimmed.chars().count(), 48);

    let (clause, text) = notice_text(Some("Ctrl+Alt+Escape"), "VM", true, true);
    assert!(clause.contains("Ctrl+Alt+Escape"));
    assert!(text.contains("VM"));
}

#[test]
fn accent_ui_and_source_code_and_pointer_events() {
    assert_eq!(source_code("fr(nodeadkeys)"), "FR");
    let table = AccentTable::from_json(
        r#"{"en":{"a":{"direction":"right","strings":["à","á"]}}}"#,
        &["en"],
    )
    .unwrap();
    let mut ui = AccentUi::with_table(&AccentSettings::default(), table);
    assert!(ui.has_table());
    let anchor = GRect::new(100.0, 100.0, 2.0, 18.0);
    assert!(ui.open_for('a', anchor, "org.test.Editor"));
    assert!(ui.is_open());
    let popup = ui.popup.as_ref().unwrap();
    let layout = accent_layout(popup, GRect::new(0.0, 0.0, 1440.0, 900.0), 28.0);
    assert_eq!(layout.cells.len(), 2);

    // Selecting candidate 2 commits "á".
    let key = OverlayKey {
        code: 11,
        sym: keysyms::KEY_2,
        text: Some('2'),
        mods: Mods::NONE,
        alt_gr: false,
    };
    assert!(ui.key(&key, "org.test.Editor"));
    assert!(!ui.is_open());
    assert!(ui
        .outbox
        .iter()
        .any(|op| matches!(op, TextOp::Commit { text, .. } if text == "á")));

    assert_eq!(BTN_LEFT, 0x110);
    assert_eq!(BTN_RIGHT, 0x111);
    assert_eq!(axis_index(Axis::Horizontal), 0);
    assert_eq!(axis_index(Axis::Vertical), 1);

    let cfg = PointerAccelSettings::default();
    let mut mouse = PointerDevice::with_identity(
        false,
        &cfg,
        MouseIdentity {
            key: Some("046d:c077:Logitech".into()),
            name: "Logitech".into(),
            hwdb_dpi: Some("1600@1000".into()),
        },
    );
    assert!(!mouse.is_used_mouse());
    assert!(mouse.note_use());
    assert!(!mouse.note_use());
    assert!(mouse.is_used_mouse());
    let entry = mouse.mouse_entry(&cfg).expect("used mouse with key");
    assert_eq!(entry.dpi, 1600);
    assert!(!entry.stored);

    let delta = mouse.accelerate(Point::from((10.0, 0.0)), 10_000);
    assert!(delta.x > 0.0);

    let stop = AxisInput {
        device: "pad".into(),
        source: AxisSource::Finger,
        usec: 1000,
        amount: [None, Some(0.0)],
        v120: [None, None],
        relative_direction: [AxisRelativeDirection::Identical; 2],
    };
    assert_eq!(stop.stopped_axes(), [false, true]);
    assert!(stop.ends_gesture());

    let mut carry = AxisCarry::default();
    let mut total = 0.0;
    for _ in 0..8 {
        total += carry.send(1, AxisSource::Wheel, 1.0 / 512.0);
    }
    assert!((total - 8.0 / 512.0).abs() < 1e-9);
    carry.stop(1);
    // Reversal resets carry.
    let rev = carry.send(1, AxisSource::Wheel, -1.0);
    assert_eq!(rev, -1.0);
}
