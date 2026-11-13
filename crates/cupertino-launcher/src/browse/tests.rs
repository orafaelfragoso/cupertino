use super::*;

fn app(id: &str, name: &str, categories: &[&str]) -> App {
    App {
        id: format!("{id}.desktop"),
        name: name.into(),
        icon: None,
        categories: Category::set_of(categories),
    }
}

/// `n` applications named `App 000` … with no category.
fn apps(n: usize) -> Vec<App> {
    (0..n)
        .map(|i| app(&format!("app{i:03}"), &format!("App {i:03}"), &[]))
        .collect()
}

fn names(b: &Browser) -> Vec<String> {
    b.items().map(|a| a.name.clone()).collect()
}

fn sample() -> Browser {
    Browser::new(
        vec![
            app(
                "org.example.Terminal",
                "Terminal",
                &["System", "TerminalEmulator"],
            ),
            app("browser", "Web Browser", &["Network", "WebBrowser"]),
            app("editor", "Text Editor", &["Utility", "TextEditor"]),
            app("paint", "Paint", &["Graphics"]),
            app("mail", "Mail", &["Network", "Email"]),
            app("misc", "Misc", &[]),
        ],
        &["mail".into(), "paint.desktop".into(), "gone".into()],
        BrowseLayout::HEIGHT,
    )
}

#[test]
fn categories_map_to_filters_and_an_entry_without_one_is_other() {
    assert_eq!(Category::of("WebBrowser"), Some(Category::Internet));
    assert_eq!(Category::of("TextEditor"), Some(Category::Utilities));
    assert_eq!(Category::of("Settings"), Some(Category::System));
    assert_eq!(Category::of("GTK"), None);
    let none: [&str; 0] = [];
    assert_eq!(Category::set_of(&none), Category::Other.bit());
    let both = Category::set_of(&["Network", "Graphics", "GTK"]);
    assert_eq!(both, Category::Internet.bit() | Category::Creativity.bit());
    let b = sample();
    // only the categories that hold an application, in their order
    assert_eq!(
        b.categories(),
        [
            Category::Creativity,
            Category::Internet,
            Category::Utilities,
            Category::System,
            Category::Other
        ]
    );
    assert_eq!(b.apps()[0].first_category(), Category::Internet, "Mail");
}

#[test]
fn the_recents_come_first_then_every_application_by_name() {
    let b = sample();
    assert!(b.shows_recents());
    assert_eq!(b.recents_shown(), 2, "an unknown recent is skipped");
    assert_eq!(
        names(&b),
        [
            "Mail",
            "Paint",
            "Mail",
            "Misc",
            "Paint",
            "Terminal",
            "Text Editor",
            "Web Browser"
        ]
    );
    assert_eq!(
        b.separator_y(),
        Some(BrowseLayout::ROW_HEIGHT + BrowseLayout::SEPARATOR_LINE)
    );
    // the list arrangement puts the recents one per row
    let mut b = b;
    b.set_arrangement(Arrangement::List);
    assert_eq!(
        b.separator_y(),
        Some(2.0 * BrowseLayout::LIST_ROW_HEIGHT + BrowseLayout::SEPARATOR_LINE)
    );
}

#[test]
fn a_category_filters_and_a_second_press_shows_everything_again() {
    let mut b = sample();
    b.toggle_category(Category::Internet);
    assert_eq!(b.category(), Some(Category::Internet));
    assert!(!b.shows_recents(), "a filter hides the recents");
    assert_eq!(names(&b), ["Mail", "Web Browser"]);
    b.toggle_category(Category::Internet);
    assert_eq!(b.category(), None);
    assert_eq!(b.result_count(), 6);
    // a category with no application cannot be turned on
    b.set_category(Some(Category::Games));
    assert_eq!(b.category(), None);
}

#[test]
fn typing_filters_by_name_initials_and_id_with_diacritics_folded() {
    let mut b = sample();
    b.set_query("te");
    assert_eq!(names(&b), ["Terminal", "Text Editor"]);
    b.set_query("wb");
    assert_eq!(names(&b), ["Web Browser"], "initials");
    b.set_query("example");
    assert_eq!(names(&b), ["Terminal"], "the desktop id read as words");
    b.set_query("téx");
    assert_eq!(names(&b), ["Text Editor"]);
    b.set_query("zzz");
    assert_eq!(b.item_count(), 0);
    assert!(matches("QuickView", "view"), "camel-case segment");
    assert!(matches("Text Editor", "texted"), "space-stripped prefix");
    assert!(!matches("Text Editor", "xt"));
}

#[test]
fn the_arrows_walk_the_grid_and_cross_from_the_recents_to_the_list_and_the_filters() {
    let mut b = Browser::new(
        apps(20),
        &["app003".into(), "app001".into()],
        BrowseLayout::HEIGHT,
    );
    assert_eq!(b.selected(), None);
    b.key(Key::Right, 0.0);
    assert_eq!(
        b.selected(),
        Some(0),
        "the first arrow selects the first item"
    );
    b.key(Key::Right, 0.0);
    assert_eq!(b.selected_app().unwrap().name, "App 001");
    b.key(Key::Right, 0.0);
    assert_eq!(
        b.selected(),
        Some(2),
        "Right crosses from the recents row into the list"
    );
    b.key(Key::Down, 0.0);
    assert_eq!(b.selected(), Some(2 + COLUMNS), "Down keeps the column");
    b.key(Key::Up, 0.0);
    b.key(Key::Up, 0.0);
    assert_eq!(
        b.selected(),
        Some(0),
        "the recents row is above, short of the column"
    );
    b.key(Key::Up, 0.0);
    assert_eq!(
        b.zone(),
        Zone::Categories,
        "above the first row are the filters"
    );
    assert_eq!(b.category_focus(), Some(0));
    b.key(Key::Right, 0.0);
    b.key(Key::Return, 0.0);
    // every app is Other: one filter only, the focus stays on it
    assert_eq!(b.category(), Some(Category::Other));
    b.key(Key::Down, 0.0);
    assert_eq!(b.zone(), Zone::Content);
    assert_eq!(b.selected(), Some(0));
    b.key(Key::End, 0.0);
    assert_eq!(b.selected(), Some(19));
    assert_eq!(
        b.key(Key::Return, 0.0),
        Some(Outcome::Launch("app019.desktop".into()))
    );
}

#[test]
fn in_the_list_up_and_down_move_one_row_and_left_right_do_nothing() {
    let mut b = Browser::new(apps(30), &[], BrowseLayout::HEIGHT);
    b.set_arrangement(Arrangement::List);
    b.key(Key::Down, 0.0);
    b.key(Key::Down, 0.0);
    assert_eq!(b.selected(), Some(1));
    b.key(Key::Right, 0.0);
    b.key(Key::Left, 0.0);
    assert_eq!(b.selected(), Some(1));
    b.key(Key::PageDown, 0.0);
    let page = (b.layout().content.height() / BrowseLayout::LIST_ROW_HEIGHT).floor() as usize;
    assert_eq!(b.selected(), Some(1 + page));
    let cell = b.cell_of(b.selected().unwrap()).unwrap();
    assert!(
        cell.min_y() >= b.layout().content.min_y() - 0.5
            && cell.max_y() <= b.layout().content.max_y() + 0.5,
        "the selection scrolled into view"
    );
}

#[test]
fn return_with_nothing_selected_opens_the_first_match_of_a_query() {
    let mut b = sample();
    assert_eq!(b.key(Key::Return, 0.0), None, "no query, nothing selected");
    b.set_query("pa");
    assert_eq!(
        b.key(Key::Return, 0.0),
        Some(Outcome::Launch("paint.desktop".into()))
    );
}

#[test]
fn the_selection_follows_its_application_when_the_set_changes() {
    let mut b = Browser::new(apps(10), &[], BrowseLayout::HEIGHT);
    b.key(Key::Down, 0.0);
    b.key(Key::Right, 0.0);
    b.key(Key::Right, 0.0);
    assert_eq!(b.selected_app().unwrap().id, "app002.desktop");
    // an application sorted before it is installed: the selection moves with its application
    let mut more = apps(10);
    more.push(app("aaa", "AAA", &[]));
    b.set_apps(more);
    assert_eq!(b.selected_app().unwrap().id, "app002.desktop");
    assert_eq!(b.selected(), Some(3));
    // the selected application is removed: nothing is selected
    let fewer: Vec<App> = apps(10)
        .into_iter()
        .filter(|a| a.id != "app002.desktop")
        .collect();
    b.set_apps(fewer);
    assert_eq!(b.selected(), None);
}

#[test]
fn clearing_a_filter_puts_back_the_scroll_of_the_full_list() {
    let mut list = apps(200);
    list.push(app("zgame", "Zed Game", &["Game"]));
    let mut b = Browser::new(list, &[], BrowseLayout::HEIGHT);
    b.scroll_by(1234.0, 1.0);
    let place = b.scroll();
    assert!(place > 0.0);
    b.set_category(Some(Category::Games));
    assert_eq!(b.scroll(), 0.0, "a filter starts at the top");
    b.set_category(None);
    assert_eq!(b.scroll(), place, "the full list is back where it was");
    b.set_query("app 1");
    assert_eq!(b.scroll(), 0.0);
    b.set_query("");
    assert_eq!(b.scroll(), place);
}

#[test]
fn switching_to_the_list_keeps_the_selection_in_view() {
    let mut b = Browser::new(apps(200), &[], BrowseLayout::HEIGHT);
    for _ in 0..12 {
        b.key(Key::Down, 0.0);
    }
    let selected = b.selected_app().unwrap().id.clone();
    b.set_arrangement(Arrangement::List);
    assert_eq!(b.selected_app().unwrap().id, selected);
    let cell = b.cell_of(b.selected().unwrap()).unwrap();
    let content = b.layout().content;
    assert!(cell.min_y() >= content.min_y() - 0.5 && cell.max_y() <= content.max_y() + 0.5);
    b.set_arrangement(Arrangement::Grid);
    let cell = b.cell_of(b.selected().unwrap()).unwrap();
    assert!(cell.min_y() >= content.min_y() - 0.5 && cell.max_y() <= content.max_y() + 0.5);
}

/// The selected row the keys scroll to stays inset from the panel's bottom edge as the list is from its sides, the
/// last row too, in either arrangement.
#[test]
fn a_row_scrolled_into_view_keeps_the_bottom_inset() {
    for arrangement in [Arrangement::List, Arrangement::Grid] {
        let mut b = Browser::new(apps(60), &[], BrowseLayout::HEIGHT);
        b.set_arrangement(arrangement);
        let bottom = b.layout().panel.max_y() - BrowseLayout::INSET;
        let inset = |b: &Browser| bottom - b.cell_of(b.selected().unwrap()).unwrap().max_y();
        b.set_zone(Zone::Content);
        for _ in 0..12 {
            b.key(Key::Down, 0.0);
        }
        assert!(b.scroll() > 0.0, "{arrangement:?}: the keys scrolled");
        assert!(
            inset(&b).abs() < 0.5,
            "{arrangement:?}: a row the keys reached sits on the inset, {}",
            inset(&b)
        );
        b.key(Key::End, 0.0);
        assert_eq!(b.selected(), Some(59));
        assert!((b.scroll() - b.max_scroll()).abs() < 1e-9);
        assert!(
            inset(&b).abs() < 0.5,
            "{arrangement:?}: the last row sits on the inset, {}",
            inset(&b)
        );
    }
}

#[test]
fn only_rows_in_view_are_placed_even_for_thousands_of_applications() {
    let mut b = Browser::new(apps(5000), &[], BrowseLayout::HEIGHT);
    let rows = (b.layout().content.height() / BrowseLayout::ROW_HEIGHT).ceil() as usize + 1;
    assert!(b.visible().len() <= rows * COLUMNS);
    b.key(Key::End, 0.0);
    assert_eq!(b.selected(), Some(4999));
    assert!((b.scroll() - b.max_scroll()).abs() < 1e-9);
    assert!(b.visible().iter().any(|p| p.index == 4999));
    assert!(b.nearby().len() <= 3 * rows * COLUMNS);
    let thumb = b.scroller_thumb().unwrap();
    assert!(thumb.height() >= BrowseLayout::SCROLLER_MIN);
}

#[test]
fn the_scroll_indicator_shows_while_scrolling_then_fades() {
    let mut b = Browser::new(apps(100), &[], BrowseLayout::HEIGHT);
    assert_eq!(b.scroller_alpha(0.0), 0.0);
    b.scroll_by(50.0, 0.0);
    assert_eq!(b.scroller_alpha(SCROLLER_LINGER), 1.0);
    assert!(b.animating(SCROLLER_LINGER));
    assert_eq!(b.scroller_alpha(SCROLLER_LINGER + SCROLLER_FADE), 0.0);
    assert!(!b.animating(SCROLLER_LINGER + SCROLLER_FADE));
}

#[test]
fn the_scroll_indicator_shows_when_the_browser_comes_into_view_then_fades() {
    let mut b = Browser::new(apps(100), &[], BrowseLayout::HEIGHT);
    b.reveal_scroller(10.0);
    assert_eq!(b.scroll(), 0.0, "nothing scrolls");
    assert_eq!(b.scroller_alpha(10.0), 1.0);
    assert!(b.scroller_thumb().is_some());
    assert!(b.animating(10.0 + SCROLLER_LINGER));
    assert_eq!(
        b.scroller_alpha(10.0 + SCROLLER_LINGER + SCROLLER_FADE),
        0.0
    );
}

#[test]
fn clicks_open_items_and_toggle_filters() {
    let mut b = sample();
    let (c, r) = b.filter_rects()[1];
    assert_eq!(b.click(r.center()), None);
    assert_eq!(b.category(), Some(c));
    let first = b.visible()[0].clone();
    assert_eq!(
        b.click(first.cell.center()),
        Some(Outcome::Launch(b.item(0).unwrap().id.clone()))
    );
    // the header is not the content
    assert_eq!(b.click(Point::new(100.0, 20.0)), None);
}

#[test]
fn the_filters_fill_their_row_and_narrow_when_they_do_not_fit() {
    let l = BrowseLayout::new(BrowseLayout::HEIGHT);
    // the row: 22 pt in from each side, 67 pt below the panel's top, 26 pt tall
    assert_eq!(l.categories, Rect::new(22.0, 67.0, 800.0, 26.0));
    let fills = |rects: &[Rect]| {
        assert!((rects[0].x() - l.categories.x()).abs() < 1e-9);
        let right = rects.last().unwrap().max_x();
        assert!((right - l.categories.max_x()).abs() < 1e-6, "{right}");
    };
    // all ten: every name keeps room for its text, the gaps closing in before anything is cut
    let rects = l.filter_rects(&Category::ALL);
    assert_eq!(rects.len(), 10);
    fills(&rects);
    for (c, r) in Category::ALL.iter().zip(&rects) {
        assert!(
            r.width() >= BrowseLayout::filter_text_width(*c) + 2.0 * BrowseLayout::FILTER_MIN_PAD,
            "{c:?} {r:?}"
        );
    }
    let gap = rects[1].x() - rects[0].max_x();
    assert!((BrowseLayout::FILTER_MIN_GAP..=BrowseLayout::FILTER_GAP).contains(&gap));
    // seven: 12 pt apart, the row filled by sharing the width left over evenly, however much there is
    let seven = &Category::ALL[..7];
    let rects = l.filter_rects(seven);
    fills(&rects);
    let gap = rects[1].x() - rects[0].max_x();
    assert!((gap - BrowseLayout::FILTER_GAP).abs() < 1e-9);
    // two short names: each its least width, grown by half the row's leftover, uncapped
    let two = l.filter_rects(&[Category::Media, Category::Games]);
    fills(&two);
    assert!((two[0].width() - two[1].width()).abs() < 1e-9);
    assert!(two[0].width() > 300.0, "{two:?}");
}

#[test]
fn the_panel_shrinks_on_a_short_output_and_keeps_its_least_height() {
    assert_eq!(
        BrowseLayout::height_on(1000.0, 200.0, 96.0),
        BrowseLayout::HEIGHT
    );
    assert_eq!(BrowseLayout::height_on(700.0, 140.0, 96.0), 464.0);
    assert_eq!(
        BrowseLayout::height_on(400.0, 80.0, 96.0),
        BrowseLayout::MIN_HEIGHT
    );
    let mut b = Browser::new(apps(100), &[], BrowseLayout::HEIGHT);
    b.key(Key::End, 0.0);
    b.set_height(464.0);
    assert!(b.scroll() <= b.max_scroll());
    assert_eq!(b.layout().content.max_y(), 464.0);
}

#[test]
fn tab_walks_the_applications_the_filters_and_the_view_options() {
    let mut b = sample();
    assert_eq!(b.zone(), Zone::Content);
    b.key(Key::Tab, 0.0);
    assert_eq!(b.zone(), Zone::Categories);
    b.key(Key::Tab, 0.0);
    assert_eq!(b.zone(), Zone::Options);
    assert_eq!(b.key(Key::Return, 0.0), Some(Outcome::ViewOptions));
    b.key(Key::Tab, 0.0);
    assert_eq!(b.zone(), Zone::Content);
    assert_eq!(
        b.selected(),
        Some(0),
        "coming back to the applications selects the first"
    );
    b.key(Key::BackTab, 0.0);
    assert_eq!(b.zone(), Zone::Options);
    // without filters, Tab skips their row
    let mut bare = Browser::new(Vec::new(), &[], BrowseLayout::HEIGHT);
    bare.key(Key::Tab, 0.0);
    assert_eq!(bare.zone(), Zone::Options);
}
