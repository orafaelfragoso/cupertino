//! The panel's open and close and a detail view's motion follow the design catalogue's curves, and a reversal goes on
//! from where the last motion was.

use cupertino_controlcenter::lifecycle::{self, Lifecycle, Phase};
use cupertino_controlcenter::model::Detail;
use cupertino_controlcenter::motion::{self, visibility, Expansion, OTHERS_FADE};
use cupertino_kit::motion::{SpringPreset, Timing};

#[test]
fn the_fades_and_the_detail_motion_are_the_catalogues() {
    assert_eq!(
        lifecycle::open(),
        Timing::from(SpringPreset::CONTROL_PANEL_OPEN)
    );
    assert_eq!(lifecycle::close(), Timing::CONTROL_PANEL_CLOSE);
    assert_eq!(
        motion::spring(),
        Timing::from(SpringPreset::CONTROL_PANEL_DETAIL)
    );
    assert_eq!(motion::fade(), Timing::CONTROL_PANEL_DETAIL_FADE);
}

#[test]
fn the_card_fades_in_over_the_opening_curve_and_out_over_the_closing_one() {
    let open = lifecycle::open().duration();
    let close = lifecycle::close().duration();
    let mut panel = Lifecycle::new();
    assert_eq!((panel.phase(0.0), panel.opacity(0.0)), (Phase::Hidden, 0.0));
    panel.toggle(0.0);
    assert_eq!(panel.phase(0.0), Phase::Shown);
    let mut last = panel.opacity(0.0);
    for step in 1..=10 {
        let t = open * f64::from(step) / 10.0;
        let opacity = panel.opacity(t);
        assert!(opacity >= last, "{t}: {opacity} after {last}");
        last = opacity;
    }
    assert!(panel.opening(open / 2.0));
    assert!(!panel.opening(open));
    assert_eq!(panel.opacity(open), 1.0);

    let at = open + 1.0;
    panel.toggle(at);
    assert_eq!(panel.phase(at), Phase::Hiding);
    let left = panel.until_hidden(at).expect("hiding");
    assert!((left - close).abs() < 1e-9, "{left}");
    assert!(panel.opacity(at + close / 2.0) < 1.0);
    let gone = at + close + 1e-9;
    assert_eq!(panel.phase(gone), Phase::Hidden);
    assert_eq!(panel.opacity(gone), 0.0);
    assert_eq!(panel.until_hidden(gone), None);
}

#[test]
fn a_toggle_while_hiding_opens_again_from_where_the_fade_was() {
    let close = lifecycle::close().duration();
    let mut panel = Lifecycle::new();
    panel.toggle(0.0);
    panel.dismiss(5.0);
    let turn = 5.0 + close / 2.0;
    let reached = panel.opacity(turn);
    assert!(reached > 0.0 && reached < 1.0, "{reached}");
    panel.toggle(turn);
    assert_eq!(panel.phase(turn), Phase::Shown);
    assert!((panel.opacity(turn) - reached).abs() < 1e-12);
    // a dismissal of a hidden panel does nothing
    let mut hidden = Lifecycle::new();
    hidden.dismiss(1.0);
    assert_eq!(hidden.phase(1.0), Phase::Hidden);
}

#[test]
fn a_hidden_panel_goes_at_once_and_reduce_motion_drops_the_fades() {
    let mut panel = Lifecycle::new();
    panel.toggle(0.0);
    panel.hide();
    assert_eq!(panel.phase(0.0), Phase::Hidden);
    panel.set_reduce_motion(true);
    panel.toggle(1.0);
    assert_eq!(panel.opacity(1.0), 1.0);
    assert!(!panel.opening(1.0));
    panel.dismiss(2.0);
    assert_eq!(panel.phase(2.0), Phase::Hidden);
    assert_eq!(panel.opacity(2.0), 0.0);
}

#[test]
fn a_detail_view_grows_on_the_spring_and_shrinks_back_from_where_it_was() {
    let length = motion::spring().duration();
    let mut view = Expansion::open(Detail::WiFi, 0.0, false);
    assert_eq!(view.progress(0.0), 0.0);
    let half = view.progress(length / 4.0);
    assert!(half > 0.0 && half < 1.0, "{half}");
    assert!(!view.settled(length / 4.0));
    view.turn(false, length / 4.0);
    assert!((view.progress(length / 4.0) - half).abs() < 1e-12);
    assert!(!view.closed(length / 4.0));
    assert!(view.closed(length / 4.0 + length));
    assert_eq!(view.progress(length / 4.0 + length), 0.0);
    // turning the way it goes already changes nothing
    let mut open = Expansion::open(Detail::Sound, 0.0, false);
    open.turn(true, 0.1);
    assert!(open.settled(length));
    assert_eq!(open.progress(length), 1.0);
}

#[test]
fn with_reduce_motion_a_detail_view_cross_fades_over_the_catalogues_fade() {
    let length = motion::fade().duration();
    let view = Expansion::open(Detail::Display, 0.0, true);
    assert!((view.progress(length / 2.0) - 0.5).abs() < 1e-9);
    assert!(view.settled(length));
}

#[test]
fn the_other_tiles_are_gone_before_the_view_comes_in() {
    let start = visibility(0.0);
    assert_eq!(
        (start.others, start.tile_content, start.detail_content),
        (1.0, 1.0, 0.0)
    );
    assert_eq!(visibility(OTHERS_FADE).others, 0.0);
    let half = visibility(0.5);
    assert_eq!((half.tile_content, half.detail_content), (0.0, 0.0));
    let end = visibility(1.0);
    assert_eq!(
        (end.others, end.tile_content, end.detail_content),
        (0.0, 0.0, 1.0)
    );
    assert_eq!(visibility(2.0), end);
}
