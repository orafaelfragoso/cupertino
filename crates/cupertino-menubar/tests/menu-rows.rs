//! How a menu draws what an exporter says of its rows: a menu laid out right to left draws its labels at the right, a
//! warning row its label in orange and an alert row in the theme's destructive colour, and an item neither on nor off a
//! dash where a checked one has its check mark. Drawn headless with the CPU renderer, by the ink one row's look adds.

use std::rc::Rc;

use cupertino_kit::theme::Appearance;
use cupertino_menubar::controller::MenuController;
use cupertino_menubar::feed::Shared;
use cupertino_menubar::geom;
use cupertino_menubar::item::{Disposition, MenuItem, Toggle};
use cupertino_menubar::layout::{BarContent, BarMetrics, SlotKey};
use cupertino_menubar::view::{BarFont, KitMeasure, RowIcons};

use crate::glass::{drawn, text, Drawn, Scene};

/// The row whose look is compared, and a longer one that sets the menu's width, so every look has the same.
const ROW: i32 = 2;
const WIDE: &str = "A row wide enough to set the width";
/// A channel that differs by more than this is ink of the row's look.
const INK: u8 = 24;

/// The bar with its one menu open, whose first row is `row`.
fn scene(row: MenuItem) -> Scene {
    let text = text();
    let design = cupertino_kit_shell::design::menu_bar();
    let font = BarFont::of(&design);
    let measure = KitMeasure {
        text: Arc::clone(&text),
        font,
    };
    let output = geom::Rect::new(0.0, 0.0, 1440.0, 900.0);
    let mut controller = MenuController::new(output, Box::new(measure));
    controller.bar_metrics = BarMetrics {
        height: design.height,
        selection_corner_radius: design.selection_radius,
        selection_inset_y: BarMetrics::item_inset_y(design.height),
        ..BarMetrics::default()
    };
    let rtl = row.rtl;
    let mut menu = MenuItem::submenu(1, "File", vec![row, MenuItem::new(3, WIDE)]).exported();
    menu.for_each_mut(&mut |item| item.rtl = rtl);
    controller.set_content(BarContent {
        app_name: "Notes".into(),
        menus: vec![menu],
        ..BarContent::default()
    });
    let slot = controller
        .layout()
        .slot(&SlotKey::Menu(cupertino_menubar::item::ItemId(1)))
        .unwrap()
        .rect;
    let at = geom::Point::new(slot.x + slot.w / 2.0, slot.y + slot.h / 2.0);
    controller.pointer_press(at, 1.0);
    controller.pointer_release(at, 1.05);
    assert!(controller.is_open());
    Scene {
        controller,
        shared: Shared::default(),
        font,
        icons: Rc::new(RowIcons::default()),
        prompt: None,
    }
}

use std::sync::Arc;

/// The menu's window as drawn, and the rectangle of its first row in the window's points.
fn menu(row: MenuItem, appearance: Appearance) -> (Drawn, geom::Rect) {
    let scene = scene(row);
    let level = &scene.controller.levels()[0];
    let (r, at) = (level.layout.rows[0].rect, level.layout.rect);
    let rect = geom::Rect::new(r.x - at.x, r.y - at.y, r.w, r.h);
    let mut windows = drawn(text(), scene, appearance, false, true);
    assert_eq!(windows.len(), 2, "the bar and its menu");
    (windows.remove(1).1, rect)
}

/// The pixels of `rect` where `a` differs from `b`: `(x, y, rgba of a)` in the window's points.
fn ink(a: &Drawn, b: &Drawn, rect: geom::Rect) -> Vec<(f64, f64, [u8; 4])> {
    assert_eq!((a.width, a.size), (b.width, b.size), "the same menu size");
    let scale = f64::from(a.width) / a.size.width;
    let mut out = Vec::new();
    let (x0, y0) = ((rect.x * scale) as u32, (rect.y * scale) as u32);
    let (x1, y1) = (
        ((rect.x + rect.w) * scale) as u32,
        ((rect.y + rect.h) * scale) as u32,
    );
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * a.width + x) as usize * 4;
            let (pa, pb) = (&a.pixels[i..i + 4], &b.pixels[i..i + 4]);
            if pa.iter().zip(pb).any(|(p, q)| p.abs_diff(*q) > INK) {
                out.push((
                    f64::from(x) / scale,
                    f64::from(y) / scale,
                    [pa[0], pa[1], pa[2], pa[3]],
                ));
            }
        }
    }
    out
}

fn labelled(label: &str, rtl: bool) -> MenuItem {
    let mut item = MenuItem::new(ROW, label);
    item.rtl = rtl;
    item
}

#[test]
fn a_menu_laid_out_right_to_left_draws_its_labels_at_the_right() {
    for rtl in [false, true] {
        let (with, rect) = menu(labelled("Open", rtl), Appearance::Light);
        let (without, _) = menu(labelled("", rtl), Appearance::Light);
        let ink = ink(&with, &without, rect);
        assert!(!ink.is_empty(), "the label draws");
        let mean = ink.iter().map(|(x, _, _)| x).sum::<f64>() / ink.len() as f64;
        let middle = rect.x + rect.w / 2.0;
        assert_eq!(
            mean > middle,
            rtl,
            "label at {mean} of a row {rect:?}, rtl {rtl}"
        );
    }
}

/// The share of red over blue in the ink of the label of a row of `disposition`.
fn redness(disposition: Disposition, appearance: Appearance) -> f64 {
    let mut row = labelled("Erase All", false);
    row.disposition = disposition;
    let (with, rect) = menu(row, appearance);
    let (without, _) = menu(labelled("", false), appearance);
    let ink = ink(&with, &without, rect);
    let (red, blue) = ink.iter().fold((0.0, 0.0), |(r, b), (_, _, p)| {
        (r + f64::from(p[0]), b + f64::from(p[2]))
    });
    red / blue.max(1.0)
}

#[test]
fn a_warning_or_an_alert_row_draws_its_label_in_its_colour() {
    for appearance in [Appearance::Light, Appearance::Dark] {
        let normal = redness(Disposition::Normal, appearance);
        let informative = redness(Disposition::Informative, appearance);
        assert!(
            (normal - informative).abs() < 0.05,
            "{normal} {informative}"
        );
        for marked in [Disposition::Warning, Disposition::Alert] {
            let red = redness(marked, appearance);
            assert!(
                red > normal * 2.0,
                "{marked:?} in {appearance:?}: {red} against {normal}"
            );
        }
    }
}

/// How tall the mark of a row toggled `toggle` is, in points.
fn mark_height(toggle: Toggle) -> f64 {
    let (with, rect) = menu(
        labelled("Wrap", false).with_toggle(toggle),
        Appearance::Light,
    );
    let (without, _) = menu(
        labelled("Wrap", false).with_toggle(Toggle::Check(false)),
        Appearance::Light,
    );
    let ink = ink(&with, &without, rect);
    assert!(!ink.is_empty(), "{toggle:?} draws a mark");
    let top = ink.iter().map(|(_, y, _)| *y).fold(f64::MAX, f64::min);
    let bottom = ink.iter().map(|(_, y, _)| *y).fold(f64::MIN, f64::max);
    bottom - top
}

#[test]
fn an_item_neither_on_nor_off_shows_a_dash() {
    let check = mark_height(Toggle::Check(true));
    for mixed in [Toggle::Indeterminate, Toggle::RadioIndeterminate] {
        let dash = mark_height(mixed);
        assert!(
            dash < check / 2.0,
            "{mixed:?}: {dash} pt against the check's {check}"
        );
    }
}
