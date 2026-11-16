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

/// The words a person reads and hears for the list: its window's title, its heading and its accessible name are one
/// string, "Notification Center" (the semantics goldens of `tests/kit_goldens.rs` hold the heading and the name).
#[test]
fn the_centre_s_window_title_reads_notification_center() {
    assert_eq!(CENTRE_TITLE, "Notification Center");
    let m = metrics();
    let mut c = NotificationCenter::default();
    c.notify(NotifyInput::new("app", "Hello", "World"), 0.0);
    let ui = Ui {
        centre_open: true,
        ..Ui::default()
    };
    let centre = windows::<Input>(&mut c, &ui, &m, 1.0, |i| i);
    assert_eq!(centre[0].spec.title, "Notification Center");
}

/// The open centre has a window over each output that hears a press outside it, or one where the compositor chooses
/// while no output is known. A centre that is closed, sliding out or behind the lock screen has none.
#[test]
fn the_open_centre_has_a_press_catcher_over_each_output() {
    let mut c = NotificationCenter::default();
    c.notify(NotifyInput::new("app", "Hello", "World"), 0.0);
    let mut ui = Ui::default();
    let outputs = |c: &NotificationCenter, ui: &Ui, outputs: &[u32]| -> Vec<Option<u32>> {
        catchers::<Input>(c, ui, outputs, |i| i)
            .iter()
            .map(|w| catcher_output(&w.spec.app_id).expect("a catcher's app_id"))
            .collect()
    };
    assert_eq!(outputs(&c, &ui, &[3, 7]), [], "closed");
    ui.open_centre(1.0, false);
    assert_eq!(outputs(&c, &ui, &[3, 7]), [Some(3), Some(7)]);
    assert_eq!(outputs(&c, &ui, &[]), [None], "no output known yet");
    let shown = catchers::<Input>(&c, &ui, &[3, 7], |i| i);
    assert_ne!(shown[0].key, shown[1].key);
    for window in &shown {
        assert_eq!(window.spec.size, Size::ZERO, "its surface's role sizes it");
        assert_eq!(window.spec.input_region, None, "all of it takes input");
        assert_eq!(window.background, WindowBackground::INPUT_ONLY);
        assert_ne!(window.spec.app_id, APP_ID, "no card window");
    }
    c.set_screen_locked(true, 1.5);
    assert_eq!(outputs(&c, &ui, &[3, 7]), [], "locked");
    c.set_screen_locked(false, 1.6);
    ui.close_centre(2.0, false);
    assert!(ui.centre_shown(2.0), "still sliding out");
    assert_eq!(outputs(&c, &ui, &[3, 7]), [], "closing");

    assert_eq!(catcher_output(APP_ID), None);
    assert_eq!(catcher_output("cupertino-notifications.catcher.x"), None);
    assert_eq!(catcher_output("cupertino-notifications.catchers"), None);
}

/// A press outside closes the centre and its Clear menu in one go (Escape closes the menu first); the window of one
/// output going is the host's to note and closes nothing, and the last one going closes the centre.
#[test]
fn a_press_outside_closes_the_centre_with_its_menu_and_a_lost_output_does_not() {
    let mut c = NotificationCenter::default();
    c.notify(NotifyInput::new("app", "Hello", "World"), 0.0);
    let mut ui = Ui::default();
    ui.open_centre(1.0, true);
    assert_eq!(apply(&mut c, &mut ui, Input::ToggleClearMenu, 1.0), None);
    assert_eq!(apply(&mut c, &mut ui, Input::CloseCentre, 1.0), None);
    assert!(ui.centre_open && !ui.menu_open, "Escape: the menu first");
    assert_eq!(apply(&mut c, &mut ui, Input::ToggleClearMenu, 1.0), None);
    assert_eq!(apply(&mut c, &mut ui, Input::PressOutside, 1.0), None);
    assert!(!ui.centre_open && !ui.menu_open);

    ui.open_centre(5.0, true);
    assert_eq!(
        apply(&mut c, &mut ui, Input::CatcherClosed(Some(3)), 5.0),
        Some(HostAction::OutputGone(3))
    );
    assert!(ui.centre_open);
    assert_eq!(
        apply(&mut c, &mut ui, Input::CatcherClosed(None), 5.0),
        None
    );
    assert!(!ui.centre_open, "no output is left");
}

#[test]
fn a_card_is_as_tall_as_its_text_and_never_shorter_than_its_icon() {
    let m = metrics();
    let title = m.card(&notification("Title", ""), WIDTH, Fold::Banner);
    assert_eq!(
        title.height,
        (2.0 * PADDING + ICON_SIZE).max(MIN_CARD_HEIGHT),
        "one line is shorter than the icon, and no card is shorter than its least height"
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
    assert_eq!(
        body.height,
        (2.0 * PADDING + text.max(ICON_SIZE))
            .ceil()
            .max(MIN_CARD_HEIGHT)
    );
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
    // a folded centre card shows its first action as a pill over its corner, never a row of buttons
    let n = c.get(id).unwrap();
    assert!(m.card(n, WIDTH, Fold::Folded).actions.is_empty());
    let folded = m.card_with(n, WIDTH, Fold::Folded, true);
    assert!(folded.actions.is_empty());
    assert_eq!(folded.pill, Some(("reply".into(), "Reply".into())));
    assert_eq!(folded.height, body.height);
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
        column - AGE_WIDTH - m.disclosure_side() - BUTTON_GAP,
        "the summary leaves room for the age and the disclosure button"
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
    let bounded = body_text(&n).unwrap().content.text;
    assert_eq!(bounded.len(), MAX_BODY_BYTES);
    assert!(bounded.ends_with(ELLIPSIS), "a cut body says so");
    let multibyte = "é".repeat(MAX_BODY_BYTES);
    let n = notification("Title", &multibyte);
    let bounded = body_text(&n).unwrap().content.text;
    assert!(bounded.len() <= MAX_BODY_BYTES && bounded.len() > MAX_BODY_BYTES - 2);
    assert!(bounded.ends_with(ELLIPSIS));
    // a cut that falls short of the bound on a character boundary is still the body's end: no later run follows it
    let styled = format!("{}<b>a</b>", "é".repeat(MAX_BODY_BYTES));
    let n = notification("Title", &styled);
    let bounded = body_text(&n).unwrap().content.text;
    assert!(
        bounded.ends_with(ELLIPSIS),
        "{:?}",
        &bounded[bounded.len() - 8..]
    );
    assert!(!bounded.contains('a'));
}

#[test]
fn a_long_application_name_and_summary_are_drawn_and_named_within_their_caps() {
    use crate::notification::{MAX_APP_NAME_CHARS, MAX_SUMMARY_CHARS};
    let m = metrics();
    // a notification the host built itself, past the bus server's caps
    let mut n = notification("", "");
    n.app_name = "a".repeat(MAX_APP_NAME_CHARS * 4);
    n.summary = "s".repeat(MAX_SUMMARY_CHARS * 4);
    let card = m.card(&n, WIDTH, Fold::Banner);
    assert!(card.summary.text.chars().count() <= MAX_SUMMARY_CHARS);
    assert!(card.summary.text.ends_with(ELLIPSIS));
    let name = accessible_name(&n);
    let (app, summary) = name.split_once(": ").unwrap();
    assert_eq!(app.chars().count(), MAX_APP_NAME_CHARS);
    assert_eq!(summary.chars().count(), MAX_SUMMARY_CHARS);
    assert!(app.ends_with(ELLIPSIS) && summary.ends_with(ELLIPSIS));
    let speech = banner_speech(&n, &card);
    assert!(
        speech.starts_with(app),
        "the speech names the capped application"
    );
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
fn the_close_button_sits_on_the_card_s_corner_curve_over_the_icon_s_corner_and_the_banner_takes_input_there(
) {
    let card = Rect::new(28.0, SHADOW_MARGIN, 344.0, 80.0);
    let close = close_rect(card);
    assert_eq!(
        close.center(),
        Point::new(
            card.min_x() + CLOSE_DIAMETER / 2.0 - CLOSE_REACH_X,
            card.min_y() + CLOSE_DIAMETER / 2.0 - CLOSE_REACH_Y
        ),
        "centred on the top-leading corner curve"
    );
    assert!(
        card.contains(close.center())
            && close.min_x() < card.min_x()
            && close.min_y() < card.min_y(),
        "{close:?} straddles the corner of {card:?}"
    );
    let icon = Rect::new(
        card.min_x() + LEADING,
        card.min_y() + PADDING,
        ICON_SIZE,
        ICON_SIZE,
    );
    assert!(
        !icon.contains(close.center()),
        "{close:?} covers only the corner of the icon at {icon:?}"
    );

    // the banner's window takes input over the whole button, the part past the card too
    let m = metrics();
    let mut c = NotificationCenter::default();
    c.notify(NotifyInput::new("app", "Hello", "World"), 0.0);
    let banners = windows::<Input>(&mut c, &Ui::default(), &m, 1.0, |i| i);
    let region = banners[0].spec.input_region.clone().unwrap();
    assert_eq!(region.len(), 2, "{region:?}");
    assert_eq!(region[1], close_rect(region[0]));
    assert!(
        region[1].min_x() >= 0.0 && region[1].min_y() >= 0.0,
        "{region:?}"
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
fn a_banner_s_speech_is_its_application_summary_and_shown_body_in_that_order() {
    let m = metrics();
    let speech = |app: &str, summary: &str, body: &str| {
        let mut c = NotificationCenter::default();
        let id = c.notify(NotifyInput::new(app, summary, body), 0.0);
        let n = c.get(id).unwrap().clone();
        banner_speech(&n, &m.card(&n, WIDTH, Fold::Banner))
    };
    assert_eq!(
        speech("Mail", "Lunch", "Are you free?"),
        "Mail: Lunch, Are you free?"
    );
    assert_eq!(speech("Mail", "Lunch", ""), "Mail: Lunch");
    assert_eq!(speech("", "Lunch", "Are you free?"), "Lunch, Are you free?");
    assert_eq!(speech("Mail", "", "Are you free?"), "Mail, Are you free?");
    assert_eq!(speech("", "", "Are you free?"), "Are you free?");
    // line breaks and runs of spaces are one space; markup is gone
    assert_eq!(
        speech(" Mail ", "Lunch\nat noon", "Are  you\n\n<b>free</b>?"),
        "Mail: Lunch at noon, Are you free?"
    );
    // only what the card shows: a body past its lines ends where the card cuts it
    let cut = speech("app", "Title", &long(400));
    assert!(cut.ends_with('\u{2026}'), "{cut}");
    assert!(cut.len() < long(400).len());
}

#[test]
fn no_window_is_open_while_the_screen_is_locked() {
    let m = metrics();
    let mut c = NotificationCenter::default();
    c.notify(NotifyInput::new("app", "Hello", "World"), 0.0);
    let ui = Ui::default();
    c.set_screen_locked(true, 1.0);
    assert!(windows::<Input>(&mut c, &ui, &m, 1.0, |i| i).is_empty());
    let centre = Ui {
        centre_open: true,
        ..Ui::default()
    };
    assert!(windows::<Input>(&mut c, &centre, &m, 1.0, |i| i).is_empty());
    c.set_screen_locked(false, 1.5);
    assert_eq!(windows::<Input>(&mut c, &ui, &m, 1.5, |i| i).len(), 1);
}

#[test]
fn escape_closes_the_clear_menu_before_the_centre_and_a_centre_opened_once_it_is_out_starts_afresh()
{
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
    assert!(!ui.centre_open);
    assert!(
        ui.unfolded.contains(&7),
        "the list keeps its state while it slides out"
    );
    ui.open_centre(10.0, false);
    assert!(ui.centre_open);
    assert!(
        ui.unfolded.is_empty(),
        "a centre that was out opens with a fresh list"
    );
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

#[test]
fn the_age_reads_now_then_minutes_hours_and_days() {
    assert_eq!(age(100.0, 100.0), "now");
    assert_eq!(age(100.0, 159.0), "now");
    assert_eq!(age(0.0, 60.0), "1m ago");
    assert_eq!(age(0.0, 3599.0), "59m ago");
    assert_eq!(age(0.0, 7200.0), "2h ago");
    assert_eq!(age(0.0, 3.0 * 86_400.0), "3d ago");
    // a clock that went back says now
    assert_eq!(age(100.0, 50.0), "now");
}

#[test]
fn a_group_expands_and_collapses_with_a_spring_and_at_once_under_reduce_motion() {
    let mut c = NotificationCenter::default();
    c.notify(NotifyInput::new("Mail", "One", ""), 0.0);
    c.notify(NotifyInput::new("Mail", "Two", ""), 0.1);
    let key = c.groups()[0].key.clone();
    let mut ui = Ui::default();
    assert_eq!(ui.expansion(&key, 1.0), 0.0);
    apply(&mut c, &mut ui, Input::Expand(key.clone(), true), 1.0);
    assert!(ui.is_animating(1.1));
    let early = ui.expansion(&key, 1.05);
    let later = ui.expansion(&key, 1.2);
    assert!(
        0.0 < early && early < later && later < 1.0,
        "{early} {later}"
    );
    assert_eq!(ui.expansion(&key, 3.0), 1.0);
    assert!(!ui.is_animating(3.0));
    apply(&mut c, &mut ui, Input::Expand(key.clone(), false), 3.0);
    let closing = ui.expansion(&key, 3.1);
    assert!(0.0 < closing && closing < 1.0);
    assert_eq!(ui.expansion(&key, 5.0), 0.0);

    c.config.motion.reduce_motion = true;
    apply(&mut c, &mut ui, Input::Expand(key.clone(), true), 6.0);
    assert_eq!(
        ui.expansion(&key, 6.0),
        1.0,
        "Reduce Motion expands at once"
    );
    assert!(!ui.is_animating(6.0));
    apply(&mut c, &mut ui, Input::Expand(key.clone(), false), 6.0);
    assert_eq!(ui.expansion(&key, 6.0), 0.0);

    // clearing the group clears each of its notifications
    apply(&mut c, &mut ui, Input::ClearGroup(key), 7.0);
    assert!(c.groups().is_empty());
}

/// A halo is kept off a neighbouring card only where the list shows that card: the part of a card under the header
/// or past the list's bottom edge, and a card wholly past either, leave no unshadowed patch in the shape of a card.
#[test]
fn a_halo_is_kept_off_a_neighbour_only_where_the_list_shows_it() {
    let card = |y: f64| Rect::new(112.0, y, 344.0, 58.0);
    // the list from 68 to 624
    assert_eq!(shown_in_list(card(200.0), 68.0, 556.0), Some(card(200.0)));
    assert_eq!(
        shown_in_list(card(48.0), 68.0, 556.0),
        Some(Rect::new(112.0, 68.0, 344.0, 38.0)),
        "cut at the list's top, as its glass is"
    );
    assert_eq!(
        shown_in_list(card(600.0), 68.0, 556.0),
        Some(Rect::new(112.0, 600.0, 344.0, 24.0)),
        "cut at the list's bottom"
    );
    assert_eq!(shown_in_list(card(-10.0), 68.0, 556.0), None, "above");
    assert_eq!(shown_in_list(card(10.0), 68.0, 556.0), None, "at the edge");
    assert_eq!(shown_in_list(card(624.0), 68.0, 556.0), None, "below");
    assert_eq!(
        shown_in_list(card(-10.0), 68.0, 0.0),
        Some(card(-10.0)),
        "before the list was laid out"
    );
}

/// The shape a cut card casts its halo from and keeps it off is the card's between the list's edges: the whole card
/// when neither cuts it, a straight edge with square corners where one cuts it below its corners' curves, the rest of
/// the curve where one cuts it inside them, and nothing when the card is past an edge.
#[test]
fn a_cut_card_s_shape_ends_straight_at_the_list_s_edge() {
    let card = Rect::new(10.0, 100.0, 344.0, 58.0);
    let radius = 18.0;
    // every point of a path, its curves' control points included: the shape lies inside their box
    let bounds = |verbs: &[PathVerb]| {
        let mut points = Vec::new();
        for verb in verbs {
            match verb {
                PathVerb::MoveTo(p) | PathVerb::LineTo(p) => points.push(*p),
                PathVerb::CubicTo(a, b, c) => points.extend([*a, *b, *c]),
                _ => {}
            }
        }
        let fold = |f: fn(f64, f64) -> f64, start: f64, of: fn(&Point) -> f64| {
            points.iter().map(of).fold(start, f)
        };
        (
            fold(f64::min, f64::INFINITY, |p| p.x),
            fold(f64::min, f64::INFINITY, |p| p.y),
            fold(f64::max, f64::NEG_INFINITY, |p| p.x),
            fold(f64::max, f64::NEG_INFINITY, |p| p.y),
        )
    };
    let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
    // the points of a path at the height `y`, left to right
    let at_height = |verbs: &[PathVerb], y: f64| {
        let mut xs: Vec<f64> = verbs
            .iter()
            .filter_map(|verb| match verb {
                PathVerb::MoveTo(p) | PathVerb::LineTo(p) | PathVerb::CubicTo(_, _, p) => Some(*p),
                _ => None,
            })
            .filter(|p| near(p.y, y))
            .map(|p| p.x)
            .collect();
        xs.sort_by(f64::total_cmp);
        (xs[0], xs[xs.len() - 1])
    };

    // not cut: the card's own shape
    assert_eq!(
        cut_rounded_path(card, radius, 68.0, 624.0),
        rounded_path(card, radius)
    );
    assert_eq!(
        cut_rounded_path(card, radius, f64::NEG_INFINITY, f64::INFINITY),
        rounded_path(card, radius)
    );
    // past either edge: nothing
    assert!(cut_rounded_path(card, radius, 158.0, 624.0).is_empty());
    assert!(cut_rounded_path(card, radius, 0.0, 100.0).is_empty());

    // cut through its straight sides at the top: square corners on the edge, its lower corners round
    let cut = cut_rounded_path(card, radius, 130.0, 624.0);
    let (l, t, r, b) = bounds(&cut);
    assert!(near(l, 10.0) && near(t, 130.0) && near(r, 354.0) && near(b, 158.0));
    assert_eq!(at_height(&cut, 130.0), (10.0, 354.0), "{cut:?}");
    let (left, right) = at_height(&cut, 158.0);
    assert!(near(left, 28.0) && near(right, 336.0), "{cut:?}");
    assert_eq!(
        cut.iter()
            .filter(|v| matches!(v, PathVerb::CubicTo(..)))
            .count(),
        2
    );

    // cut at the bottom: the same upside down
    let cut = cut_rounded_path(card, radius, 68.0, 130.0);
    let (_, t, _, b) = bounds(&cut);
    assert!(near(t, 100.0) && near(b, 130.0), "{t} {b}: {cut:?}");
    assert_eq!(at_height(&cut, 130.0), (10.0, 354.0), "{cut:?}");
    let (left, right) = at_height(&cut, 100.0);
    assert!(near(left, 28.0) && near(right, 336.0), "{cut:?}");

    // cut inside its upper corners' curves, 9 points down: the edge ends on the curves, which go on below it
    let cut = cut_rounded_path(card, radius, 109.0, 624.0);
    let inset = radius - (radius * radius - 81.0_f64).sqrt();
    let (left, right) = at_height(&cut, 109.0);
    assert!(
        near(left, 10.0 + inset) && near(right, 354.0 - inset),
        "{left} {right} {inset}"
    );
    let (l, t, r, b) = bounds(&cut);
    assert!(near(t, 109.0) && near(b, 158.0), "{t} {b}");
    assert!(l >= 10.0 - 1e-6 && r <= 354.0 + 1e-6, "{l} {r}");
    assert_eq!(
        cut.iter()
            .filter(|v| matches!(v, PathVerb::CubicTo(..)))
            .count(),
        4
    );
    // every curve of it stays on its corner's circle: half way along, a curve is the radius from the centre
    let mut from = Point::ZERO;
    for verb in &cut {
        match verb {
            PathVerb::MoveTo(p) | PathVerb::LineTo(p) => from = *p,
            PathVerb::CubicTo(a, b, to) => {
                let mid = Point::new(
                    (from.x + 3.0 * a.x + 3.0 * b.x + to.x) / 8.0,
                    (from.y + 3.0 * a.y + 3.0 * b.y + to.y) / 8.0,
                );
                let centre = Point::new(
                    if mid.x < 182.0 { 28.0 } else { 336.0 },
                    if mid.y < 129.0 { 118.0 } else { 140.0 },
                );
                let distance = (mid.x - centre.x).hypot(mid.y - centre.y);
                assert!((distance - radius).abs() < 0.01, "{verb:?}: {distance}");
                from = *to;
            }
            _ => {}
        }
    }
}

/// A card fading in covers a neighbour's halo by as much as it is there: at the start of its fade the halo still shows
/// over it almost whole, and only once it is opaque is the halo kept off it entirely.
#[test]
fn a_card_fading_in_covers_its_neighbours_halo_by_its_opacity() {
    let pass = |inside: Option<usize>, cut: &[usize], opacity: f32| HaloPass {
        inside,
        cut: cut.to_vec(),
        opacity,
    };
    // no neighbour: the halo whole
    assert_eq!(halo_passes(&[]), [pass(None, &[], 1.0)]);
    // an opaque neighbour: kept off it, nothing over it
    assert_eq!(halo_passes(&[1.0]), [pass(None, &[0], 1.0)]);
    // a neighbour a tenth of the way into its fade: the halo over it at nine tenths
    let fading = halo_passes(&[0.1]);
    assert_eq!(fading.len(), 2);
    assert_eq!(fading[0], pass(None, &[0], 1.0));
    assert_eq!(
        (fading[1].inside, fading[1].cut.as_slice()),
        (Some(0), &[][..])
    );
    assert!((fading[1].opacity - 0.9).abs() < 1e-6, "{fading:?}");
    // two neighbours, one opaque and one half in: the halo over the second at half, kept off the first there too
    assert_eq!(
        halo_passes(&[1.0, 0.5]),
        [pass(None, &[0, 1], 1.0), pass(Some(1), &[0], 0.5)]
    );
    // the halo over a fading neighbour grows fainter as the neighbour comes in
    let over = |o: f64| halo_passes(&[o]).get(1).map_or(0.0, |p| p.opacity);
    let mut last = 1.0;
    for step in 1..=10 {
        let now = over(f64::from(step) / 10.0);
        assert!(now < last, "{step}: {now} after {last}");
        last = now;
    }
    assert_eq!(last, 0.0);
}
