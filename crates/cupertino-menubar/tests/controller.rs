use cupertino_menubar::controller::{
    point_in_triangle, Effect, Key, MenuController, Mode, APP_MENU_ROOT,
};
use cupertino_menubar::extras::{Extras, FrontApp, SystemRow};
use cupertino_menubar::fallback::{
    self, Chords, FallbackAction, HELP_MENU, HELP_SEARCH, WINDOW_MENU,
};
use cupertino_menubar::geom::{Point, Rect};
use cupertino_menubar::item::{ItemId, MenuItem, Shortcut, Toggle};
use cupertino_menubar::layout::{BarContent, MonoMeasure, SlotKey, StatusEntry};
use cupertino_menubar::recent::RecentItems;
use cupertino_ui_text::{KeyInput, Keymap};
use std::time::Instant;

const OUT: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);

fn content() -> BarContent {
    BarContent {
        app_name: "Files".into(),
        system_menu: vec![],
        app_menu: vec![
            MenuItem::new(100, "About Files"),
            MenuItem::separator(101),
            MenuItem::new(102, "Hide Files"),
            MenuItem::new(103, "Quit Files"),
        ],
        menus: vec![
            MenuItem::submenu(
                1,
                "File",
                vec![
                    MenuItem::new(10, "New Window")
                        .with_shortcut(Shortcut::from_tokens(&["Control", "N"]).unwrap()),
                    MenuItem::submenu(
                        11,
                        "Open Recent",
                        vec![MenuItem::new(12, "a.txt"), MenuItem::new(13, "b.txt")],
                    ),
                    MenuItem::separator(14),
                    MenuItem::new(15, "Close").disabled(),
                    MenuItem::new(16, "Show Hidden").with_toggle(Toggle::Check(true)),
                ],
            ),
            MenuItem::submenu(
                2,
                "Edit",
                vec![
                    MenuItem::new(20, "Undo"),
                    MenuItem::new(21, "Redo").disabled(),
                    MenuItem::separator(22),
                    MenuItem::new(23, "Cut"),
                    MenuItem::new(24, "Copy"),
                    MenuItem::new(25, "Paste"),
                ],
            ),
            MenuItem::submenu(
                3,
                "View",
                vec![MenuItem::new(30, "Zoom In"), MenuItem::new(31, "Zoom Out")],
            ),
        ],
        status: vec![
            StatusEntry {
                id: "net".into(),
                width: 16.0,
                menu: Some(MenuItem::submenu(
                    500,
                    "net",
                    vec![MenuItem::new(501, "Wi-Fi Off")],
                )),
            },
            StatusEntry {
                id: "idle".into(),
                width: 16.0,
                menu: None,
            },
        ],
        clock: "Tue 10:42".into(),
        control_panel: true,
    }
}

/// A controller whose choices are made at once, as under Reduce Motion: the flash a chosen row gives first has a test
/// of its own.
fn controller() -> MenuController {
    let mut c = flashing_controller();
    c.cfg.flash_choice = false;
    c
}

fn flashing_controller() -> MenuController {
    let mut c = MenuController::new(OUT, Box::new(MonoMeasure::default()));
    c.set_content(content());
    c
}

fn slot_center(c: &MenuController, key: &SlotKey) -> Point {
    let r = c.layout().slot(key).unwrap().rect;
    Point::new(r.x + r.w / 2.0, r.y + r.h / 2.0)
}

fn row_center(c: &MenuController, level: usize, id: i32) -> Point {
    let l = &c.levels()[level].layout;
    let r = l.rows[l.index_of(ItemId(id)).unwrap()].rect;
    Point::new(r.x + r.w / 2.0, r.y + r.h / 2.0)
}

fn file() -> SlotKey {
    SlotKey::Menu(ItemId(1))
}
fn edit() -> SlotKey {
    SlotKey::Menu(ItemId(2))
}

fn selected_id(c: &MenuController, level: usize) -> Option<i32> {
    c.levels()[level].selected_item().map(|i| i.id.0)
}

fn has(o: &cupertino_menubar::controller::Outcome, e: &Effect) -> bool {
    o.effects.contains(e)
}

#[test]
fn click_opens_a_sticky_menu_and_a_second_click_chooses() {
    let mut c = controller();
    let p = slot_center(&c, &file());
    let o = c.pointer_press(p, 0.0);
    assert!(o.consumed);
    assert!(has(&o, &Effect::Opened { slot: file() }));
    assert!(has(
        &o,
        &Effect::AboutToShow {
            slot: file(),
            item: ItemId(1)
        }
    ));
    assert_eq!(c.mode(), Mode::Tracking);
    let o = c.pointer_release(p, 0.1);
    assert!(o.consumed && o.effects.is_empty());
    assert_eq!(c.mode(), Mode::Sticky);
    assert!(c.is_open());
    let item = row_center(&c, 0, 10);
    c.pointer_move(item, 0.5);
    assert_eq!(selected_id(&c, 0), Some(10));
    c.pointer_press(item, 0.6);
    let o = c.pointer_release(item, 0.65);
    assert!(has(
        &o,
        &Effect::Activate {
            slot: file(),
            item: ItemId(10)
        }
    ));
    assert!(has(&o, &Effect::Closed));
    assert!(!c.is_open());
    assert_eq!(c.mode(), Mode::Idle);
}

#[test]
fn a_chosen_row_goes_dark_then_lights_again_and_only_then_is_chosen_and_the_menus_close() {
    use cupertino_menubar::controller::{FLASH_OFF, FLASH_ON};
    let mut c = flashing_controller();
    c.pointer_press(slot_center(&c, &file()), 0.0);
    c.pointer_move(row_center(&c, 0, 16), 0.2);
    let o = c.pointer_release(row_center(&c, 0, 16), 0.4);
    assert!(!o
        .effects
        .iter()
        .any(|e| matches!(e, Effect::Activate { .. })));
    assert!(c.is_open(), "open while the row flashes");
    assert_eq!(selected_id(&c, 0), None, "the row goes dark first");
    assert_eq!(c.next_deadline(), Some(0.4 + FLASH_OFF));
    c.tick(0.4 + FLASH_OFF);
    assert_eq!(selected_id(&c, 0), Some(16), "then lights again");
    let ends = 0.4 + FLASH_OFF + FLASH_ON;
    assert_eq!(c.next_deadline(), Some(ends));
    let o = c.tick(ends);
    assert!(has(
        &o,
        &Effect::Activate {
            slot: file(),
            item: ItemId(16)
        }
    ));
    assert!(has(&o, &Effect::Closed));
    assert!(!c.is_open());
}

#[test]
fn press_drag_release_chooses_in_one_gesture() {
    let mut c = controller();
    c.pointer_press(slot_center(&c, &file()), 0.0);
    c.pointer_move(row_center(&c, 0, 16), 0.2);
    assert_eq!(selected_id(&c, 0), Some(16));
    let o = c.pointer_release(row_center(&c, 0, 16), 0.4);
    assert!(has(
        &o,
        &Effect::Activate {
            slot: file(),
            item: ItemId(16)
        }
    ));
    assert!(!c.is_open());
}

#[test]
fn dragging_across_titles_switches_menus_and_release_chooses_there() {
    let mut c = controller();
    c.pointer_press(slot_center(&c, &file()), 0.0);
    let o = c.pointer_move(slot_center(&c, &edit()), 0.1);
    assert!(has(&o, &Effect::Opened { slot: edit() }));
    assert_eq!(c.open_slot(), Some(&edit()));
    c.pointer_move(row_center(&c, 0, 24), 0.2);
    let o = c.pointer_release(row_center(&c, 0, 24), 0.3);
    assert!(has(
        &o,
        &Effect::Activate {
            slot: edit(),
            item: ItemId(24)
        }
    ));
}

#[test]
fn releasing_outside_cancels() {
    let mut c = controller();
    c.pointer_press(slot_center(&c, &file()), 0.0);
    c.pointer_move(Point::new(900.0, 700.0), 0.1);
    let o = c.pointer_release(Point::new(900.0, 700.0), 0.2);
    assert!(has(&o, &Effect::Closed));
    assert!(!o
        .effects
        .iter()
        .any(|e| matches!(e, Effect::Activate { .. })));
}

#[test]
fn release_on_a_disabled_item_or_separator_keeps_the_menu_open() {
    let mut c = controller();
    c.pointer_press(slot_center(&c, &file()), 0.0);
    c.pointer_move(row_center(&c, 0, 15), 0.1);
    assert_eq!(
        selected_id(&c, 0),
        None,
        "disabled rows are not highlighted"
    );
    let o = c.pointer_release(row_center(&c, 0, 15), 0.2);
    assert!(o
        .effects
        .iter()
        .all(|e| !matches!(e, Effect::Activate { .. } | Effect::Closed)));
    assert!(c.is_open() && c.mode() == Mode::Sticky);
    c.pointer_move(row_center(&c, 0, 14), 0.3);
    assert_eq!(selected_id(&c, 0), None, "separators are not highlighted");
}

#[test]
fn hovering_across_titles_switches_only_while_a_menu_is_open() {
    let mut c = controller();
    let o = c.pointer_move(slot_center(&c, &edit()), 0.0);
    assert!(!o.consumed && !c.is_open());
    let p = slot_center(&c, &file());
    c.pointer_press(p, 0.1);
    c.pointer_release(p, 0.2);
    let o = c.pointer_move(slot_center(&c, &edit()), 0.3);
    assert!(o.consumed && has(&o, &Effect::Opened { slot: edit() }));
    c.pointer_move(slot_center(&c, &SlotKey::AppMenu), 0.4);
    assert_eq!(c.open_slot(), Some(&SlotKey::AppMenu));
    assert_eq!(c.levels()[0].menu.id, APP_MENU_ROOT);
    let o = c.pointer_move(slot_center(&c, &SlotKey::Clock), 0.5);
    assert_eq!(
        c.open_slot(),
        Some(&SlotKey::AppMenu),
        "buttons do not switch the menu"
    );
    assert!(o.consumed);
    let mut cfg_off = controller();
    cfg_off.cfg.hover_switch = false;
    let p = slot_center(&cfg_off, &file());
    cfg_off.pointer_press(p, 0.0);
    cfg_off.pointer_release(p, 0.1);
    cfg_off.pointer_move(slot_center(&cfg_off, &edit()), 0.2);
    assert_eq!(cfg_off.open_slot(), Some(&file()));
}

#[test]
fn clicking_the_open_title_closes_and_clicking_outside_closes() {
    let mut c = controller();
    let p = slot_center(&c, &file());
    c.pointer_press(p, 0.0);
    c.pointer_release(p, 0.1);
    let o = c.pointer_press(p, 1.0);
    assert!(has(&o, &Effect::Closed));
    let o = c.pointer_release(p, 1.1);
    assert!(
        o.effects.is_empty() && !c.is_open(),
        "the release does not reopen"
    );

    c.pointer_press(p, 2.0);
    c.pointer_release(p, 2.1);
    let o = c.pointer_press(Point::new(900.0, 700.0), 3.0);
    assert!(has(&o, &Effect::Closed));
    assert!(o.consumed, "the closing click is swallowed by default");
    c.pointer_press(p, 4.0);
    c.pointer_release(p, 4.1);
    c.cfg.click_outside_passthrough = true;
    let o = c.pointer_press(Point::new(900.0, 700.0), 5.0);
    assert!(!o.consumed);
}

/// A point of the bar that is in no slot: right of the last menu title.
fn empty_bar_point(c: &MenuController) -> Point {
    let layout = c.layout();
    let y = layout.bar.y + layout.bar.h / 2.0;
    let after = layout
        .slots
        .iter()
        .filter(|s| matches!(s.key, SlotKey::Menu(_)))
        .map(|s| s.rect.x + s.rect.w)
        .fold(0.0, f64::max);
    let p = Point::new(after + 10.0, y);
    assert!(
        layout.bar.contains(p) && layout.hit(p).is_none(),
        "{p:?} is empty bar space"
    );
    p
}

#[test]
fn a_click_on_empty_bar_space_closes_the_menu_without_choosing_and_is_consumed() {
    for sticky in [true, false] {
        let mut c = controller();
        let title = slot_center(&c, &file());
        c.pointer_press(title, 0.0);
        if sticky {
            c.pointer_release(title, 0.1);
        }
        // a row of the open menu is selected under the pointer on the way, as a hover would leave it
        c.pointer_move(row_center(&c, 0, 10), 0.2);
        assert!(c.is_open());
        let empty = empty_bar_point(&c);
        let press = c.pointer_press(empty, 1.0);
        let release = c.pointer_release(empty, 1.1);
        assert!(has(&press, &Effect::Closed), "sticky {sticky}: {press:?}");
        assert!(!c.is_open() && c.open_slot().is_none());
        assert_eq!(c.mode(), Mode::Idle);
        for o in [&press, &release] {
            assert!(
                !o.effects
                    .iter()
                    .any(|e| matches!(e, Effect::Activate { .. } | Effect::SlotActivated { .. })),
                "sticky {sticky}: nothing is chosen or activated: {o:?}"
            );
        }
        assert!(
            press.consumed,
            "sticky {sticky}: the closing press reaches no client"
        );
        // the next press on the empty bar does nothing at all, and the title opens its menu again
        let o = c.pointer_press(empty, 2.0);
        assert!(o.effects.is_empty() && !c.is_open());
        c.pointer_release(empty, 2.1);
        c.pointer_press(title, 3.0);
        assert!(c.is_open());
    }
}

#[test]
fn button_slots_activate_on_release_over_the_same_slot() {
    let mut c = controller();
    let clock = slot_center(&c, &SlotKey::Clock);
    let o = c.pointer_press(clock, 0.0);
    assert!(o.consumed && !c.is_open());
    let o = c.pointer_release(clock, 0.1);
    assert!(has(
        &o,
        &Effect::SlotActivated {
            slot: SlotKey::Clock
        }
    ));
    c.pointer_press(clock, 1.0);
    let o = c.pointer_release(slot_center(&c, &SlotKey::ControlPanel), 1.1);
    assert!(o.effects.is_empty());
    let idle = slot_center(&c, &SlotKey::Status("idle".into()));
    c.pointer_press(idle, 2.0);
    assert!(has(
        &c.pointer_release(idle, 2.1),
        &Effect::SlotActivated {
            slot: SlotKey::Status("idle".into())
        }
    ));
}

#[test]
fn status_item_menus_open_like_title_menus() {
    let mut c = controller();
    let p = slot_center(&c, &SlotKey::Status("net".into()));
    let o = c.pointer_press(p, 0.0);
    assert!(has(
        &o,
        &Effect::Opened {
            slot: SlotKey::Status("net".into())
        }
    ));
    c.pointer_move(row_center(&c, 0, 501), 0.1);
    let o = c.pointer_release(row_center(&c, 0, 501), 0.2);
    assert!(has(
        &o,
        &Effect::Activate {
            slot: SlotKey::Status("net".into()),
            item: ItemId(501)
        }
    ));
}

#[test]
fn keyboard_focus_arrows_open_choose_and_escape() {
    let mut c = controller();
    assert!(!c.key(Key::Down, 0.0).consumed, "idle keys pass through");
    let o = c.focus_bar();
    assert!(o.consumed);
    assert_eq!(c.mode(), Mode::TitleFocus);
    assert_eq!(c.highlighted_slot(), Some(&SlotKey::AppMenu));
    c.key(Key::Right, 0.1);
    assert_eq!(c.highlighted_slot(), Some(&file()));
    c.key(Key::Left, 0.2);
    c.key(Key::Left, 0.3);
    assert_eq!(
        c.highlighted_slot(),
        Some(&SlotKey::Status("net".into())),
        "wraps over the menu-bearing slots only"
    );
    c.key(Key::Right, 0.4);
    c.key(Key::Right, 0.5);
    let o = c.key(Key::Down, 0.6);
    assert!(has(&o, &Effect::Opened { slot: file() }));
    assert_eq!(c.mode(), Mode::Keyboard);
    assert_eq!(selected_id(&c, 0), Some(10));
    c.key(Key::Down, 0.7);
    assert_eq!(selected_id(&c, 0), Some(11));
    c.key(Key::Down, 0.8);
    assert_eq!(
        selected_id(&c, 0),
        Some(16),
        "separator and disabled item are skipped"
    );
    c.key(Key::Down, 0.9);
    assert_eq!(selected_id(&c, 0), Some(10), "wraps");
    c.key(Key::Up, 1.0);
    assert_eq!(selected_id(&c, 0), Some(16));
    c.key(Key::Home, 1.1);
    assert_eq!(selected_id(&c, 0), Some(10));
    c.key(Key::End, 1.2);
    assert_eq!(selected_id(&c, 0), Some(16));
    let o = c.key(Key::Enter, 1.3);
    assert!(has(
        &o,
        &Effect::Activate {
            slot: file(),
            item: ItemId(16)
        }
    ));
    assert!(!c.is_open());
    assert_eq!(c.mode(), Mode::Idle);
}

#[test]
fn escape_returns_to_title_focus_then_leaves() {
    let mut c = controller();
    c.focus_bar();
    c.key(Key::Right, 0.1);
    c.key(Key::Down, 0.2);
    assert!(c.is_open());
    c.key(Key::Escape, 0.3);
    assert!(!c.is_open());
    assert_eq!(c.mode(), Mode::TitleFocus);
    assert_eq!(c.highlighted_slot(), Some(&file()));
    c.key(Key::Escape, 0.4);
    assert_eq!(c.mode(), Mode::Idle);
    assert_eq!(c.highlighted_slot(), None);
    let o = c.focus_bar();
    assert!(o.consumed);
    let o = c.focus_bar();
    assert!(o.consumed && c.mode() == Mode::Idle, "the shortcut toggles");
}

#[test]
fn left_right_switch_menus_while_open() {
    let mut c = controller();
    c.focus_bar();
    c.key(Key::Right, 0.1);
    c.key(Key::Down, 0.2);
    c.key(Key::Down, 0.3);
    assert_eq!(selected_id(&c, 0), Some(11));
    c.key(Key::Right, 0.31);
    assert_eq!(
        c.open_slot(),
        Some(&file()),
        "Right on a submenu row opens it instead of switching"
    );
    assert_eq!(c.levels().len(), 2);
    c.key(Key::Escape, 0.32);
    assert_eq!(c.levels().len(), 1);
    assert_eq!(selected_id(&c, 0), Some(11));
    c.key(Key::Right, 0.4);
    assert_eq!(c.levels().len(), 2);
    assert_eq!(selected_id(&c, 1), Some(12));
    c.key(Key::Left, 0.5);
    assert_eq!(c.levels().len(), 1);
    c.key(Key::End, 0.55);
    c.key(Key::Right, 0.6);
    assert_eq!(c.open_slot(), Some(&edit()));
    assert_eq!(
        selected_id(&c, 0),
        Some(20),
        "first selectable row of the next menu"
    );
    c.key(Key::Left, 0.7);
    assert_eq!(c.open_slot(), Some(&file()));
    c.key(Key::Left, 0.8);
    assert_eq!(c.open_slot(), Some(&SlotKey::AppMenu));
}

#[test]
fn type_ahead_selects_by_prefix_and_cycles_on_repeat() {
    let mut c = controller();
    let p = slot_center(&c, &edit());
    c.pointer_press(p, 0.0);
    c.pointer_release(p, 0.1);
    c.key(Key::Char('c'), 1.0);
    assert_eq!(selected_id(&c, 0), Some(23), "Cut");
    c.key(Key::Char('c'), 1.2);
    assert_eq!(
        selected_id(&c, 0),
        Some(24),
        "repeating the letter cycles: Copy"
    );
    c.key(Key::Char('c'), 1.3);
    assert_eq!(selected_id(&c, 0), Some(23));
    c.key(Key::Char('p'), 5.0);
    assert_eq!(
        selected_id(&c, 0),
        Some(25),
        "buffer restarts after the timeout"
    );
    c.key(Key::Char('u'), 5.2);
    assert_eq!(
        selected_id(&c, 0),
        Some(25),
        "'pu' matches nothing and keeps the selection"
    );
    c.key(Key::Char('u'), 9.0);
    assert_eq!(selected_id(&c, 0), Some(20), "Undo");
    c.key(Key::Char('r'), 12.0);
    assert_eq!(selected_id(&c, 0), Some(20), "disabled Redo is skipped");
}

#[test]
fn mouse_movement_takes_over_from_the_keyboard() {
    let mut c = controller();
    c.focus_bar();
    c.key(Key::Right, 0.1);
    c.key(Key::Down, 0.2);
    assert_eq!(c.mode(), Mode::Keyboard);
    c.pointer_move(row_center(&c, 0, 16), 0.3);
    assert_eq!(c.mode(), Mode::Sticky);
    assert_eq!(selected_id(&c, 0), Some(16));
}

#[test]
fn submenus_open_after_the_hover_delay_and_report_about_to_show() {
    let mut c = controller();
    let p = slot_center(&c, &file());
    c.pointer_press(p, 0.0);
    c.pointer_release(p, 0.1);
    c.pointer_move(row_center(&c, 0, 11), 1.0);
    assert_eq!(c.levels().len(), 1);
    assert_eq!(c.next_deadline(), Some(1.0 + c.cfg.submenu_open_delay));
    assert!(c
        .tick(1.0 + c.cfg.submenu_open_delay - 0.05)
        .effects
        .is_empty());
    let o = c.tick(1.0 + c.cfg.submenu_open_delay + 0.01);
    assert!(has(
        &o,
        &Effect::AboutToShow {
            slot: file(),
            item: ItemId(11)
        }
    ));
    assert_eq!(c.levels().len(), 2);
    assert_eq!(c.levels()[1].menu.id, ItemId(11));
    let parent = c.levels()[0].layout.rect;
    assert!(
        c.levels()[1].layout.rect.x < parent.right() + 1e-9
            && c.levels()[1].layout.rect.x > parent.x
    );
    c.pointer_move(row_center(&c, 1, 13), 1.4);
    assert_eq!(selected_id(&c, 1), Some(13));
    let o = c.pointer_press(row_center(&c, 1, 13), 1.5);
    assert!(o.consumed);
    let o = c.pointer_release(row_center(&c, 1, 13), 1.6);
    assert!(has(
        &o,
        &Effect::Activate {
            slot: file(),
            item: ItemId(13)
        }
    ));
    assert!(!c.is_open());
}

#[test]
fn clicking_a_submenu_row_opens_it_immediately_and_keeps_the_menu_open() {
    let mut c = controller();
    c.pointer_press(slot_center(&c, &file()), 0.0);
    c.pointer_move(row_center(&c, 0, 11), 0.1);
    let o = c.pointer_release(row_center(&c, 0, 11), 0.15);
    assert!(o
        .effects
        .iter()
        .all(|e| !matches!(e, Effect::Activate { .. })));
    assert_eq!(c.mode(), Mode::Sticky);
    c.pointer_press(row_center(&c, 0, 11), 1.0);
    assert_eq!(c.levels().len(), 2);
}

#[test]
fn moving_off_the_way_onto_another_row_closes_the_submenu_at_once() {
    let mut c = controller();
    let p = slot_center(&c, &file());
    c.pointer_press(p, 0.0);
    c.pointer_release(p, 0.1);
    c.pointer_move(row_center(&c, 0, 11), 1.0);
    c.tick(1.0 + c.cfg.submenu_open_delay);
    assert_eq!(c.levels().len(), 2);
    let o = c.pointer_move(row_center(&c, 0, 16), 2.0);
    assert!(has(&o, &Effect::Redraw));
    assert_eq!(c.levels().len(), 1, "closed with the move");
    assert_eq!(selected_id(&c, 0), Some(16), "and the row lit");
    assert_eq!(c.next_deadline(), None, "nothing left to wait for");
}

#[test]
fn leaving_the_menus_keeps_the_open_submenu() {
    let mut c = controller();
    let p = slot_center(&c, &file());
    c.pointer_press(p, 0.0);
    c.pointer_release(p, 0.1);
    let row = row_center(&c, 0, 11);
    c.pointer_move(row, 1.0);
    c.tick(1.0 + c.cfg.submenu_open_delay);
    assert_eq!(c.levels().len(), 2);
    let parent = c.levels()[0].layout.rect;
    // off the menu on the side away from the submenu
    c.pointer_move(Point::new(parent.x - 30.0, row.y), 1.5);
    c.tick(3.0);
    assert_eq!(c.levels().len(), 2, "the submenu stays to come back to");
    assert_eq!(selected_id(&c, 0), Some(11));
}

#[test]
fn safe_triangle_keeps_the_submenu_while_crossing_other_rows() {
    let mut c = controller();
    let p = slot_center(&c, &file());
    c.pointer_press(p, 0.0);
    c.pointer_release(p, 0.1);
    let row = row_center(&c, 0, 11);
    c.pointer_move(row, 1.0);
    c.tick(1.0 + c.cfg.submenu_open_delay);
    assert_eq!(c.levels().len(), 2);
    let child = c.levels()[1].layout.rect;
    let parent = c.levels()[0].layout.rect;
    // a point in the parent popup, below the submenu row and left of the child's near edge: on the
    // straight path from `row` towards the bottom corner of the child popup
    let probe = Point::new(parent.right() - 10.0, row.y + 24.0);
    assert!(point_in_triangle(
        probe,
        row,
        Point::new(child.x, child.y),
        Point::new(child.x, child.bottom())
    ));
    assert!(c.levels()[0].layout.row_at(probe).is_some());
    assert!(!child.contains(probe));
    c.pointer_move(probe, 1.35);
    assert_eq!(
        selected_id(&c, 0),
        Some(11),
        "selection stays on the submenu row inside the triangle"
    );
    assert_eq!(c.levels().len(), 2);
    // a point on another row but away from the triangle changes the selection
    let away = row_center(&c, 0, 16);
    c.pointer_move(away, 1.4);
    assert_eq!(selected_id(&c, 0), Some(16));
    c.cfg.safe_triangle = false;
    c.pointer_move(row, 2.0);
    c.tick(2.3);
    c.pointer_move(probe, 2.4);
    assert_ne!(
        selected_id(&c, 0),
        Some(11),
        "without the triangle the row under the pointer wins"
    );
}

#[test]
fn triangle_geometry() {
    let (a, b, c) = (
        Point::new(0.0, 0.0),
        Point::new(10.0, 0.0),
        Point::new(0.0, 10.0),
    );
    assert!(point_in_triangle(Point::new(2.0, 2.0), a, b, c));
    assert!(point_in_triangle(Point::new(0.0, 0.0), a, b, c));
    assert!(!point_in_triangle(Point::new(8.0, 8.0), a, b, c));
    assert!(!point_in_triangle(Point::new(-1.0, 1.0), a, b, c));
    assert!(
        point_in_triangle(Point::new(2.0, 2.0), a, c, b),
        "orientation independent"
    );
}

#[test]
fn content_updates_keep_the_open_menu() {
    let mut c = controller();
    let p = slot_center(&c, &file());
    c.pointer_press(p, 0.0);
    c.pointer_release(p, 0.1);
    c.pointer_move(row_center(&c, 0, 16), 0.2);
    let mut updated = content();
    updated.menus[0].children.push(MenuItem::new(17, "Extra"));
    let o = c.set_content(updated);
    assert!(has(&o, &Effect::Redraw));
    assert!(c.is_open());
    assert_eq!(c.levels()[0].layout.rows.len(), 6);
    assert_eq!(selected_id(&c, 0), Some(16), "selection survives by id");
    let mut gone = content();
    gone.menus.remove(0);
    let o = c.set_content(gone);
    assert!(has(&o, &Effect::Closed));
    assert!(!c.is_open());
}

#[test]
fn lazy_submenus_load_through_set_content() {
    let mut cont = content();
    cont.menus[0].children[1] = MenuItem::submenu(11, "Open Recent", vec![]);
    let mut c = MenuController::new(OUT, Box::new(MonoMeasure::default()));
    c.cfg.flash_choice = false;
    c.set_content(cont);
    c.focus_bar();
    c.key(Key::Right, 0.1);
    c.key(Key::Down, 0.2);
    c.key(Key::Down, 0.3);
    let o = c.key(Key::Right, 0.4);
    assert!(has(
        &o,
        &Effect::AboutToShow {
            slot: file(),
            item: ItemId(11)
        }
    ));
    assert_eq!(c.levels()[1].layout.rows.len(), 0);
    c.set_content(content());
    assert_eq!(
        c.levels()[1].layout.rows.len(),
        2,
        "children arrive and the popup is rebuilt"
    );
}

#[test]
fn close_resets_everything() {
    let mut c = controller();
    let p = slot_center(&c, &file());
    c.pointer_press(p, 0.0);
    c.pointer_release(p, 0.1);
    let o = c.close();
    assert!(has(&o, &Effect::Closed));
    assert_eq!(
        (c.mode(), c.is_open(), c.highlighted_slot()),
        (Mode::Idle, false, None)
    );
    assert!(c.close().effects.is_empty(), "closing twice is silent");
}

#[test]
fn narrow_output_hides_menus_that_do_not_fit() {
    let mut c = MenuController::new(
        Rect::new(0.0, 0.0, 260.0, 600.0),
        Box::new(MonoMeasure::default()),
    );
    c.cfg.flash_choice = false;
    c.set_content(content());
    assert!(!c.layout().hidden_menus.is_empty() || !c.layout().hidden_status.is_empty());
    c.focus_bar();
    for _ in 0..10 {
        c.key(Key::Right, 0.0);
    }
    let h = c.highlighted_slot().cloned().unwrap();
    assert!(
        c.layout().slot(&h).is_some(),
        "keyboard focus never lands on a hidden slot"
    );
}

#[test]
fn second_output_popups_stay_on_their_output() {
    let right = Rect::new(1920.0, 0.0, 1280.0, 800.0);
    let mut c = MenuController::new(right, Box::new(MonoMeasure::default()));
    c.cfg.flash_choice = false;
    c.set_content(content());
    let p = slot_center(&c, &edit());
    c.pointer_press(p, 0.0);
    let r = c.levels()[0].layout.rect;
    assert!(r.x >= right.x && r.right() <= right.right() && r.y >= 24.0 - 1e-9);
}

fn help() -> SlotKey {
    SlotKey::Menu(HELP_MENU)
}

/// The bar of [`content`] with a Help menu, every menu closed.
fn with_help() -> MenuController {
    let mut content = content();
    content.menus.push(MenuItem::submenu(
        HELP_MENU.0,
        "Help",
        vec![
            MenuItem::new(HELP_SEARCH.0, "Search").disabled(),
            MenuItem::separator(-1110),
            MenuItem::new(61, "Files Guide"),
        ],
    ));
    let mut c = MenuController::new(OUT, Box::new(MonoMeasure::default()));
    c.cfg.flash_choice = false;
    c.set_content(content);
    c
}

/// The bar of [`content`] with a Help menu, the Help menu open from a click.
fn help_open() -> MenuController {
    let mut c = with_help();
    let p = slot_center(&c, &help());
    c.pointer_press(p, 0.0);
    c.pointer_release(p, 0.05);
    assert_eq!(c.open_slot(), Some(&help()));
    c
}

/// A key press of the search field, `code` the evdev code; whether the field took it.
fn field_key(c: &mut MenuController, key: Option<Key>, code: u32, character: Option<char>) -> bool {
    let input = KeyInput::new(code, false, false, false, false, character);
    c.search_key(key, &mut Keymap::standard(), input, Instant::now())
        .is_some_and(|o| o.consumed)
}

fn type_text(c: &mut MenuController, text: &str) {
    for ch in text.chars() {
        assert!(field_key(c, None, 0, Some(ch)), "the field takes {ch:?}");
    }
}

fn row_ids(c: &MenuController) -> Vec<i32> {
    c.levels()[0]
        .layout
        .rows
        .iter()
        .map(|r| r.item.id.0)
        .collect()
}

#[test]
fn typing_in_the_help_menu_lists_results_and_up_down_and_return_choose_one() {
    let mut c = help_open();
    assert_eq!(row_ids(&c), [-1030, -1110, 61]);
    type_text(&mut c, "zoom");
    assert_eq!(c.search_field().map(|f| f.text()), Some("zoom"));
    assert_eq!(row_ids(&c), [-1030, -1050, -1051, -1110, 61]);
    let rows = &c.levels()[0].layout.rows;
    assert_eq!(
        (rows[1].item.label.as_str(), rows[1].detail.as_deref()),
        ("Zoom In", Some("View"))
    );
    assert_eq!(
        selected_id(&c, 0),
        Some(-1050),
        "the first result is selected"
    );
    assert!(
        field_key(&mut c, Some(Key::Space), 57, Some(' ')),
        "Space types"
    );
    assert_eq!(c.search_field().map(|f| f.text()), Some("zoom "));

    assert!(
        !field_key(&mut c, Some(Key::Down), 108, None),
        "Down is the menu's"
    );
    c.key(Key::Down, 1.1);
    assert_eq!(selected_id(&c, 0), Some(-1051));
    c.key(Key::Up, 1.2);
    assert_eq!(selected_id(&c, 0), Some(-1050));
    c.key(Key::Down, 1.3);
    let o = c.key(Key::Enter, 1.4);
    assert!(has(
        &o,
        &Effect::Activate {
            slot: SlotKey::Menu(ItemId(3)),
            item: ItemId(31)
        }
    ));
    assert!(!c.is_open() && c.search_field().is_none());
}

#[test]
fn a_click_on_a_result_chooses_the_item_in_its_own_menu() {
    let mut c = help_open();
    type_text(&mut c, "files");
    // Files Guide (Help), About Files, Hide Files
    assert_eq!(row_ids(&c)[..4], [-1030, -1050, -1051, -1052]);
    let p = row_center(&c, 0, -1052);
    c.pointer_move(p, 1.0);
    c.pointer_press(p, 1.1);
    let o = c.pointer_release(p, 1.2);
    assert!(has(
        &o,
        &Effect::Activate {
            slot: SlotKey::AppMenu,
            item: ItemId(102)
        }
    ));
}

#[test]
fn escape_clears_the_text_first_then_closes_the_menu() {
    let mut c = help_open();
    type_text(&mut c, "qq");
    assert_eq!(row_ids(&c), [-1030, -1031, -1110, 61]);
    assert_eq!(c.levels()[0].layout.rows[1].item.label, "No Results");
    assert_eq!(selected_id(&c, 0), None, "No Results cannot be selected");
    c.key(Key::Down, 1.0);
    assert_eq!(selected_id(&c, 0), Some(61));
    let o = c.key(Key::Escape, 1.1);
    assert!(c.is_open() && !has(&o, &Effect::Closed));
    assert_eq!(c.search_field().map(|f| f.text()), Some(""));
    assert_eq!(row_ids(&c), [-1030, -1110, 61]);
    let o = c.key(Key::Escape, 1.2);
    assert!(has(&o, &Effect::Closed) && !c.is_open());
}

#[test]
fn the_arrows_move_the_caret_while_the_field_holds_text_and_switch_menus_without() {
    let mut c = help_open();
    assert!(
        !field_key(&mut c, Some(Key::Left), 105, None),
        "an empty field leaves Left to the menu"
    );
    type_text(&mut c, "cu");
    assert!(field_key(&mut c, Some(Key::Left), 105, None));
    assert_eq!(c.search_field().unwrap().caret(), 1);
    type_text(&mut c, "o");
    assert_eq!(c.search_field().unwrap().text(), "cou");
    assert_eq!(c.open_slot(), Some(&help()));
}

#[test]
fn result_ids_stay_by_position_while_typing_and_across_content_updates() {
    let mut c = help_open();
    type_text(&mut c, "c");
    assert_eq!(row_ids(&c)[..3], [-1030, -1050, -1051]);
    let labels: Vec<&str> = c.levels()[0].layout.rows[1..3]
        .iter()
        .map(|r| r.item.label.as_str())
        .collect();
    assert_eq!(labels, ["Cut", "Copy"]);
    c.key(Key::Down, 1.0);
    assert_eq!(selected_id(&c, 0), Some(-1051));
    let content = c.content().clone();
    c.set_content(content);
    assert_eq!(
        selected_id(&c, 0),
        Some(-1051),
        "an update keeps the selected result"
    );
    type_text(&mut c, "o");
    assert_eq!(row_ids(&c)[..2], [-1030, -1050]);
    assert_eq!(c.levels()[0].layout.rows[1].item.label, "Copy");
    let o = c.key(Key::Enter, 1.1);
    assert!(has(
        &o,
        &Effect::Activate {
            slot: edit(),
            item: ItemId(24)
        }
    ));
}

#[test]
fn other_menus_have_no_search_field() {
    let mut c = controller();
    let p = slot_center(&c, &file());
    c.pointer_press(p, 0.0);
    c.pointer_release(p, 0.05);
    assert!(c.search_field().is_none());
    assert!(!field_key(&mut c, None, 0, Some('n')));
}

#[test]
fn the_help_search_shortcut_takes_the_keyboard_then_opens_help_with_the_field_taking_typing() {
    let mut c = with_help();
    let o = c.focus_help_search(false);
    assert!(o.consumed);
    // as for the menu-bar shortcut: the Help title has keyboard focus, which asks for the keyboard
    assert_eq!(c.mode(), Mode::TitleFocus);
    assert_eq!(c.highlighted_slot(), Some(&help()));
    assert!(!c.is_open(), "no menu before the keyboard is the bar's");

    let o = c.keyboard_arrived();
    assert!(has(&o, &Effect::Opened { slot: help() }));
    assert_eq!(c.mode(), Mode::Keyboard);
    assert_eq!(c.search_field().map(|f| f.text()), Some(""));
    assert_eq!(selected_id(&c, 0), None, "the field has the focus");
    type_text(&mut c, "zoom");
    assert_eq!(row_ids(&c)[..3], [-1030, -1050, -1051]);
    assert_eq!(selected_id(&c, 0), Some(-1050));

    // Esc clears the field, then closes the menu: nothing holds the keyboard any more
    c.key(Key::Escape, 1.0);
    assert_eq!(c.search_field().map(|f| f.text()), Some(""));
    let o = c.key(Key::Escape, 1.1);
    assert!(has(&o, &Effect::Closed));
    assert_eq!(c.mode(), Mode::Idle);
    assert!(!c.is_open() && c.highlighted_slot().is_none());
    assert!(
        !c.keyboard_arrived().consumed,
        "a later keyboard focus opens nothing"
    );
}

#[test]
fn esc_before_the_keyboard_arrives_gives_it_back_and_nothing_opens() {
    let mut c = with_help();
    c.focus_help_search(false);
    let o = c.key(Key::Escape, 0.5);
    assert!(has(&o, &Effect::Closed));
    assert_eq!(c.mode(), Mode::Idle);
    assert!(!c.keyboard_arrived().consumed);
    assert!(!c.is_open());
}

#[test]
fn with_the_keyboard_held_or_a_menu_open_the_help_search_shortcut_opens_help_at_once() {
    // the menu-bar shortcut gave the bar the keyboard
    let mut c = with_help();
    c.focus_bar();
    let o = c.focus_help_search(true);
    assert!(has(&o, &Effect::Opened { slot: help() }));
    assert!(c.search_field().is_some());

    // another menu is open: Help takes its place
    let mut c = with_help();
    let p = slot_center(&c, &file());
    c.pointer_press(p, 0.0);
    c.pointer_release(p, 0.05);
    c.focus_help_search(true);
    assert_eq!(c.open_slot(), Some(&help()));
    type_text(&mut c, "zoom");
    assert_eq!(c.search_field().map(|f| f.text()), Some("zoom"));
}

#[test]
fn with_help_open_the_help_search_shortcut_keeps_the_field_and_its_text() {
    let mut c = help_open();
    type_text(&mut c, "zo");
    let o = c.focus_help_search(true);
    assert!(o.consumed && !has(&o, &Effect::Closed));
    assert_eq!(c.search_field().map(|f| f.text()), Some("zo"));
    type_text(&mut c, "om");
    assert_eq!(c.search_field().map(|f| f.text()), Some("zoom"));
}

#[test]
fn without_a_help_menu_the_help_search_shortcut_does_nothing() {
    let mut c = controller();
    assert!(!c.focus_help_search(false).consumed);
    assert_eq!(c.mode(), Mode::Idle);
}

fn window() -> SlotKey {
    SlotKey::Menu(WINDOW_MENU)
}

/// The bar's Window menu with an exporter's item below its own.
fn with_window() -> MenuController {
    let mut content = content();
    let mut menu = fallback::window_menu(1, Some(1), true, true, &[], &Chords::default());
    menu.children
        .push(MenuItem::new(90, "Split View").exported());
    content.menus.push(menu);
    let mut c = controller();
    c.set_content(content);
    let p = slot_center(&c, &window());
    c.pointer_press(p, 0.0);
    c.pointer_release(p, 0.05);
    c
}

fn row_labels(c: &MenuController) -> Vec<String> {
    c.levels()[0]
        .layout
        .rows
        .iter()
        .filter(|r| !r.item.separator)
        .map(|r| r.item.label.clone())
        .collect()
}

#[test]
fn alt_shows_the_alternates_of_the_open_menu_until_it_goes_up() {
    let mut c = with_window();
    let id = |a: FallbackAction| a.item_id().0;
    c.pointer_move(row_center(&c, 0, id(FallbackAction::Minimize)), 0.2);
    let width = c.levels()[0].layout.rect;
    let o = c.set_alt(true);
    assert!(has(&o, &Effect::Redraw));
    assert_eq!(
        row_labels(&c),
        [
            "Minimize All",
            "Zoom All",
            "Tile Window",
            "Arrange in Front",
            "Close All",
            "Split View"
        ]
    );
    assert_eq!(
        row_ids(&c)[..2],
        [id(FallbackAction::MinimizeAll), id(FallbackAction::ZoomAll)]
    );
    assert!(row_ids(&c).contains(&id(FallbackAction::CascadeAll)));
    assert!(row_ids(&c).contains(&90), "an exporter's item stays");
    assert_eq!(
        selected_id(&c, 0),
        Some(id(FallbackAction::MinimizeAll)),
        "the highlight stays on the row"
    );
    assert_eq!(c.levels()[0].layout.rect, width, "the menu keeps its size");
    assert!(c.set_alt(true).effects.is_empty(), "nothing changed");
    c.set_alt(false);
    assert_eq!(
        row_labels(&c),
        [
            "Minimize",
            "Zoom",
            "Tile Window",
            "Bring All to Front",
            "Close Window",
            "Split View"
        ]
    );
    assert_eq!(selected_id(&c, 0), Some(id(FallbackAction::Minimize)));
    assert_eq!(c.levels()[0].layout.rect, width);
}

#[test]
fn a_menu_opened_with_alt_held_shows_the_alternates_and_closed_menus_are_not_redrawn() {
    let mut c = controller();
    let mut content = content();
    content.menus.push(fallback::window_menu(
        1,
        Some(1),
        true,
        true,
        &[],
        &Chords::default(),
    ));
    c.set_content(content);
    assert!(c.set_alt(true).effects.is_empty());
    let p = slot_center(&c, &window());
    c.pointer_press(p, 0.0);
    assert_eq!(row_labels(&c)[0], "Minimize All");
}

#[test]
fn activation_chooses_what_the_row_shows_at_that_moment() {
    let id = |a: FallbackAction| a.item_id().0;
    let activate = |a: FallbackAction| Effect::Activate {
        slot: window(),
        item: a.item_id(),
    };
    for (alt, chosen) in [
        (false, FallbackAction::CloseWindow),
        (true, FallbackAction::CloseAll),
    ] {
        let mut c = with_window();
        c.set_alt(alt);
        let row = row_center(&c, 0, id(chosen));
        c.pointer_move(row, 0.2);
        c.pointer_press(row, 0.3);
        assert!(has(&c.pointer_release(row, 0.35), &activate(chosen)));
    }
    let mut c = with_window();
    c.key(Key::Down, 0.2);
    assert_eq!(selected_id(&c, 0), Some(id(FallbackAction::Minimize)));
    c.set_alt(true);
    assert!(has(
        &c.key(Key::Enter, 0.3),
        &activate(FallbackAction::MinimizeAll)
    ));
    let mut c = with_window();
    c.set_alt(true);
    c.key(Key::Down, 0.2);
    c.set_alt(false);
    assert!(has(
        &c.key(Key::Enter, 0.3),
        &activate(FallbackAction::Minimize)
    ));
}

const FILES: FrontApp<'static> = FrontApp {
    app_id: "org.example.files",
    name: "Files",
    can_force_quit: true,
};

/// A bar whose System menu is open, built for `front`, with Alt held.
fn system_menu_with_alt(front: Option<FrontApp<'_>>) -> MenuController {
    let mut content = content();
    content.system_menu = Extras::default()
        .system_menu(&Chords::default(), &RecentItems::default(), front)
        .rows;
    let mut c = controller();
    c.set_content(content);
    let p = slot_center(&c, &SlotKey::System);
    c.pointer_press(p, 0.0);
    c.pointer_release(p, 0.05);
    c.set_alt(true);
    c
}

/// Click the row `id` of the open menu; whether that chose it.
fn choose(c: &mut MenuController, id: ItemId) -> bool {
    let row = row_center(c, 0, id.0);
    c.pointer_move(row, 0.2);
    c.pointer_press(row, 0.3);
    has(
        &c.pointer_release(row, 0.35),
        &Effect::Activate {
            slot: SlotKey::System,
            item: id,
        },
    )
}

#[test]
fn with_alt_held_the_system_menu_shows_force_quit_with_the_app_name_and_chooses_it() {
    let mut c = system_menu_with_alt(Some(FILES));
    let labels = row_labels(&c);
    assert!(
        labels.contains(&"Force Quit Files".to_owned()),
        "{labels:?}"
    );
    assert!(!labels.contains(&"Force Quit\u{2026}".to_owned()));
    assert!(choose(&mut c, SystemRow::ForceQuitApp.id()));
}

#[test]
fn the_system_menu_keeps_force_quit_on_the_desktop_and_greys_the_alternate_below_version_13() {
    let desktop = system_menu_with_alt(None);
    assert!(row_labels(&desktop).contains(&"Force Quit\u{2026}".to_owned()));
    let mut c = system_menu_with_alt(Some(FrontApp {
        can_force_quit: false,
        ..FILES
    }));
    assert!(row_labels(&c).contains(&"Force Quit Files".to_owned()));
    assert!(!choose(&mut c, SystemRow::ForceQuitApp.id()));
}

#[test]
fn the_system_menu_closes_when_its_content_goes_and_other_menus_stay() {
    let mut c = system_menu_with_alt(None);
    assert_eq!(c.open_slot(), Some(&SlotKey::System));
    assert!(
        c.close_slot(&SlotKey::AppMenu).is_none(),
        "another slot leaves it open"
    );
    assert!(c.is_open());
    assert!(c.close_slot(&SlotKey::System).is_some());
    assert!(!c.is_open());
    assert!(
        c.close_slot(&SlotKey::System).is_none(),
        "nothing left to close"
    );
}

// ---- tracking shared with every menu -----------------------------------------------------------

fn activations(o: &cupertino_menubar::controller::Outcome) -> usize {
    o.effects
        .iter()
        .filter(|e| matches!(e, Effect::Activate { .. }))
        .count()
}

/// The pointer's way from `from` to `to` in `steps` moves `dt` seconds apart from `t`, with the timers run between moves
/// as the bar runs them; the time of the last move.
fn path(c: &mut MenuController, from: Point, to: Point, steps: u32, t: f64, dt: f64) -> f64 {
    let mut now = t;
    for step in 1..=steps {
        let k = f64::from(step) / f64::from(steps);
        now = t + dt * f64::from(step);
        while let Some(due) = c.next_deadline().filter(|due| *due <= now) {
            c.tick(due);
        }
        c.pointer_move(
            Point::new(from.x + (to.x - from.x) * k, from.y + (to.y - from.y) * k),
            now,
        );
    }
    now
}

#[test]
fn a_press_dragged_onto_a_row_chooses_it_once_on_release_and_closes_the_menu() {
    let mut c = controller();
    let title = slot_center(&c, &edit());
    c.pointer_press(title, 0.0);
    let copy = row_center(&c, 0, 24);
    let now = path(&mut c, title, copy, 8, 0.0, 0.016);
    assert_eq!(selected_id(&c, 0), Some(24));
    let o = c.pointer_release(copy, now + 0.016);
    assert_eq!(activations(&o), 1);
    assert!(has(
        &o,
        &Effect::Activate {
            slot: edit(),
            item: ItemId(24)
        }
    ));
    assert!(!c.is_open() && has(&o, &Effect::Closed));
    let o = c.pointer_release(copy, now + 0.05);
    assert_eq!(activations(&o), 0, "a second release chooses nothing");
}

#[test]
fn a_quick_click_leaves_the_menu_open_and_a_hold_or_a_drag_back_to_the_title_closes_it() {
    let tracking_interval = cupertino_menu_model::tracking::TRACKING_INTERVAL;
    let mut c = controller();
    let title = slot_center(&c, &file());
    c.pointer_press(title, 0.0);
    let o = c.pointer_release(title, tracking_interval - 0.1);
    assert_eq!(
        (c.is_open(), c.mode(), activations(&o)),
        (true, Mode::Sticky, 0)
    );
    c.close();

    c.pointer_press(title, 10.0);
    let o = c.pointer_release(title, 10.0 + tracking_interval);
    assert!(
        !c.is_open() && has(&o, &Effect::Closed) && activations(&o) == 0,
        "held without reaching a row"
    );

    c.pointer_press(title, 20.0);
    let row = row_center(&c, 0, 10);
    let now = path(&mut c, title, row, 6, 20.0, 0.1);
    path(&mut c, row, title, 3, now, 0.05);
    let o = c.pointer_release(title, now + 0.2);
    assert!(
        !c.is_open() && activations(&o) == 0,
        "dragged onto a row and back"
    );
}

/// A Sort menu whose first row opens a tall submenu and whose third opens another: the way from the first into its
/// submenu crosses the rows below it.
fn nested() -> MenuController {
    let mut content = content();
    content.menus.push(MenuItem::submenu(
        4,
        "Sort",
        vec![
            MenuItem::submenu(
                40,
                "By Name",
                vec![
                    MenuItem::new(41, "Ascending"),
                    MenuItem::new(42, "Descending"),
                    MenuItem::new(43, "Natural"),
                    MenuItem::new(44, "Kind"),
                    MenuItem::new(45, "Size"),
                ],
            ),
            MenuItem::new(46, "Refresh"),
            MenuItem::submenu(
                47,
                "Group",
                vec![MenuItem::new(48, "None"), MenuItem::new(49, "Kind")],
            ),
            MenuItem::new(50, "Reload"),
        ],
    ));
    let mut c = MenuController::new(OUT, Box::new(MonoMeasure::default()));
    c.cfg.flash_choice = false;
    c.set_content(content);
    c
}

fn sort() -> SlotKey {
    SlotKey::Menu(ItemId(4))
}

/// Opens Sort with a click and By Name's submenu by resting on its row; the time then.
fn by_name_open(c: &mut MenuController) -> f64 {
    let title = slot_center(c, &sort());
    c.pointer_press(title, 0.0);
    c.pointer_release(title, 0.1);
    let row = row_center(c, 0, 40);
    let now = path(c, title, row, 4, 0.1, 0.05);
    c.tick(now + c.cfg.submenu_open_delay);
    assert_eq!(c.levels()[1].menu.id, ItemId(40));
    now + c.cfg.submenu_open_delay
}

/// The middle of the submenu's row `id`, on the side of it away from the parent.
fn far_in_submenu(c: &MenuController, id: i32) -> Point {
    let p = row_center(c, 1, id);
    let child = c.levels()[1].layout.rect;
    Point::new(child.x + child.w * 0.25, p.y)
}

#[test]
fn a_diagonal_into_the_submenu_keeps_it_across_the_rows_it_crosses() {
    let mut c = nested();
    let now = by_name_open(&mut c);
    let from = row_center(&c, 0, 40);
    let to = far_in_submenu(&c, 45);
    let parent = c.levels()[0].layout.rect;
    let child = c.levels()[1].layout.rect;
    let crossed = |p: Point| parent.contains(p) && !child.contains(p);
    let mut t = now;
    let mut over = Vec::new();
    for step in 1..=16 {
        let k = f64::from(step) / 16.0;
        let p = Point::new(from.x + (to.x - from.x) * k, from.y + (to.y - from.y) * k);
        t += 0.016;
        c.tick(t);
        c.pointer_move(p, t);
        if crossed(p) {
            if let Some(row) = c.levels()[0].layout.row_at(p) {
                over.push(c.levels()[0].layout.rows[row].item.id.0);
            }
            assert_eq!(
                selected_id(&c, 0),
                Some(40),
                "the highlight stays on By Name"
            );
            assert_eq!(c.levels()[1].menu.id, ItemId(40), "and its submenu open");
        }
    }
    assert!(
        over.contains(&46) && over.contains(&47),
        "the way crosses Refresh and Group: {over:?}"
    );
    assert_eq!(selected_id(&c, 1), Some(45), "Size, in the submenu");
    c.tick(t + 2.0);
    assert_eq!(c.levels()[1].menu.id, ItemId(40), "and it stays");
}

#[test]
fn resting_on_the_way_or_a_slow_path_gives_the_submenu_up_to_the_row_under_the_pointer() {
    // a rest on Group, halfway along the diagonal
    let mut c = nested();
    let now = by_name_open(&mut c);
    let from = row_center(&c, 0, 40);
    let group = row_center(&c, 0, 47);
    let rest = Point::new(from.x + (group.x - from.x) * 0.2 + 30.0, group.y);
    let t = path(&mut c, from, rest, 4, now, 0.016);
    assert_eq!(selected_id(&c, 0), Some(40), "still heading");
    let rested = t + cupertino_menu_model::tracking::SUBMENU_AIM_PAUSE;
    assert_eq!(c.next_deadline(), Some(rested));
    c.tick(rested);
    assert_eq!(selected_id(&c, 0), Some(47), "Group takes over");
    c.tick(rested + c.cfg.submenu_open_delay);
    assert_eq!(
        c.levels()[1].menu.id,
        ItemId(47),
        "and its submenu replaces By Name's"
    );

    // the same way, slowly: every move comes after the pause
    let mut c = nested();
    let now = by_name_open(&mut c);
    let to = far_in_submenu(&c, 45);
    let mut t = now;
    let mut switched = false;
    for step in 1..=16 {
        let k = f64::from(step) / 16.0;
        let p = Point::new(from.x + (to.x - from.x) * k, from.y + (to.y - from.y) * k);
        t += cupertino_menu_model::tracking::SUBMENU_AIM_PAUSE + 0.1;
        while let Some(due) = c.next_deadline().filter(|due| *due <= t) {
            c.tick(due);
        }
        c.pointer_move(p, t);
        if c.levels().len() > 1 && c.levels()[1].menu.id == ItemId(47) {
            switched = true;
            break;
        }
    }
    assert!(switched, "the slow way opened Group's submenu on the way");
}

#[test]
fn typed_letters_select_the_first_match_return_chooses_it_and_escape_closes_one_level_back_to_the_title(
) {
    let mut c = nested();
    c.focus_bar();
    for _ in 0..8 {
        if c.highlighted_slot() == Some(&sort()) {
            break;
        }
        c.key(Key::Right, 0.0);
    }
    assert_eq!(c.highlighted_slot(), Some(&sort()));
    c.key(Key::Down, 0.1);
    c.key(Key::Char('r'), 0.2);
    c.key(Key::Char('e'), 0.3);
    c.key(Key::Char('l'), 0.4);
    assert_eq!(selected_id(&c, 0), Some(50), "Reload, not Refresh");
    c.key(Key::Char('g'), 2.0);
    assert_eq!(selected_id(&c, 0), Some(47), "a new search after a pause");
    c.key(Key::Right, 2.1);
    assert_eq!(c.levels().len(), 2);
    c.key(Key::Char('k'), 2.2);
    assert_eq!(selected_id(&c, 1), Some(49));
    c.key(Key::Escape, 2.3);
    assert_eq!(c.levels().len(), 1, "Escape closes the submenu");
    assert_eq!(selected_id(&c, 0), Some(47), "back on its row");
    c.key(Key::Escape, 2.4);
    assert_eq!(
        (c.is_open(), c.mode(), c.highlighted_slot()),
        (false, Mode::TitleFocus, Some(&sort())),
        "then the menu, back to its title"
    );
    c.key(Key::Down, 3.0);
    c.key(Key::Char('r'), 3.5);
    let o = c.key(Key::Enter, 3.6);
    assert_eq!(activations(&o), 1);
    assert!(has(
        &o,
        &Effect::Activate {
            slot: sort(),
            item: ItemId(46)
        }
    ));
}
