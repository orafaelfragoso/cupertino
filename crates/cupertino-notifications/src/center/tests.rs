use super::*;
use crate::notification::Hints;

const VP: Size = Size { width: 1000.0, height: 700.0 };
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
    assert_eq!(p.duration(Urgency::Normal, Expire::Default, false), Some(5.0));
    assert_eq!(p.duration(Urgency::Low, Expire::Default, false), Some(5.0));
    assert_eq!(p.duration(Urgency::Critical, Expire::Default, false), None);
    assert_eq!(p.duration(Urgency::Normal, Expire::Never, false), None);
    assert_eq!(p.duration(Urgency::Normal, Expire::Millis(1500), false), Some(1.5));
    assert_eq!(p.duration(Urgency::Critical, Expire::Millis(1500), false), None);
    assert_eq!(p.duration(Urgency::Normal, Expire::Millis(10_000_000), false), Some(60.0));
}

#[test]
fn the_lock_screen_lifetime_is_10_s_for_default_timeouts_only() {
    let p = TimeoutPolicy::default();
    assert_eq!(p.duration(Urgency::Normal, Expire::Default, true), Some(10.0));
    assert_eq!(p.duration(Urgency::Critical, Expire::Default, true), None);
    assert_eq!(p.duration(Urgency::Normal, Expire::Millis(1500), true), Some(1.5));
}

#[test]
fn a_banner_shown_on_the_lock_screen_expires_after_10_s() {
    let mut c = center();
    c.set_screen_locked(true);
    let id = c.notify(input("app", "locked"), 0.0);
    c.tick(9.9);
    assert!(closed(&mut c).is_empty());
    c.tick(10.1);
    assert_eq!(closed(&mut c), vec![(id, CloseReason::Expired)]);
}

#[test]
fn the_width_setting_is_clamped_to_344_620() {
    let widths: Vec<f64> = [0, 300, 344, 500, 620, 900].map(|w| StackLayout::with_width_setting(w).width).to_vec();
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
    assert_eq!(c.next_wakeup(5.3), Some(Wakeup::At(5.26)), "0.25 s exit + 0.01 s");
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
    let id = c.notify(input("app", "disk full").with_urgency(Urgency::Critical), 0.0);
    let next = c.notify(input("app", "next"), 10.0);
    c.tick(3600.0);
    assert_eq!(c.visible_ids(), vec![id]);
    assert_eq!(c.queued_count(), 1);
    assert_eq!(c.next_wakeup(3600.0), None);
    c.dismiss_banner(id, 3600.0);
    assert_eq!(closed(&mut c), vec![(id, CloseReason::Dismissed)]);
    assert_eq!(c.visible_ids(), vec![next, id], "the waiting banner enters while the alert leaves");
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
    assert!(closed(&mut c).iter().all(|(id, _)| *id != b), "b goes to the centre without a close");
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
    c.notify(input("app", "urgent 2").with_urgency(Urgency::Critical), 0.0);
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
    i.hints = Hints { transient: true, ..Default::default() };
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
    assert_eq!(ev[0], Event::ActionInvoked { id: a, key: "reply".into(), activation_token: Some("tok".into()) });
    assert_eq!(ev[1], Event::Closed { id: a, reason: CloseReason::Dismissed });
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
    assert_eq!(g[0].notifications.iter().map(|n| n.summary.as_str()).collect::<Vec<_>>(), ["m3", "m2", "m1"]);
    assert_eq!(g[0].unread, 2);
    assert_eq!(g[1].key, "Chat");
    assert_eq!(c.clear_group("Mail", 5.0), 3);
    assert_eq!(c.groups().len(), 1);
    assert_eq!(c.clear_scope(ClearScope::All, 5.0), 1);
    assert_eq!(c.history_len(), 0);
}

#[test]
fn history_is_bounded() {
    let mut c = NotificationCenter::new(CenterConfig { history_limit: 3, ..CenterConfig::default() });
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
    assert!((x_at(&mut c, 0.125) - (REST_X + 188.0)).abs() < 1e-6, "half way at half time");
    assert_eq!(c.next_wakeup(0.1), Some(Wakeup::Frame));
    let p = c.placements(VP, &mut FixedHeight(60.0), 0.25);
    assert_eq!((p[0].rect.x(), p[0].rect.y(), p[0].rect.width(), p[0].opacity), (REST_X, 40.0, 344.0, 1.0));
    c.tick(5.0);
    assert!((x_at(&mut c, 5.125) - (REST_X + 188.0)).abs() < 1e-6, "slides out the same way");
}

#[test]
fn reduce_motion_fades_in_place() {
    let mut c = center();
    c.config.motion.reduce_motion = true;
    c.notify(input("app", "x"), 0.0);
    let opacity = |c: &mut NotificationCenter, t: f64| c.placements(VP, &mut FixedHeight(60.0), t)[0].opacity;
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
    assert_eq!(x_at(&mut c, 1.05), REST_X, "under the 5 pt minimum distance");
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
    assert!((x_at(&mut c, 1.2) - 628.133_68).abs() < 1e-4, "{}", x_at(&mut c, 1.2));
    assert_eq!(x_at(&mut c, 4.0), REST_X);
    assert!(closed(&mut c).is_empty());
}

#[test]
fn rightward_release_dismisses_at_the_dismiss_speed() {
    let swipe = SwipeConfig::default();
    assert_eq!((swipe.dismiss_speed(50.0), swipe.dismiss_speed(1500.0)), (8.0, 15.0));

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
    assert_eq!(cfg.layout.top_inset, theme.shapes.menu_bar_height + 16.0);
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
    assert_eq!(c.history().iter().map(|n| n.id).collect::<Vec<_>>(), [pinned]);
    assert_ne!(normal, pinned);
    assert_eq!(c.clear_scope(ClearScope::All, 4.0), 1);
    assert_eq!(c.history_len(), 0);
}
