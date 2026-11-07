use super::*;
use crate::notification::NotifyInput;
use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::text::FontIndex;
use cupertino_kit::theme::{Appearance, ThemePreferences};
use cupertino_theme::Theme;

const WIDTH: f64 = 344.0;

fn metrics() -> CardMetrics {
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
    let preferences = ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency: false,
        reduce_motion: false,
    };
    let theme = ThemeTokens::cupertino(&Theme::from_dir(data).unwrap(), preferences).unwrap();
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    CardMetrics::new(
        Arc::new(theme),
        Arc::new(TextSystem::new(fonts, "Inter Variable")),
    )
}

/// How many lines `content` takes in the card's body column, as Kit's wrapping text lays it out.
fn lines(m: &CardMetrics, content: &AttributedText, width: f64) -> usize {
    m.wrapped(content, m.theme.type_scale.body, width, None)
        .unwrap()
        .lines()
        .len()
}

fn notification(summary: &str, body: &str) -> Notification {
    let mut c = NotificationCenter::default();
    let id = c.notify(NotifyInput::new("app", summary, body), 0.0);
    c.get(id).unwrap().clone()
}

fn long(words: usize) -> String {
    (0..words)
        .map(|i| format!("word{i}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn a_card_is_as_tall_as_its_text_and_never_shorter_than_its_icon() {
    let m = metrics();
    let title = m.card(&notification("Title", ""), WIDTH, Fold::Banner);
    assert_eq!(
        title.height,
        2.0 * PADDING + ICON_SIZE,
        "one line is shorter than the icon"
    );
    assert!(title.body.is_none());
    let column = CardMetrics::column_width(WIDTH);
    let body = m.card(&notification("Title", "Body"), WIDTH, Fold::Banner);
    let role = m.theme.type_scale.body;
    let line = |text: &str, weight| {
        m.wrapped(&plain(text, weight), role, column, None)
            .unwrap()
            .size()
            .height
    };
    let text = line("Title", FontWeight::SEMIBOLD) + LINE_GAP + line("Body", FontWeight::REGULAR);
    assert_eq!(body.height, (2.0 * PADDING + text.max(ICON_SIZE)).ceil());
    let mut c = NotificationCenter::default();
    let id = c.notify(
        NotifyInput::new("app", "Title", "Body").with_action("reply", "Reply"),
        0.0,
    );
    let actions = m.card(c.get(id).unwrap(), WIDTH, Fold::Banner);
    assert_eq!(
        actions.height,
        (2.0 * PADDING + text + BUTTON_TOP_GAP + BUTTON_ROW).ceil()
    );
    assert_eq!(actions.actions, vec![("reply".into(), "Reply".into())]);
}

#[test]
fn a_long_body_wraps_and_stops_at_its_line_limit_with_an_ellipsis() {
    let m = metrics();
    let column = CardMetrics::column_width(WIDTH);
    let two = m.card(&notification("Title", &long(8)), WIDTH, Fold::Banner);
    let body = two.body.as_ref().unwrap();
    assert!(
        !body.content.text.ends_with(ELLIPSIS),
        "a body that fits is whole"
    );
    assert!(
        lines(&m, &body.content, column) > 1,
        "it wraps instead of ending at one line"
    );

    let long_card = m.card(&notification("Title", &long(200)), WIDTH, Fold::Banner);
    let cut = long_card.body.as_ref().unwrap();
    assert!(
        cut.content.text.ends_with(ELLIPSIS),
        "{:?}",
        cut.content.text
    );
    assert_eq!(lines(&m, &cut.content, column), BODY_LINES as usize);
    assert!(!long_card.foldable, "a banner has no disclosure button");
    let longer = m.card(&notification("Title", &long(2000)), WIDTH, Fold::Banner);
    assert_eq!(
        longer.height, long_card.height,
        "a longer body does not grow the card"
    );
}

#[test]
fn a_centre_card_unfolds_to_more_lines_and_no_further_than_its_limit() {
    let m = metrics();
    let column = CardMetrics::column_width(WIDTH);
    let n = notification("Title", &long(60));
    let folded = m.card(&n, WIDTH, Fold::Folded);
    assert!(folded.foldable);
    let unfolded = m.card(&n, WIDTH, Fold::Unfolded);
    let all = &unfolded.body.as_ref().unwrap().content;
    assert!(!all.text.ends_with(ELLIPSIS), "sixty words fit unfolded");
    assert!(lines(&m, all, column) > BODY_LINES as usize);
    assert!(unfolded.height > folded.height);
    assert_eq!(
        folded.summary_width,
        column - m.disclosure_side() - BUTTON_GAP,
        "the summary leaves room for the disclosure button"
    );

    let huge = notification("Title", &long(3000));
    let unfolded = m.card(&huge, WIDTH, Fold::Unfolded);
    let shown = &unfolded.body.as_ref().unwrap().content;
    assert!(shown.text.ends_with(ELLIPSIS));
    assert_eq!(lines(&m, shown, column), UNFOLDED_BODY_LINES as usize);
    let short = m.card(&notification("Title", "Short"), WIDTH, Fold::Folded);
    assert!(!short.foldable, "a body that fits has no disclosure button");
}

#[test]
fn line_breaks_of_the_body_are_kept_and_count_as_lines() {
    let m = metrics();
    let column = CardMetrics::column_width(WIDTH);
    let card = m.card(
        &notification("Title", "one\ntwo\r\nthree\nfour\nfive\nsix"),
        WIDTH,
        Fold::Banner,
    );
    let body = &card.body.as_ref().unwrap().content;
    assert!(
        body.text.starts_with("one\ntwo\nthree\nfour"),
        "{:?}",
        body.text
    );
    assert!(body.text.ends_with(ELLIPSIS));
    assert_eq!(lines(&m, body, column), BODY_LINES as usize);
}

#[test]
fn the_summary_wraps_to_two_lines() {
    let m = metrics();
    let column = CardMetrics::column_width(WIDTH);
    let card = m.card(&notification(&long(40), ""), WIDTH, Fold::Banner);
    assert!(card.summary.text.ends_with(ELLIPSIS));
    assert_eq!(lines(&m, &card.summary, column), SUMMARY_LINES as usize);
}

#[test]
fn a_huge_body_is_bounded_before_it_is_laid_out() {
    let body = "x".repeat(MAX_BODY_BYTES * 20) + "é";
    let n = notification("Title", &body);
    assert_eq!(body_text(&n).unwrap().content.text.len(), MAX_BODY_BYTES);
    let multibyte = "é".repeat(MAX_BODY_BYTES);
    let n = notification("Title", &multibyte);
    let bounded = body_text(&n).unwrap().content.text;
    assert!(bounded.len() <= MAX_BODY_BYTES && bounded.len() > MAX_BODY_BYTES - 2);
}

#[test]
fn links_past_the_cut_are_dropped_and_one_across_it_is_shortened() {
    let m = metrics();
    let n = notification(
        "Title",
        &format!(
            "<a href=\"https://one.example\">first</a> {} <a href=\"https://two.example\">{}</a>",
            long(10),
            long(400)
        ),
    );
    let card = m.card(&n, WIDTH, Fold::Banner);
    let body = card.body.unwrap();
    assert_eq!(body.links.len(), 2, "{:?}", body.links);
    let far = m.card(
        &notification(
            "Title",
            &format!("{} <a href=\"https://far.example\">far</a>", long(400)),
        ),
        WIDTH,
        Fold::Banner,
    );
    assert!(
        far.body.unwrap().links.is_empty(),
        "a link past the cut is dropped"
    );
    assert_eq!(body.links[0], (0..5, "https://one.example".into()));
    let (cut, href) = &body.links[1];
    assert_eq!(href, "https://two.example");
    assert!(
        cut.end < body.content.text.len(),
        "the ellipsis is no part of the link"
    );
}

#[test]
fn action_labels_that_do_not_fit_share_the_row() {
    let m = metrics();
    let mut c = NotificationCenter::default();
    let label = long(6);
    let id = c.notify(
        NotifyInput::new("app", "Title", "Body")
            .with_action("a", &label)
            .with_action("b", &label)
            .with_action("c", "OK"),
        0.0,
    );
    let card = m.card(c.get(id).unwrap(), WIDTH, Fold::Banner);
    let column = CardMetrics::column_width(WIDTH);
    let share = (column - 2.0 * BUTTON_GAP) / 3.0;
    let inset = m.theme.metrics.button_inset;
    for (_, label) in &card.actions {
        let width = m
            .wrapped(
                &plain(label, FontWeight::REGULAR),
                m.theme.type_scale.body,
                f64::INFINITY,
                None,
            )
            .unwrap()
            .size()
            .width;
        assert!(
            width + 2.0 * inset <= share + 0.5,
            "{label:?} is {width} wide"
        );
    }
    assert!(card.actions[0].1.ends_with(ELLIPSIS));
    assert_eq!(
        card.actions[2].1, "OK",
        "a label that fits its share is whole"
    );
}

#[test]
fn the_close_button_lies_inside_the_card_s_rounded_corner() {
    let centre = CLOSE_INSET + CLOSE_DIAMETER / 2.0;
    let corner = CORNER_RADIUS;
    let reach = ((centre - corner).powi(2) * 2.0).sqrt() + CLOSE_DIAMETER / 2.0;
    assert!(
        reach <= corner,
        "{reach} reaches past the curve of radius {corner}"
    );
}

#[test]
fn the_strip_holds_the_card_its_margin_the_swipe_travel_and_the_shadow() {
    let width = strip_width(&StackLayout::default(), &SwipeConfig::default());
    assert_eq!(width, 344.0 + 16.0 + 16.0 + SHADOW_MARGIN);
}

#[test]
fn the_centre_replaces_the_banners_and_closing_it_brings_them_back() {
    let m = metrics();
    let mut c = NotificationCenter::default();
    c.notify(NotifyInput::new("app", "Hello", "World"), 0.0);
    let mut ui = Ui::default();
    let banners = windows::<Input>(&mut c, &ui, &m, 1.0, |i| i);
    assert_eq!(banners.len(), 1);
    assert_eq!(banners[0].spec.title, "app: Hello");
    ui.centre_open = true;
    let centre = windows::<Input>(&mut c, &ui, &m, 1.0, |i| i);
    assert_eq!(centre.len(), 1);
    assert_eq!(centre[0].spec.title, CENTRE_TITLE);
    assert_eq!(apply(&mut c, &mut ui, Input::CloseCentre, 1.0), None);
    assert!(!ui.centre_open);
}

#[test]
fn escape_closes_the_clear_menu_before_the_centre_and_closing_forgets_the_list_s_state() {
    let mut c = NotificationCenter::default();
    let mut ui = Ui {
        centre_open: true,
        menu_open: true,
        ..Ui::default()
    };
    apply(&mut c, &mut ui, Input::Unfold(7, true), 0.0);
    apply(&mut c, &mut ui, Input::CloseCentre, 0.0);
    assert!(ui.centre_open && !ui.menu_open);
    assert!(ui.unfolded.contains(&7));
    apply(&mut c, &mut ui, Input::CloseCentre, 0.0);
    assert_eq!(ui, Ui::default());
}

#[test]
fn the_list_scrolls_by_whole_points_and_a_card_folds_back() {
    let mut c = NotificationCenter::default();
    let mut ui = Ui::default();
    let moved = ScrollState {
        offset: Point::new(0.0, 40.6),
        content: Size::new(388.0, 900.0),
        viewport: Size::new(388.0, 500.0),
    };
    apply(&mut c, &mut ui, Input::Scroll(moved), 0.0);
    assert_eq!(ui.scroll.offset, Point::new(0.0, 41.0));
    assert_eq!(ui.scroll.content, moved.content);
    apply(&mut c, &mut ui, Input::Unfold(3, true), 0.0);
    assert!(ui.unfolded.contains(&3));
    apply(&mut c, &mut ui, Input::Unfold(3, false), 0.0);
    assert!(ui.unfolded.is_empty());
}

#[test]
fn a_press_that_never_travels_is_a_click_and_activates() {
    let m = metrics();
    let mut c = NotificationCenter::default();
    let id = c.notify(NotifyInput::new("app", "Hello", "World"), 0.0);
    let mut ui = Ui::default();
    windows::<Input>(&mut c, &ui, &m, 1.0, |i| i);
    assert_eq!(apply(&mut c, &mut ui, Input::SwipeBegin(id), 1.0), None);
    assert_eq!(
        apply(&mut c, &mut ui, Input::SwipeEnd(id), 1.1),
        Some(HostAction::Activate(id))
    );
}
