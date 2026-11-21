//! The stage policy through the crate's interface: what the controller asks of the server and what its strip shows for
//! the server's groups of window events, and `windowmanager.conf`.

use cupertino_windowmanager::{parse_conf, Change, Controller, Request, Window};
use cupertino_wm::geometry::Rect;
use cupertino_wm::stage::StageConfig;

/// An output of 800 x 600 points: four rows of strip.
const USABLE: Rect = Rect::new(0.0, 0.0, 800.0, 600.0);
/// The rows the strip has at that height.
const ROWS: usize = 4;

/// Windows `1..=n`, each of an application of its own, the last the key window.
fn windows(n: u32) -> Vec<Change> {
    let mut group = vec![Change::UsableArea(USABLE)];
    group.extend((1..=n).map(|id| {
        Change::Window(Window {
            id,
            app_id: format!("org.example.app{id}"),
            frame: Rect::new(300.0, 100.0, 300.0, 200.0),
            active: id == n,
        })
    }));
    group
}

fn hidden(requests: &[Request]) -> Vec<u32> {
    requests
        .iter()
        .filter_map(|r| match r {
            Request::Place { id, hidden: true } => Some(*id),
            _ => None,
        })
        .collect()
}

#[test]
fn the_stage_off_places_nothing_and_shows_no_strip() {
    let mut controller = Controller::new(StageConfig::default(), false);
    assert!(controller.apply(&windows(6), 0.0).is_empty());
    assert!(controller.strip().is_empty());
    assert!(controller.hidden().is_empty());
}

#[test]
fn the_stage_on_shows_the_other_groups_in_the_strip_and_hides_those_beyond_its_rows() {
    let mut controller = Controller::new(StageConfig::default(), true);
    let requests = controller.apply(&windows(6), 0.0);
    let strip = controller.strip();
    assert_eq!(strip.len(), ROWS, "a thumbnail per row");
    assert!(!strip.contains_key(&6), "the key window is on the stage");
    assert_eq!(hidden(&requests), vec![1], "the oldest group is hidden");
    assert_eq!(requests.last(), Some(&Request::Commit));
    let column = controller.strip_column();
    for rect in strip.values() {
        assert!(
            rect.x >= column.x && rect.right() <= column.right(),
            "{rect:?} rests in the strip's column"
        );
    }
}

#[test]
fn a_press_on_a_thumbnail_brings_its_group_to_the_stage_and_activates_it() {
    let mut controller = Controller::new(StageConfig::default(), true);
    controller.apply(&windows(3), 0.0);
    assert!(controller.strip().contains_key(&1));
    let requests = controller.apply(&[Change::StripPressed(1)], 1.0);
    assert_eq!(requests.last(), Some(&Request::Activate(1)));
    let strip = controller.strip();
    assert!(!strip.contains_key(&1), "the pressed group left the strip");
    assert!(
        strip.contains_key(&3),
        "the group on the stage went to the strip"
    );
}

#[test]
fn turning_the_stage_off_shows_every_window_again() {
    let mut controller = Controller::new(StageConfig::default(), true);
    controller.apply(&windows(6), 0.0);
    let requests = controller.apply(&[Change::ToggleStage], 1.0);
    assert_eq!(
        requests,
        vec![
            Request::Place {
                id: 1,
                hidden: false
            },
            Request::Commit
        ]
    );
    assert!(!controller.is_enabled());
    assert!(controller.strip().is_empty());
    assert!(controller.hidden().is_empty());
}

#[test]
fn a_closed_window_leaves_the_strip() {
    let mut controller = Controller::new(StageConfig::default(), true);
    controller.apply(&windows(3), 0.0);
    controller.apply(&[Change::WindowClosed(2)], 1.0);
    assert!(!controller.strip().contains_key(&2));
}

#[test]
fn the_conf_sets_the_stage_and_reports_what_it_cannot_use() {
    let (file, problems) = parse_conf("# the stage\nstage = on\nstage = maybe\ncolour = blue\n");
    assert_eq!(file.stage, Some(true));
    assert_eq!(
        problems.iter().map(|p| p.line).collect::<Vec<_>>(),
        vec![3, 4]
    );
}
