//! The accent picker state machine, its table loader and the popup layout.

use cupertino_keyboard::accent::{
    layout_popup, AccentEvent, AccentPicker, AccentTable, Direction, KeyDown, PickerKey,
    PopupMetrics, Rect,
};
use cupertino_keyboard::modifiers::Mods;
use cupertino_keyboard::AccentConfig;

fn table() -> AccentTable {
    let mut t = AccentTable::new();
    t.insert(
        'e',
        Direction::Right,
        ["e", "é", "è", "ê"].iter().map(|s| s.to_string()).collect(),
    );
    t.insert('1', Direction::Right, vec!["¹".to_string()]);
    t
}

fn picker(cfg: AccentConfig) -> AccentPicker {
    AccentPicker::new(cfg, table())
}

fn held(ch: char) -> KeyDown {
    KeyDown {
        ch: Some(ch),
        key: PickerKey::Char,
        repeat: true,
        mods: Mods::NONE,
    }
}

fn key(k: PickerKey) -> KeyDown {
    KeyDown {
        ch: None,
        key: k,
        repeat: false,
        mods: Mods::NONE,
    }
}

fn digit(ch: char) -> KeyDown {
    KeyDown {
        ch: Some(ch),
        ..key(PickerKey::Char)
    }
}

/// A picker open on a held `e`.
fn open(cfg: AccentConfig) -> AccentPicker {
    let mut p = picker(cfg);
    assert!(matches!(
        p.key_down(&held('e'), "app"),
        AccentEvent::Show { .. }
    ));
    p
}

fn focus(index: usize, preview: Option<&str>) -> AccentEvent {
    AccentEvent::Focus {
        index,
        preview: preview.map(String::from),
    }
}

fn commit(text: &str) -> AccentEvent {
    AccentEvent::Commit { text: text.into() }
}

fn cancel(restore: Option<&str>) -> AccentEvent {
    AccentEvent::Cancel {
        restore: restore.map(String::from),
    }
}

#[test]
fn the_picker_opens_on_the_first_auto_repeat_of_a_character_with_alternatives() {
    let mut p = picker(AccentConfig::default());
    assert_eq!(
        p.key_down(&held('e'), "app"),
        AccentEvent::Show {
            base: 'e',
            candidates: vec!["é".into(), "è".into(), "ê".into()],
            focused: None
        },
        "the base character is left out by default"
    );
    assert!(p.is_open());

    let kept = open(AccentConfig {
        remove_base: false,
        ..AccentConfig::default()
    });
    assert_eq!(kept.candidates(), ["e", "é", "è", "ê"], "base kept first");
    let marked = open(AccentConfig {
        mark_on_open: true,
        ..AccentConfig::default()
    });
    assert_eq!(marked.focused(), Some(0), "marking focuses the first");
}

#[test]
fn the_picker_stays_closed_when_it_should_not_open() {
    let first_press = KeyDown {
        repeat: false,
        ..held('e')
    };
    let ctrl = KeyDown {
        mods: Mods::CTRL,
        ..held('e')
    };
    let primary = KeyDown {
        mods: Mods::PRIMARY,
        ..held('e')
    };
    let off = AccentConfig {
        enabled: false,
        ..AccentConfig::default()
    };
    let per_app = AccentConfig {
        disabled_apps: vec!["term".into()],
        ..AccentConfig::default()
    };
    for (row, cfg, ev, app) in [
        (
            "a first press is not a repeat",
            AccentConfig::default(),
            first_press,
            "app",
        ),
        ("Ctrl held", AccentConfig::default(), ctrl, "app"),
        ("Primary held", AccentConfig::default(), primary, "app"),
        ("a digit", AccentConfig::default(), held('1'), "app"),
        ("no alternatives", AccentConfig::default(), held('x'), "app"),
        ("switched off", off, held('e'), "app"),
        ("disabled for the application", per_app, held('e'), "term"),
    ] {
        let mut p = picker(cfg);
        assert_eq!(p.key_down(&ev, app), AccentEvent::PassThrough, "{row}");
        assert!(!p.is_open(), "{row}");
    }
    let p = picker(AccentConfig {
        disabled_apps: vec!["term".into()],
        ..AccentConfig::default()
    });
    assert!(!p.allowed_for("term") && p.allowed_for("app"));
    assert!(p.offers('e') && !p.offers('1') && !p.offers('x'));
}

#[test]
fn tab_and_arrows_move_the_focus_and_stop_or_wrap_at_the_ends() {
    let mut p = open(AccentConfig::default());
    for (k, event) in [
        (PickerKey::Tab, focus(0, Some("é"))),
        (PickerKey::Right, focus(1, Some("è"))),
        (PickerKey::Left, focus(0, Some("é"))),
    ] {
        assert_eq!(p.key_down(&key(k), "app"), event, "{k:?}");
    }
    for _ in 0..5 {
        p.key_down(&key(PickerKey::Right), "app");
    }
    assert_eq!(p.focused(), Some(2), "the focus stops at the end");

    let mut w = open(AccentConfig {
        wrap: true,
        ..AccentConfig::default()
    });
    for _ in 0..4 {
        w.key_down(&key(PickerKey::Right), "app");
    }
    assert_eq!(w.focused(), Some(0), "wrapping returns to the start");

    let mut quiet = open(AccentConfig {
        preview_focused: false,
        ..AccentConfig::default()
    });
    assert_eq!(
        quiet.key_down(&key(PickerKey::Tab), "app"),
        focus(0, None),
        "no preview when it is off"
    );
}

#[test]
fn the_open_picker_ends_by_key_click_or_dismissal() {
    type Act = fn(&mut AccentPicker) -> AccentEvent;
    fn tab_then(p: &mut AccentPicker, k: PickerKey) -> AccentEvent {
        p.key_down(&key(PickerKey::Tab), "app");
        p.key_down(&key(k), "app")
    }
    let rows: [(&str, Act, AccentEvent, bool); 9] = [
        (
            "Return commits the focused candidate",
            |p| tab_then(p, PickerKey::Return),
            commit("é"),
            false,
        ),
        (
            "Return with nothing focused cancels",
            |p| p.key_down(&key(PickerKey::Return), "app"),
            cancel(None),
            false,
        ),
        (
            "Escape after a preview restores the character",
            |p| tab_then(p, PickerKey::Escape),
            cancel(Some("e")),
            false,
        ),
        (
            "Escape without a preview restores nothing",
            |p| p.key_down(&key(PickerKey::Escape), "app"),
            cancel(None),
            false,
        ),
        (
            "a digit commits the nth candidate",
            |p| p.key_down(&digit('2'), "app"),
            commit("è"),
            false,
        ),
        (
            "a digit past the candidates is consumed",
            |p| p.key_down(&digit('9'), "app"),
            AccentEvent::Consumed,
            true,
        ),
        (
            "another key closes and passes through",
            |p| p.key_down(&key(PickerKey::Other), "app"),
            AccentEvent::PassThrough,
            false,
        ),
        (
            "a click commits its candidate",
            |p| p.click(1),
            commit("è"),
            false,
        ),
        (
            "a click past the candidates cancels",
            |p| p.click(9),
            cancel(None),
            false,
        ),
    ];
    for (row, act, event, stays_open) in rows {
        let mut p = open(AccentConfig::default());
        assert_eq!(act(&mut p), event, "{row}");
        assert_eq!(p.is_open(), stays_open, "{row}");
    }
    let mut p = open(AccentConfig::default());
    assert_eq!(p.dismiss(), cancel(None));
    assert_eq!(p.dismiss(), AccentEvent::PassThrough, "already closed");
    assert_eq!(p.click(0), AccentEvent::PassThrough, "already closed");
}

const TABLE_JSON: &str = r#"{
    "default": {"e": {"direction": "right", "strings": ["e", "é"]}},
    "en": {"a": {"direction": "left", "strings": ["à", ""]},
           "e": {"direction": "strictly-left", "strings": ["ë"]},
           "ab": {"strings": ["x"]}}
}"#;

#[test]
fn the_table_loads_the_named_keyboards_with_later_names_overriding() {
    let t = AccentTable::from_json(TABLE_JSON, &["default", "en"]).unwrap();
    assert_eq!(t.len(), 2, "the multi-character key is skipped");
    let e = t.entry('e').unwrap();
    assert_eq!(
        (e.direction, e.strings.as_slice()),
        (Direction::StrictlyLeft, &["ë".to_string()][..])
    );
    let a = t.entry('a').unwrap();
    assert_eq!(
        (a.direction, a.strings.as_slice()),
        (Direction::Left, &["à".to_string()][..]),
        "empty strings are dropped"
    );

    let only_default = AccentTable::from_json(TABLE_JSON, &["default", "fr"]).unwrap();
    assert_eq!(only_default.len(), 1, "unnamed keyboards are skipped");
    assert_eq!(only_default.entry('e').unwrap().direction, Direction::Right);
    assert!(AccentTable::from_json(TABLE_JSON, &[]).unwrap().is_empty());
    assert!(AccentTable::from_json("[", &["default"]).is_err());
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { x, y, w, h }
}

#[test]
fn the_popup_is_centred_above_the_anchor_in_a_row_of_cells() {
    let m = PopupMetrics::default();
    let l = layout_popup(
        3,
        rect(500.0, 400.0, 10.0, 20.0),
        rect(0.0, 0.0, 1000.0, 800.0),
        &m,
    );
    assert!(l.above);
    assert_eq!(l.cells.len(), 3);
    assert_eq!(l.rect.w, 3.0 * m.cell_w + 2.0 * m.padding);
    assert_eq!(l.rect.h, m.cell_h + 2.0 * m.padding);
    assert_eq!(l.rect.x + l.rect.w / 2.0, 505.0);
    assert_eq!(l.rect.y + l.rect.h, 400.0 - m.gap_to_anchor);
    assert_eq!(l.cells[0].x, l.rect.x + m.padding);
    assert_eq!(l.cells[1].x - l.cells[0].x, m.cell_w);
}

#[test]
fn the_popup_flips_below_and_stays_inside_the_screen() {
    let m = PopupMetrics::default();
    let screen = rect(0.0, 0.0, 1000.0, 800.0);
    let below = layout_popup(3, rect(500.0, 10.0, 10.0, 20.0), screen, &m);
    assert!(!below.above, "no room above");
    assert_eq!(below.rect.y, 30.0 + m.gap_to_anchor);
    let left = layout_popup(3, rect(0.0, 400.0, 10.0, 20.0), screen, &m);
    assert_eq!(left.rect.x, m.screen_margin);
    let right = layout_popup(3, rect(995.0, 400.0, 10.0, 20.0), screen, &m);
    assert_eq!(right.rect.x + right.rect.w, 1000.0 - m.screen_margin);

    let narrow = rect(0.0, 0.0, 200.0, 800.0);
    let l = layout_popup(50, rect(100.0, 400.0, 10.0, 20.0), narrow, &m);
    let fit = ((200.0 - 2.0 * m.screen_margin - 2.0 * m.padding) / m.cell_w).floor() as usize;
    assert_eq!(
        l.cells.len(),
        fit,
        "a row wider than the screen is truncated"
    );
    assert!(l.rect.x >= m.screen_margin);
    assert!(l.rect.x + l.rect.w <= 200.0 - m.screen_margin);
}
