//! The sharing chooser's model (`cupertino_screenshot::share_picker`): how its rows name the sources, that nothing is
//! chosen or shared before it is armed, what it answers, and when it offers to remember.

use std::time::Instant;

use cupertino_kit::controls::Selection;
use cupertino_screenshot::color_picker::ARMING;
use cupertino_screenshot::share_picker::{
    input_id, prompt, window_key, Ask, Input, Picker, Source, SourceKind,
};

fn ask(offer_remember: bool) -> Ask {
    Ask {
        app_name: Some("Viewer".into()),
        app_icon: None,
        parent_window: String::new(),
        sources: vec![
            Source::display("DP-1", "Dell P2723QE"),
            Source::window("Notes", "org.example.Notes"),
            Source::window("Terminal", ""),
        ],
        offer_remember,
    }
}

/// A chooser shown ARMING ago and armed by its timer.
fn armed(offer_remember: bool) -> Picker {
    let mut picker = Picker::new(7, ask(offer_remember));
    assert!(picker.shown(
        Instant::now()
            .checked_sub(ARMING)
            .expect("a clock past ARMING")
    ));
    picker.arm();
    picker
}

fn row(index: usize) -> Selection {
    let mut rows = Selection::default();
    rows.select_only(index);
    rows
}

#[test]
fn a_row_names_its_source() {
    assert_eq!(
        Source::display("DP-1", "Dell P2723QE"),
        Source {
            kind: SourceKind::Display,
            label: "Dell P2723QE".into()
        }
    );
    assert_eq!(Source::display("DP-1", "").label, "DP-1");
    assert_eq!(
        Source::window("Notes", "org.example.Notes").label,
        "Notes (org.example.Notes)"
    );
    assert_eq!(Source::window("Notes", "").label, "Notes");
    assert_eq!(
        Source::window("", "org.example.Notes").label,
        "org.example.Notes"
    );
    assert_eq!(Source::window("", "").kind, SourceKind::Window);
}

#[test]
fn the_prompt_names_the_app_and_says_when_there_is_nothing_to_share() {
    let mut asked = ask(false);
    assert_eq!(
        prompt(&asked),
        "“Viewer” wants to share your screen. Choose a display or a window."
    );
    asked.app_name = None;
    asked.sources.clear();
    assert_eq!(
        prompt(&asked),
        "An application wants to share your screen. There is no display or window to share."
    );
}

#[test]
fn nothing_is_chosen_or_shared_before_the_chooser_is_armed() {
    let mut picker = Picker::new(7, ask(true));
    assert_eq!(picker.input(Input::Select(7, row(1))), None);
    assert_eq!(
        picker.chosen(),
        None,
        "a key typed for the app chooses nothing"
    );
    assert_eq!(picker.input(Input::Share(7)), None);

    // shown just now and armed by a timer that came early: the clock says no
    assert!(picker.shown(Instant::now()));
    assert!(
        !picker.shown(Instant::now()),
        "the first frame starts the delay once"
    );
    picker.arm();
    assert_eq!(picker.input(Input::Select(7, row(1))), None);
    assert_eq!(picker.chosen(), None);
}

#[test]
fn an_armed_chooser_shares_the_row_chosen() {
    let mut picker = armed(false);
    assert_eq!(picker.input(Input::Share(7)), None, "no row is chosen yet");
    assert_eq!(picker.input(Input::Select(7, row(2))), None);
    assert_eq!(picker.chosen(), Some(2));
    assert_eq!(picker.input(Input::Share(7)), Some(Some((2, false))));
}

#[test]
fn remember_is_answered_only_when_the_app_asked_for_it() {
    let mut offered = armed(true);
    offered.input(Input::Select(7, row(0)));
    offered.input(Input::Remember(7, true));
    assert_eq!(offered.input(Input::Share(7)), Some(Some((0, true))));

    let mut not_offered = armed(false);
    not_offered.input(Input::Select(7, row(0)));
    not_offered.input(Input::Remember(7, true));
    assert_eq!(not_offered.input(Input::Share(7)), Some(Some((0, false))));
}

#[test]
fn a_row_past_the_sources_is_not_chosen() {
    let mut picker = armed(false);
    picker.input(Input::Select(7, row(3)));
    assert_eq!(picker.chosen(), None);
    assert_eq!(picker.input(Input::Share(7)), None);
}

#[test]
fn cancel_ends_the_chooser_at_any_time() {
    let mut picker = Picker::new(7, ask(false));
    assert_eq!(picker.input(Input::Cancel(7)), Some(None));
    assert_eq!(input_id(&Input::Cancel(9)), 9);
    assert_ne!(window_key(7), window_key(8));
}
