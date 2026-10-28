use cupertino_menubar::controller::{
    point_in_triangle, Effect, Key, MenuController, Mode, APP_MENU_ROOT,
};
use cupertino_menubar::fallback::{HELP_MENU, HELP_SEARCH};
use cupertino_menubar::geom::{Point, Rect};
use cupertino_menubar::item::{ItemId, MenuItem, Shortcut, Toggle};
use cupertino_menubar::layout::{BarContent, MonoMeasure, SlotKey, StatusEntry};
use cupertino_text_field::{KeyInput, Keymap};
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

fn controller() -> MenuController {
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
    assert!(c.tick(1.1).effects.is_empty());
    let o = c.tick(1.21);
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
    c.pointer_move(row_center(&c, 1, 13), 1.3);
    assert_eq!(selected_id(&c, 1), Some(13));
    let o = c.pointer_press(row_center(&c, 1, 13), 1.4);
    assert!(o.consumed);
    let o = c.pointer_release(row_center(&c, 1, 13), 1.5);
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
fn moving_to_another_row_closes_the_submenu_after_the_delay() {
    let mut c = controller();
    let p = slot_center(&c, &file());
    c.pointer_press(p, 0.0);
    c.pointer_release(p, 0.1);
    c.pointer_move(row_center(&c, 0, 11), 1.0);
    c.tick(1.3);
    assert_eq!(c.levels().len(), 2);
    c.pointer_move(row_center(&c, 0, 16), 2.0);
    assert_eq!(c.levels().len(), 2, "closes after the delay, not at once");
    c.tick(2.0 + c.cfg.submenu_close_delay + 0.01);
    assert_eq!(c.levels().len(), 1);
    assert_eq!(selected_id(&c, 0), Some(16));
}

#[test]
fn safe_triangle_keeps_the_submenu_while_crossing_other_rows() {
    let mut c = controller();
    let p = slot_center(&c, &file());
    c.pointer_press(p, 0.0);
    c.pointer_release(p, 0.1);
    let row = row_center(&c, 0, 11);
    c.pointer_move(row, 1.0);
    c.tick(1.3);
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
    c.set_content(content());
    let p = slot_center(&c, &edit());
    c.pointer_press(p, 0.0);
    let r = c.levels()[0].layout.rect;
    assert!(r.x >= right.x && r.right() <= right.right() && r.y >= 24.0 - 1e-9);
}

fn help() -> SlotKey {
    SlotKey::Menu(HELP_MENU)
}

/// The bar of [`content`] with a Help menu, the Help menu open from a click.
fn help_open() -> MenuController {
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
    c.set_content(content);
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
    assert_eq!(row_ids(&c)[..4], [-1030, -1050, -1051, -1052]);
    let p = row_center(&c, 0, -1051);
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
