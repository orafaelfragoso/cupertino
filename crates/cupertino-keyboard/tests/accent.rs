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

fn open(p: &mut AccentPicker) {
    assert!(matches!(
        p.key_down(&held('e'), "app"),
        AccentEvent::Show { .. }
    ));
}

#[test]
fn the_picker_opens_on_the_first_auto_repeat_without_the_base_character() {
    let mut p = picker(AccentConfig::default());
    assert_eq!(
        p.key_down(&held('e'), "app"),
        AccentEvent::Show {
            base: 'e',
            candidates: vec!["é".into(), "è".into(), "ê".into()],
            focused: None
        }
    );
    assert!(p.is_open());
    assert_eq!(p.candidates().len(), 3);
}

#[test]
fn a_first_press_that_is_not_a_repeat_passes_through() {
    let mut p = picker(AccentConfig::default());
    let mut ev = held('e');
    ev.repeat = false;
    assert_eq!(p.key_down(&ev, "app"), AccentEvent::PassThrough);
    assert!(!p.is_open());
}

#[test]
fn control_and_primary_modifiers_prevent_opening() {
    let mut p = picker(AccentConfig::default());
    for mods in [Mods::CTRL, Mods::PRIMARY] {
        let mut ev = held('e');
        ev.mods = mods;
        assert_eq!(p.key_down(&ev, "app"), AccentEvent::PassThrough);
    }
}

#[test]
fn digits_and_characters_without_alternatives_do_not_open() {
    let mut p = picker(AccentConfig::default());
    assert_eq!(p.key_down(&held('1'), "app"), AccentEvent::PassThrough);
    assert_eq!(p.key_down(&held('x'), "app"), AccentEvent::PassThrough);
    assert!(p.offers('e'));
    assert!(!p.offers('1'));
    assert!(!p.offers('x'));
}

#[test]
fn the_global_switch_and_the_per_application_list_disable_the_picker() {
    let off = AccentConfig {
        enabled: false,
        ..AccentConfig::default()
    };
    assert_eq!(
        picker(off).key_down(&held('e'), "app"),
        AccentEvent::PassThrough
    );
    let cfg = AccentConfig {
        disabled_apps: vec!["term".into()],
        ..AccentConfig::default()
    };
    let mut p = picker(cfg);
    assert!(!p.allowed_for("term"));
    assert!(p.allowed_for("app"));
    assert_eq!(p.key_down(&held('e'), "term"), AccentEvent::PassThrough);
}

#[test]
fn keeping_the_base_character_lists_it_first() {
    let cfg = AccentConfig {
        remove_base: false,
        ..AccentConfig::default()
    };
    let mut p = picker(cfg);
    open(&mut p);
    assert_eq!(p.candidates()[0], "e");
    assert_eq!(p.candidates().len(), 4);
}

#[test]
fn marking_on_open_focuses_the_first_candidate() {
    let cfg = AccentConfig {
        mark_on_open: true,
        ..AccentConfig::default()
    };
    let mut p = picker(cfg);
    open(&mut p);
    assert_eq!(p.focused(), Some(0));
}

#[test]
fn tab_and_arrows_move_the_focus_with_a_preview() {
    let mut p = picker(AccentConfig::default());
    open(&mut p);
    assert_eq!(
        p.key_down(&key(PickerKey::Tab), "app"),
        AccentEvent::Focus {
            index: 0,
            preview: Some("é".into())
        }
    );
    assert_eq!(
        p.key_down(&key(PickerKey::Right), "app"),
        AccentEvent::Focus {
            index: 1,
            preview: Some("è".into())
        }
    );
    assert_eq!(
        p.key_down(&key(PickerKey::Left), "app"),
        AccentEvent::Focus {
            index: 0,
            preview: Some("é".into())
        }
    );
}

#[test]
fn the_focus_stops_at_the_ends_unless_wrapping_is_on() {
    let mut p = picker(AccentConfig::default());
    open(&mut p);
    for _ in 0..5 {
        p.key_down(&key(PickerKey::Right), "app");
    }
    assert_eq!(p.focused(), Some(2));
    let mut w = picker(AccentConfig {
        wrap: true,
        ..AccentConfig::default()
    });
    open(&mut w);
    for _ in 0..4 {
        w.key_down(&key(PickerKey::Right), "app");
    }
    assert_eq!(w.focused(), Some(0));
}

#[test]
fn disabling_the_preview_leaves_it_out_of_focus_events() {
    let mut p = picker(AccentConfig {
        preview_focused: false,
        ..AccentConfig::default()
    });
    open(&mut p);
    assert_eq!(
        p.key_down(&key(PickerKey::Tab), "app"),
        AccentEvent::Focus {
            index: 0,
            preview: None
        }
    );
}

#[test]
fn return_commits_the_focused_candidate() {
    let mut p = picker(AccentConfig::default());
    open(&mut p);
    p.key_down(&key(PickerKey::Tab), "app");
    assert_eq!(
        p.key_down(&key(PickerKey::Return), "app"),
        AccentEvent::Commit { text: "é".into() }
    );
    assert!(!p.is_open());
}

#[test]
fn return_without_a_focused_candidate_cancels() {
    let mut p = picker(AccentConfig::default());
    open(&mut p);
    assert_eq!(
        p.key_down(&key(PickerKey::Return), "app"),
        AccentEvent::Cancel { restore: None }
    );
}

#[test]
fn escape_restores_the_original_character_only_when_a_preview_showed() {
    let mut p = picker(AccentConfig::default());
    open(&mut p);
    p.key_down(&key(PickerKey::Tab), "app");
    assert_eq!(
        p.key_down(&key(PickerKey::Escape), "app"),
        AccentEvent::Cancel {
            restore: Some("e".into())
        }
    );
    open(&mut p);
    assert_eq!(
        p.key_down(&key(PickerKey::Escape), "app"),
        AccentEvent::Cancel { restore: None }
    );
}

#[test]
fn a_digit_commits_the_nth_candidate() {
    let mut p = picker(AccentConfig::default());
    open(&mut p);
    let mut ev = key(PickerKey::Char);
    ev.ch = Some('2');
    assert_eq!(
        p.key_down(&ev, "app"),
        AccentEvent::Commit { text: "è".into() }
    );
    assert!(!p.is_open());
}

#[test]
fn a_digit_beyond_the_candidates_is_consumed_and_keeps_the_popup() {
    let mut p = picker(AccentConfig::default());
    open(&mut p);
    let mut ev = key(PickerKey::Char);
    ev.ch = Some('9');
    assert_eq!(p.key_down(&ev, "app"), AccentEvent::Consumed);
    assert!(p.is_open());
}

#[test]
fn any_other_key_closes_the_popup_and_passes_through() {
    let mut p = picker(AccentConfig::default());
    open(&mut p);
    assert_eq!(
        p.key_down(&key(PickerKey::Other), "app"),
        AccentEvent::PassThrough
    );
    assert!(!p.is_open());
}

#[test]
fn a_click_commits_and_dismiss_cancels() {
    let mut p = picker(AccentConfig::default());
    open(&mut p);
    assert_eq!(p.click(1), AccentEvent::Commit { text: "è".into() });
    open(&mut p);
    assert_eq!(p.click(9), AccentEvent::Cancel { restore: None });
    open(&mut p);
    assert_eq!(p.dismiss(), AccentEvent::Cancel { restore: None });
    assert_eq!(p.dismiss(), AccentEvent::PassThrough);
    assert_eq!(p.click(0), AccentEvent::PassThrough);
}

const TABLE_JSON: &str = r#"{
    "default": {"e": {"direction": "right", "strings": ["e", "é"]}},
    "en": {"a": {"direction": "left", "strings": ["à", ""]},
           "e": {"direction": "strictly-left", "strings": ["ë"]},
           "ab": {"strings": ["x"]}}
}"#;

#[test]
fn the_table_loads_the_named_keyboards_and_later_names_override() {
    let t = AccentTable::from_json(TABLE_JSON, &["default", "en"]).unwrap();
    assert_eq!(t.len(), 2);
    let e = t.entry('e').unwrap();
    assert_eq!(e.direction, Direction::StrictlyLeft);
    assert_eq!(e.strings, vec!["ë".to_string()]);
    let a = t.entry('a').unwrap();
    assert_eq!(a.direction, Direction::Left);
    assert_eq!(a.strings, vec!["à".to_string()]);
}

#[test]
fn the_table_skips_unnamed_keyboards_and_multi_character_keys() {
    let t = AccentTable::from_json(TABLE_JSON, &["default", "fr"]).unwrap();
    assert_eq!(t.len(), 1);
    assert_eq!(t.entry('e').unwrap().direction, Direction::Right);
    assert!(AccentTable::from_json(TABLE_JSON, &[]).unwrap().is_empty());
}

#[test]
fn malformed_table_text_is_an_error() {
    assert!(AccentTable::from_json("[", &["default"]).is_err());
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { x, y, w, h }
}

#[test]
fn the_popup_is_centred_above_the_anchor() {
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
fn the_popup_flips_below_when_there_is_no_room_above() {
    let m = PopupMetrics::default();
    let anchor = rect(500.0, 10.0, 10.0, 20.0);
    let l = layout_popup(3, anchor, rect(0.0, 0.0, 1000.0, 800.0), &m);
    assert!(!l.above);
    assert_eq!(l.rect.y, 30.0 + m.gap_to_anchor);
}

#[test]
fn the_popup_stays_inside_the_screen_margin() {
    let m = PopupMetrics::default();
    let screen = rect(0.0, 0.0, 1000.0, 800.0);
    let left = layout_popup(3, rect(0.0, 400.0, 10.0, 20.0), screen, &m);
    assert_eq!(left.rect.x, m.screen_margin);
    let right = layout_popup(3, rect(995.0, 400.0, 10.0, 20.0), screen, &m);
    assert_eq!(right.rect.x + right.rect.w, 1000.0 - m.screen_margin);
}

#[test]
fn a_row_wider_than_the_screen_is_truncated_to_the_cells_that_fit() {
    let m = PopupMetrics::default();
    let l = layout_popup(
        50,
        rect(100.0, 400.0, 10.0, 20.0),
        rect(0.0, 0.0, 200.0, 800.0),
        &m,
    );
    let fit = ((200.0 - 2.0 * m.screen_margin - 2.0 * m.padding) / m.cell_w).floor() as usize;
    assert_eq!(l.cells.len(), fit);
    assert!(l.rect.x >= m.screen_margin);
    assert!(l.rect.x + l.rect.w <= 200.0 - m.screen_margin);
}
