use super::*;
use crate::notification::Hints;
use cupertino_defaults::notifications::{AppOverride, NotificationSettings, Previews};

const VP: Size = Size {
    width: 1000.0,
    height: 700.0,
};
/// Resting x of a banner: 1000 − 16 (right margin) − 344 (width).
const REST_X: f64 = 640.0;

fn input(app: &str, summary: &str) -> NotifyInput {
    NotifyInput::new(app, summary, "body")
}

fn center() -> NotificationCenter {
    NotificationCenter::default()
}

fn closed(c: &mut NotificationCenter) -> Vec<(u32, CloseReason)> {
    c.take_events()
        .into_iter()
        .filter_map(|e| match e {
            Event::Closed { id, reason } => Some((id, reason)),
            _ => None,
        })
        .collect()
}

fn x_at(c: &mut NotificationCenter, now: f64) -> f64 {
    c.placements(VP, &mut FixedHeight(60.0), now)[0].rect.x()
}

#[test]
fn ids_are_unique_and_replacement_keeps_the_id() {
    let mut c = center();
    let a = c.notify(input("app", "one"), 0.0);
    let b = c.notify(input("app", "two"), 0.0);
    assert_ne!(a, b);
    assert!(a != 0 && b != 0);
    let mut r = input("app", "one again");
    r.replaces_id = a;
    assert_eq!(c.notify(r, 1.0), a);
    assert_eq!(c.get(a).unwrap().summary, "one again");
    assert_eq!(c.get(a).unwrap().revision, 1);
    assert_eq!(c.history_len(), 2);
    let mut unknown = input("app", "x");
    unknown.replaces_id = 999;
    assert_ne!(c.notify(unknown, 1.0), 999);
}

#[test]
fn timeouts_follow_urgency_and_client_request() {
    let p = TimeoutPolicy::default();
    assert_eq!(p.duration(Urgency::Normal, Expire::Default), Some(5.0));
    assert_eq!(p.duration(Urgency::Low, Expire::Default), Some(5.0));
    assert_eq!(p.duration(Urgency::Critical, Expire::Default), None);
    assert_eq!(p.duration(Urgency::Normal, Expire::Never), None);
    assert_eq!(p.duration(Urgency::Normal, Expire::Millis(1500)), Some(1.5));
    assert_eq!(p.duration(Urgency::Critical, Expire::Millis(1500)), None);
    assert_eq!(
        p.duration(Urgency::Normal, Expire::Millis(10_000_000)),
        Some(60.0)
    );
}

#[test]
fn the_width_setting_is_clamped_to_344_620() {
    let widths: Vec<f64> = [0, 300, 344, 500, 620, 900]
        .map(|w| StackLayout::with_width_setting(w).width)
        .to_vec();
    assert_eq!(widths, [344.0, 344.0, 344.0, 500.0, 620.0, 620.0]);
}

#[test]
fn banner_lives_five_seconds_and_leaves_after_the_exit_transition() {
    let mut c = center();
    let id = c.notify(input("app", "hello"), 0.0);
    assert_eq!(c.next_wakeup(0.3), Some(Wakeup::At(5.0)));
    c.tick(4.99);
    assert!(closed(&mut c).is_empty());
    c.tick(5.0);
    assert_eq!(closed(&mut c), vec![(id, CloseReason::Expired)]);
    assert_eq!(
        c.next_wakeup(5.3),
        Some(Wakeup::At(5.26)),
        "0.25 s exit + 0.01 s"
    );
    c.tick(5.25);
    assert_eq!(c.visible_count(), 1, "still sliding out");
    c.tick(5.26);
    assert_eq!(c.visible_count(), 0);
    assert_eq!(c.history_len(), 1);
    c.tick(7.0);
    assert!(closed(&mut c).is_empty(), "signalled once");
}

#[test]
fn critical_is_an_alert_without_timer_and_holds_the_slot() {
    let mut c = center();
    let id = c.notify(
        input("app", "disk full").with_urgency(Urgency::Critical),
        0.0,
    );
    let next = c.notify(input("app", "next"), 10.0);
    c.tick(3600.0);
    assert_eq!(c.visible_ids(), vec![id]);
    assert_eq!(c.queued_count(), 1);
    assert_eq!(c.next_wakeup(3600.0), None);
    c.dismiss_banner(id, 3600.0);
    assert_eq!(closed(&mut c), vec![(id, CloseReason::Dismissed)]);
    assert_eq!(
        c.visible_ids(),
        vec![next, id],
        "the waiting banner enters while the alert leaves"
    );
}

#[test]
fn close_notification_removes_everything_with_reason_closed() {
    let mut c = center();
    let id = c.notify(input("app", "x"), 0.0);
    assert!(c.close(id, 0.5));
    assert_eq!(closed(&mut c), vec![(id, CloseReason::Closed)]);
    assert_eq!(c.history_len(), 0);
    assert!(!c.close(id, 0.6));
    c.tick(2.0);
    assert!(c.get(id).is_none());
}

#[test]
fn a_second_banner_waits_until_the_first_was_shown_one_second() {
    let mut c = center();
    let a = c.notify(input("app", "a"), 0.0);
    let b = c.notify(input("app", "b"), 0.4);
    assert_eq!(c.visible_ids(), vec![a]);
    assert_eq!(c.queued_count(), 1);
    c.tick(0.99);
    assert!(closed(&mut c).is_empty());
    c.tick(1.0);
    assert_eq!(closed(&mut c), vec![(a, CloseReason::Expired)]);
    assert_eq!(c.visible_ids(), vec![b, a]);
    assert_eq!(c.queued_count(), 0);
    c.tick(5.99);
    assert!(closed(&mut c).is_empty());
    c.tick(6.0);
    assert_eq!(closed(&mut c), vec![(b, CloseReason::Expired)]);
}

#[test]
fn a_banner_shown_longer_than_the_minimum_gives_way_at_once() {
    let mut c = center();
    let a = c.notify(input("app", "a"), 0.0);
    let b = c.notify(input("app", "b"), 3.0);
    assert_eq!(closed(&mut c), vec![(a, CloseReason::Expired)]);
    assert_eq!(c.visible_ids(), vec![b, a]);
}

#[test]
fn a_newer_banner_replaces_the_waiting_one() {
    let mut c = center();
    let a = c.notify(input("app", "a"), 0.0);
    let b = c.notify(input("app", "b"), 0.2);
    let d = c.notify(input("app", "d"), 0.5);
    assert_eq!(c.queued_count(), 1);
    c.tick(1.0);
    assert_eq!(c.visible_ids(), vec![d, a]);
    assert!(
        closed(&mut c).iter().all(|(id, _)| *id != b),
        "b goes to the centre without a close"
    );
    assert_eq!(c.history_len(), 3);
}

#[test]
fn a_replaced_waiting_transient_notification_is_reported_closed() {
    let mut c = center();
    c.notify(input("app", "a"), 0.0);
    let mut t = input("app", "volume");
    t.hints.transient = true;
    let t = c.notify(t, 0.2);
    c.notify(input("app", "d"), 0.5);
    assert_eq!(closed(&mut c), vec![(t, CloseReason::Undefined)]);
    assert!(c.get(t).is_none());
}

#[test]
fn dnd_suppresses_banners_but_keeps_history_and_critical_bypasses() {
    let mut c = center();
    c.set_dnd(DndMode::On, 0.0);
    let a = c.notify(input("app", "quiet"), 0.0);
    assert_eq!(c.visible_count(), 0);
    assert_eq!(c.history_len(), 1);
    assert!(c.get(a).is_some());
    c.notify(input("app", "urgent").with_urgency(Urgency::Critical), 0.0);
    assert_eq!(c.visible_count(), 1);
    c.config.critical_bypasses_dnd = false;
    c.notify(
        input("app", "urgent 2").with_urgency(Urgency::Critical),
        0.0,
    );
    assert_eq!((c.visible_count(), c.queued_count()), (1, 0));
}

#[test]
fn dnd_until_ends_by_itself() {
    let mut c = center();
    c.set_dnd(DndMode::Until(10.0), 0.0);
    assert!(c.dnd_active(9.9));
    assert_eq!(c.next_wakeup(5.0), Some(Wakeup::At(10.0)));
    c.tick(10.0);
    assert!(!c.dnd_active(10.0));
    assert_eq!(c.dnd(), DndMode::Off);
    c.notify(input("app", "now visible"), 10.0);
    assert_eq!(c.visible_count(), 1);
}

#[test]
fn enabling_dnd_drops_the_waiting_banner() {
    let mut c = center();
    c.notify(input("app", "a"), 0.0);
    c.notify(input("app", "b"), 0.1);
    assert_eq!(c.queued_count(), 1);
    c.set_dnd(DndMode::On, 0.2);
    assert_eq!((c.visible_count(), c.queued_count()), (1, 0));
    assert_eq!(c.history_len(), 2);
}

#[test]
fn transient_notifications_skip_the_centre() {
    let mut c = center();
    let mut i = input("app", "volume");
    i.hints = Hints {
        transient: true,
        ..Default::default()
    };
    let id = c.notify(i, 0.0);
    assert_eq!(c.history_len(), 0);
    assert_eq!(c.visible_count(), 1);
    c.tick(5.0);
    c.tick(5.26);
    assert!(c.get(id).is_none(), "fully gone");
}

#[test]
fn transient_during_dnd_is_dropped_but_reported() {
    let mut c = center();
    c.set_dnd(DndMode::On, 0.0);
    let mut i = input("app", "volume");
    i.hints.transient = true;
    let id = c.notify(i, 0.0);
    assert!(c.get(id).is_none());
    assert_eq!(closed(&mut c), vec![(id, CloseReason::Undefined)]);
}

#[test]
fn hover_rearms_the_timer_and_holds_the_banner_with_one_second_rechecks() {
    let mut c = center();
    let id = c.notify(input("app", "x"), 0.0);
    c.set_hover(Some(id), 2.0);
    assert_eq!(c.next_wakeup(2.0), Some(Wakeup::At(7.0)));
    c.tick(7.0);
    assert!(closed(&mut c).is_empty());
    assert_eq!(c.next_wakeup(7.0), Some(Wakeup::At(8.0)));
    c.tick(8.0);
    assert_eq!(c.next_wakeup(8.0), Some(Wakeup::At(9.0)));
    c.set_hover(None, 8.5);
    c.tick(13.49);
    assert!(closed(&mut c).is_empty(), "leaving re-arms the full 5 s");
    c.tick(13.5);
    assert_eq!(closed(&mut c), vec![(id, CloseReason::Expired)]);
}

#[test]
fn action_invocation_closes_unless_resident() {
    let mut c = center();
    let a = c.notify(input("app", "a").with_action("reply", "Reply"), 0.0);
    assert!(c.invoke_action(a, "reply", Some("tok".into()), 1.0));
    let ev = c.take_events();
    assert_eq!(
        ev[0],
        Event::ActionInvoked {
            id: a,
            key: "reply".into(),
            activation_token: Some("tok".into())
        }
    );
    assert_eq!(
        ev[1],
        Event::Closed {
            id: a,
            reason: CloseReason::Dismissed
        }
    );
    assert_eq!(c.history_len(), 0);
    assert!(!c.invoke_action(a, "reply", None, 2.0));

    let mut i = input("app", "b").with_action("play", "Play");
    i.hints.resident = true;
    let b = c.notify(i, 2.0);
    assert!(c.invoke_action(b, "play", None, 2.5));
    assert_eq!(c.history_len(), 1);
    assert!(!c.invoke_action(b, "unknown-key", None, 2.5));
}

#[test]
fn click_activates_default_action_or_dismisses() {
    let mut c = center();
    let a = c.notify(input("app", "a").with_action("default", "Open"), 0.0);
    assert_eq!(c.activate(a, None, 1.0), ActivateOutcome::ActionInvoked);
    assert!(matches!(c.take_events()[0], Event::ActionInvoked { .. }));
    let b = c.notify(input("app", "b"), 2.0);
    assert_eq!(c.activate(b, None, 2.0), ActivateOutcome::Dismissed);
    assert_eq!(closed(&mut c), vec![(b, CloseReason::Dismissed)]);
    assert_eq!(c.history_len(), 1, "dismissed banner stays in the centre");
    assert_eq!(c.activate(12345, None, 2.0), ActivateOutcome::NotFound);
}

#[test]
fn groups_collect_by_app_and_order_by_recency() {
    let mut c = center();
    c.set_dnd(DndMode::On, 0.0);
    c.notify(input("Mail", "m1"), 1.0);
    c.notify(input("Chat", "c1"), 2.0);
    c.notify(input("Mail", "m2"), 3.0);
    let mut e = input("Mail Alias", "m3");
    e.hints.desktop_entry = Some("Mail.desktop".into());
    let m3 = c.notify(e, 4.0);
    c.mark_read(m3);
    let g = c.groups();
    assert_eq!(g.len(), 2);
    assert_eq!(g[0].key, "Mail");
    assert_eq!(
        g[0].notifications
            .iter()
            .map(|n| n.summary.as_str())
            .collect::<Vec<_>>(),
        ["m3", "m2", "m1"]
    );
    assert_eq!(g[0].unread, 2);
    assert_eq!(g[1].key, "Chat");
    assert_eq!(c.clear_group("Mail", 5.0), 3);
    assert_eq!(c.groups().len(), 1);
    assert_eq!(c.clear_scope(ClearScope::All, 5.0), 1);
    assert_eq!(c.history_len(), 0);
}

#[test]
fn history_is_bounded() {
    let mut c = NotificationCenter::new(CenterConfig {
        history_limit: 3,
        ..CenterConfig::default()
    });
    c.set_dnd(DndMode::On, 0.0);
    for i in 0..6 {
        c.notify(input("app", &format!("{i}")), i as f64);
    }
    let h = c.history();
    assert_eq!(h.len(), 3);
    assert_eq!(h[0].summary, "5");
    assert_eq!(h[2].summary, "3");
}

#[test]
fn banner_slides_in_by_the_column_width_with_ease_in_out() {
    let mut c = center();
    c.notify(input("app", "x"), 0.0);
    assert_eq!(x_at(&mut c, 0.0), REST_X + 376.0);
    assert!(
        (x_at(&mut c, 0.125) - (REST_X + 188.0)).abs() < 1e-6,
        "half way at half time"
    );
    assert_eq!(c.next_wakeup(0.1), Some(Wakeup::Frame));
    let p = c.placements(VP, &mut FixedHeight(60.0), 0.25);
    assert_eq!(
        (
            p[0].rect.x(),
            p[0].rect.y(),
            p[0].rect.width(),
            p[0].opacity
        ),
        (REST_X, 24.0 + StackLayout::LIST_TOP_PADDING, 344.0, 1.0)
    );
    c.tick(5.0);
    assert!(
        (x_at(&mut c, 5.125) - (REST_X + 188.0)).abs() < 1e-6,
        "slides out the same way"
    );
}

#[test]
fn reduce_motion_fades_in_place() {
    let mut c = center();
    c.config.motion.reduce_motion = true;
    c.notify(input("app", "x"), 0.0);
    let opacity =
        |c: &mut NotificationCenter, t: f64| c.placements(VP, &mut FixedHeight(60.0), t)[0].opacity;
    assert_eq!(x_at(&mut c, 0.0), REST_X);
    assert_eq!(opacity(&mut c, 0.0), 0.0);
    assert!((opacity(&mut c, 0.125) - 0.5).abs() < 1e-6);
    assert_eq!(opacity(&mut c, 0.25), 1.0);
    assert_eq!(x_at(&mut c, 0.25), REST_X);
}

#[test]
fn a_press_without_travel_is_a_click() {
    let mut c = center();
    let id = c.notify(input("app", "x"), 0.0);
    assert!(c.swipe_begin(id, 1.0));
    c.swipe_update(id, 4.9, 1.05);
    assert_eq!(
        x_at(&mut c, 1.05),
        REST_X,
        "under the 5 pt minimum distance"
    );
    assert_eq!(c.swipe_end(id, 1.1), SwipeOutcome::Click);
    assert_eq!(c.visible_ids(), vec![id]);
}

#[test]
fn leftward_drag_is_clamped_at_16_and_springs_back() {
    let mut c = center();
    let id = c.notify(input("app", "x"), 0.0);
    c.swipe_begin(id, 1.0);
    c.swipe_update(id, -40.0, 1.1);
    assert_eq!(x_at(&mut c, 1.1), REST_X - 16.0);
    assert_eq!(c.swipe_end(id, 1.1), SwipeOutcome::Cancelled);
    // critically damped (ζ = 5 is clamped), ω0 = 10, v0 = −16/100: p(0.1) = 1 − e^−1 · (1 + 10.16 · 0.1)
    assert!(
        (x_at(&mut c, 1.2) - 628.133_68).abs() < 1e-4,
        "{}",
        x_at(&mut c, 1.2)
    );
    assert_eq!(x_at(&mut c, 4.0), REST_X);
    assert!(closed(&mut c).is_empty());
}

#[test]
fn rightward_release_dismisses_at_the_dismiss_speed() {
    let swipe = SwipeConfig::default();
    assert_eq!(
        (swipe.dismiss_speed(50.0), swipe.dismiss_speed(1500.0)),
        (8.0, 15.0)
    );

    let mut c = center();
    let id = c.notify(input("app", "x"), 0.0);
    c.swipe_begin(id, 1.0);
    c.swipe_update(id, 50.0, 1.1);
    assert_eq!(x_at(&mut c, 1.1), REST_X + 50.0);
    assert_eq!(c.swipe_end(id, 1.1), SwipeOutcome::Dismissed);
    assert_eq!(closed(&mut c), vec![(id, CloseReason::Dismissed)]);
    // 0.25 s · 15 / 8 + 0.01 s
    assert_eq!(c.next_wakeup(2.0), Some(Wakeup::At(1.1 + 0.468_75 + 0.01)));
    assert_eq!(c.history_len(), 1);

    let mut c = center();
    let id = c.notify(input("app", "y"), 0.0);
    c.swipe_begin(id, 1.0);
    c.swipe_update(id, 1500.0, 1.1);
    c.swipe_end(id, 1.1);
    assert_eq!(c.next_wakeup(2.0), Some(Wakeup::At(1.1 + 0.25 + 0.01)));
}

#[test]
fn swipe_removes_from_centre_when_configured() {
    let mut c = center();
    c.config.swipe_removes_from_centre = true;
    let id = c.notify(input("app", "x"), 0.0);
    c.swipe_begin(id, 1.0);
    c.swipe_update(id, 30.0, 1.1);
    assert_eq!(c.swipe_end(id, 1.1), SwipeOutcome::Dismissed);
    assert_eq!(c.history_len(), 0);
}

#[test]
fn a_dragged_banner_is_held_past_its_deadline() {
    let mut c = center();
    let id = c.notify(input("app", "x"), 0.0);
    c.swipe_begin(id, 4.0);
    c.swipe_update(id, -10.0, 4.5);
    c.tick(5.0);
    assert!(closed(&mut c).is_empty());
    assert_eq!(c.next_wakeup(5.0), Some(Wakeup::At(6.0)));
    c.swipe_end(id, 5.5);
    c.tick(6.0);
    assert_eq!(closed(&mut c), vec![(id, CloseReason::Expired)]);
}

#[test]
fn hit_testing_uses_last_placements() {
    let mut c = center();
    let id = c.notify(input("app", "x"), 0.0);
    let p = c.placements(VP, &mut FixedHeight(60.0), 3.0);
    assert_eq!(c.banner_at(p[0].rect.center()), Some(id));
    assert_eq!(c.banner_at(Point::new(5.0, 5.0)), None);
}

#[test]
fn replacing_the_visible_banner_restarts_its_timer_without_sliding() {
    let mut c = center();
    let id = c.notify(input("app", "x"), 0.0);
    let mut r = input("app", "x2");
    r.replaces_id = id;
    c.notify(r, 4.0);
    assert_eq!(x_at(&mut c, 4.0), REST_X);
    c.tick(8.99);
    assert!(closed(&mut c).is_empty(), "timer restarted at t=4");
    c.tick(9.0);
    assert_eq!(closed(&mut c), vec![(id, CloseReason::Expired)]);
}

#[test]
fn replacing_the_visible_banner_ends_its_swipe_at_rest() {
    let mut c = center();
    let id = c.notify(input("app", "x"), 0.0);
    c.swipe_begin(id, 2.0);
    c.swipe_update(id, -10.0, 2.1);
    let mut r = input("app", "x2");
    r.replaces_id = id;
    c.notify(r, 2.2);
    assert_eq!(x_at(&mut c, 2.2), REST_X);
    assert_eq!(c.swipe_end(id, 2.3), SwipeOutcome::Cancelled);
    c.tick(7.2);
    assert_eq!(closed(&mut c), vec![(id, CloseReason::Expired)]);
}

#[test]
fn replacing_the_visible_banner_while_another_waits_shows_it_for_one_second() {
    let mut c = center();
    let id = c.notify(input("app", "x"), 0.0);
    c.notify(input("app", "waiting"), 0.2);
    let mut r = input("app", "x2");
    r.replaces_id = id;
    c.notify(r, 0.5);
    assert_eq!(c.next_wakeup(1.0), Some(Wakeup::At(1.5)));
}

#[test]
fn from_theme_puts_the_banner_below_the_menu_bar() {
    let theme = Theme::load().unwrap();
    let cfg = CenterConfig::from_theme(&theme);
    assert_eq!(
        cfg.layout.top_inset,
        theme.shapes.menu_bar_height + StackLayout::LIST_TOP_PADDING
    );
}

#[test]
fn revision_changes_on_updates() {
    let mut c = center();
    let r0 = c.revision();
    c.notify(input("a", "b"), 0.0);
    assert!(c.revision() > r0);
}

#[test]
fn banners_only_keeps_pinned_and_is_offered_only_with_both_kinds() {
    let mut c = center();
    let normal = c.notify(input("Mail", "m"), 1.0);
    assert_eq!(c.clear_scopes(), [ClearScope::All]);
    let mut alert = input("Disk", "full");
    alert.hints.urgency = Some(Urgency::Critical);
    let pinned = c.notify(alert, 2.0);
    assert_eq!(c.clear_scopes(), [ClearScope::BannersOnly, ClearScope::All]);
    assert_eq!(c.clear_scope(ClearScope::BannersOnly, 3.0), 1);
    assert_eq!(
        c.history().iter().map(|n| n.id).collect::<Vec<_>>(),
        [pinned]
    );
    assert_ne!(normal, pinned);
    assert_eq!(c.clear_scope(ClearScope::All, 4.0), 1);
    assert_eq!(c.history_len(), 0);
}

// ---- the lock screen

fn locked_center() -> NotificationCenter {
    let mut c = center();
    c.set_screen_locked(true, 0.0);
    c
}

fn mail(summary: &str, body: &str) -> NotifyInput {
    let mut i = NotifyInput::new("Mail", summary, body);
    i.hints.desktop_entry = Some("org.example.Mail.desktop".into());
    i
}

fn with_policy(c: &mut NotificationCenter, app: &str, own: AppOverride) {
    let mut policy = c.policy().clone();
    assert!(policy.set_app(app, own));
    c.set_policy(policy);
}

fn previews(p: Previews) -> AppOverride {
    AppOverride {
        show_previews: Some(p),
        ..AppOverride::default()
    }
}

#[test]
fn a_notification_arriving_while_locked_goes_to_the_centre_and_the_lock_list_redacted() {
    let mut c = locked_center();
    let id = c.notify(mail("Bank", "Your code is 482913"), 1.0);
    assert_eq!(c.visible_count() + c.queued_count(), 0, "no banner");
    assert_eq!(c.history_len(), 1, "kept in the centre");
    assert_eq!(
        c.lock_screen(),
        [LockItem {
            id,
            app: "Mail".into(),
            title: REDACTED_TITLE.into(),
            body: String::new(),
        }]
    );
    // nothing counts down, nothing expires, nothing closes
    c.tick(100.0);
    assert_eq!(c.next_wakeup(100.0), None);
    assert!(closed(&mut c).is_empty());
}

#[test]
fn a_transient_notification_arriving_while_locked_is_closed_unseen() {
    let mut c = locked_center();
    let mut i = input("Volume", "50 %");
    i.hints.transient = true;
    let id = c.notify(i, 1.0);
    assert_eq!(closed(&mut c), vec![(id, CloseReason::Undefined)]);
    assert!(c.get(id).is_none() && c.lock_screen().is_empty());
}

#[test]
fn do_not_disturb_keeps_arrivals_off_the_lock_list() {
    let mut c = locked_center();
    c.set_dnd(DndMode::On, 0.0);
    c.notify(mail("Bank", "Your code is 482913"), 1.0);
    assert_eq!(c.history_len(), 1);
    assert!(c.lock_screen().is_empty());
}

#[test]
fn a_replacement_while_locked_updates_the_centre_and_the_lock_list_in_place() {
    let mut c = center();
    let earlier = c.notify(mail("Lunch", "noon?"), 0.0);
    c.tick(6.0);
    c.tick(6.3);
    assert_eq!(c.visible_count(), 0, "in the centre only");
    c.set_screen_locked(true, 7.0);
    assert!(c.lock_screen().is_empty(), "it arrived before the lock");
    let mut r = mail("Lunch", "one o'clock?");
    r.replaces_id = earlier;
    assert_eq!(c.notify(r, 8.0), earlier);
    assert_eq!(c.visible_count() + c.queued_count(), 0, "still no banner");
    assert_eq!(c.history_len(), 1);
    assert_eq!(c.get(earlier).unwrap().body, "one o'clock?");
    let listed = c.lock_screen();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, earlier);
    // closed by its application: off the lock list
    c.close(earlier, 9.0);
    assert!(c.lock_screen().is_empty());
}

#[test]
fn locking_freezes_the_banner_on_screen_and_sends_the_waiting_one_to_the_centre() {
    let mut c = center();
    let shown = c.notify(input("app", "shown"), 0.0);
    let waiting = c.notify(input("app", "waiting"), 0.5);
    assert_eq!(c.queued_count(), 1);
    c.set_hover(Some(shown), 0.6);
    c.set_screen_locked(true, 0.6);
    assert_eq!(c.queued_count(), 0, "the waiting banner went to the centre");
    assert_eq!(c.visible_ids(), vec![shown]);
    assert_eq!(c.history_len(), 2);
    assert!(c.get(waiting).is_some());
    // long past its lifetime it is still there, and nothing wakes the host for it
    c.tick(600.0);
    assert_eq!(c.visible_ids(), vec![shown]);
    assert!(closed(&mut c).is_empty());
    assert_eq!(c.next_wakeup(600.0), None);
    assert!(c.lock_screen().is_empty(), "both arrived before the lock");
}

#[test]
fn a_replacement_of_the_frozen_banner_waits_for_the_unlock_with_its_new_lifetime() {
    let mut c = center();
    let id = c.notify(input("app", "one"), 0.0);
    c.set_screen_locked(true, 4.0);
    let mut r = input("app", "two");
    r.replaces_id = id;
    c.notify(r, 10.0);
    c.tick(100.0);
    assert_eq!(c.visible_ids(), vec![id], "frozen, updated in place");
    assert_eq!(c.get(id).unwrap().summary, "two");
    assert_eq!(c.lock_screen().len(), 1, "an update while locked is listed");
    c.set_screen_locked(false, 200.0);
    // the whole new lifetime, from the unlock
    c.tick(204.9);
    assert!(closed(&mut c).is_empty());
    c.tick(205.0);
    assert_eq!(closed(&mut c), vec![(id, CloseReason::Expired)]);
}

#[test]
fn unlocking_resumes_the_rest_of_the_lifetime_quietly_and_empties_the_lock_list() {
    let mut c = center();
    let id = c.notify(input("app", "shown"), 0.0);
    c.set_screen_locked(true, 2.0);
    c.notify(mail("Bank", "code"), 3.0);
    assert_eq!(c.lock_screen().len(), 1);
    assert!(!c.is_quiet(id));
    c.set_screen_locked(false, 50.0);
    assert!(c.lock_screen().is_empty());
    assert!(c.is_quiet(id), "back on screen, said already");
    assert_eq!(
        c.visible_ids(),
        vec![id],
        "nothing that arrived while locked shows"
    );
    // 3 s of its 5 s were left
    assert_eq!(c.next_wakeup(50.3), Some(Wakeup::At(53.0)));
    c.tick(53.0);
    assert_eq!(closed(&mut c), vec![(id, CloseReason::Expired)]);
    // locking again lists only what arrives from then on
    c.set_screen_locked(true, 60.0);
    assert!(c.lock_screen().is_empty());
}

#[test]
fn a_banner_with_almost_nothing_left_at_the_unlock_stays_the_minimum_time() {
    let mut c = center();
    let id = c.notify(input("app", "shown"), 0.0);
    c.set_screen_locked(true, 4.9);
    c.set_screen_locked(false, 10.0);
    c.tick(10.9);
    assert!(closed(&mut c).is_empty());
    c.tick(11.0);
    assert_eq!(closed(&mut c), vec![(id, CloseReason::Expired)]);
}

#[test]
fn an_update_after_the_unlock_is_no_longer_quiet() {
    let mut c = center();
    let id = c.notify(input("app", "one"), 0.0);
    c.set_screen_locked(true, 1.0);
    c.set_screen_locked(false, 2.0);
    assert!(c.is_quiet(id));
    let mut r = input("app", "two");
    r.replaces_id = id;
    c.notify(r, 3.0);
    assert!(!c.is_quiet(id));
}

#[test]
fn an_application_kept_off_the_lock_screen_is_not_listed_and_others_are() {
    let mut c = locked_center();
    with_policy(
        &mut c,
        "org.example.Mail",
        AppOverride {
            show_on_lock_screen: Some(false),
            ..AppOverride::default()
        },
    );
    c.notify(mail("Bank", "code"), 1.0);
    let chat = c.notify(input("Chat", "hi"), 2.0);
    assert_eq!(
        c.lock_screen().iter().map(|i| i.id).collect::<Vec<_>>(),
        [chat]
    );
    // the default off, one application back on
    let mut policy = NotificationSettings {
        show_on_lock_screen: false,
        ..NotificationSettings::UNSET
    };
    policy.set_app(
        "org.example.Mail",
        AppOverride {
            show_on_lock_screen: Some(true),
            ..AppOverride::default()
        },
    );
    c.set_policy(policy);
    assert_eq!(
        c.lock_screen()
            .iter()
            .map(|i| i.app.as_str())
            .collect::<Vec<_>>(),
        ["Mail"]
    );
}

#[test]
fn each_preview_setting_decides_what_the_lock_list_and_the_unlocked_cards_show() {
    for (setting, on_lock, unlocked) in [
        (Previews::Always, true, true),
        (Previews::WhenUnlocked, false, true),
        (Previews::Never, false, false),
    ] {
        let mut c = locked_center();
        with_policy(&mut c, "org.example.Mail", previews(setting));
        let id = c.notify(
            mail("Bank", "Your code is <b>482913</b>").with_action("copy", "Copy"),
            1.0,
        );
        let item = &c.lock_screen()[0];
        assert_eq!(item.app, "Mail");
        if on_lock {
            assert_eq!(
                (item.title.as_str(), item.body.as_str()),
                ("Bank", "Your code is 482913"),
                "{setting:?}"
            );
        } else {
            assert_eq!(
                (item.title.as_str(), item.body.as_str()),
                (REDACTED_TITLE, ""),
                "{setting:?}"
            );
        }
        c.set_screen_locked(false, 2.0);
        let shown = c.shown(c.get(id).unwrap()).into_owned();
        if unlocked {
            assert_eq!(shown.summary, "Bank", "{setting:?}");
            assert_eq!(shown.button_actions().count(), 1);
        } else {
            assert_eq!(shown.summary, REDACTED_TITLE);
            assert!(shown.body.is_empty() && shown.spans.is_empty());
            assert_eq!(
                shown.button_actions().count(),
                0,
                "a label may hold content"
            );
        }
    }
}

#[test]
fn the_default_preview_setting_applies_to_applications_without_their_own() {
    let mut c = locked_center();
    c.set_policy(NotificationSettings {
        show_previews: Previews::Always,
        ..NotificationSettings::UNSET
    });
    with_policy(&mut c, "Chat", previews(Previews::WhenUnlocked));
    c.notify(mail("Bank", "code"), 1.0);
    c.notify(input("Chat", "secret"), 2.0);
    let titles: Vec<String> = c.lock_screen().into_iter().map(|i| i.title).collect();
    assert_eq!(titles, [REDACTED_TITLE, "Bank"], "newest first");
}

#[test]
fn the_lock_list_is_bounded_and_each_text_capped() {
    use crate::notification::{MAX_APP_NAME_CHARS, MAX_SUMMARY_CHARS};
    let mut c = locked_center();
    c.set_policy(NotificationSettings {
        show_previews: Previews::Always,
        ..NotificationSettings::UNSET
    });
    for i in 0..LOCK_SCREEN_LIMIT + 5 {
        let mut n = NotifyInput::new(
            &"a".repeat(MAX_APP_NAME_CHARS * 2),
            &"s".repeat(MAX_SUMMARY_CHARS * 2),
            &"b".repeat(LOCK_SCREEN_BODY_CHARS * 2),
        );
        n.hints.desktop_entry = Some(format!("app{i}"));
        c.notify(n, i as f64);
    }
    let listed = c.lock_screen();
    assert_eq!(listed.len(), LOCK_SCREEN_LIMIT);
    for item in &listed {
        assert_eq!(item.app.chars().count(), MAX_APP_NAME_CHARS);
        assert_eq!(item.title.chars().count(), MAX_SUMMARY_CHARS);
        assert_eq!(item.body.chars().count(), LOCK_SCREEN_BODY_CHARS);
    }
}

#[test]
fn the_applications_are_listed_once_by_their_key() {
    let mut c = center();
    c.notify(mail("a", ""), 0.0);
    c.notify(mail("b", ""), 0.0);
    c.notify(input("Chat", "c"), 0.0);
    assert_eq!(
        c.applications(),
        [
            ("Chat".to_owned(), "Chat".to_owned()),
            ("org.example.Mail".to_owned(), "Mail".to_owned())
        ]
    );
}
