//! What the portal sends the shell's dialogs over `org.cupertino.Dialogs1`, and how it reads their answers.

use std::collections::HashMap;
use std::path::PathBuf;

use cupertino_portal::app_chooser;
use cupertino_portal::dialogs::{
    app_answer, app_choices, file_answer, file_options, kind_name, Ended,
};
use cupertino_portal::file_chooser::{parse, Answer, Filter, Kind, Pattern, WireFilter};
use cupertino_portal::screenshot::LocalTime;
use zbus::zvariant::{OwnedValue, Value};

fn ask() -> cupertino_portal::file_chooser::Ask {
    let filters: Vec<WireFilter> = vec![
        ("Text".into(), vec![(0, "*.txt".into())]),
        ("Images".into(), vec![(1, "image/*".into())]),
    ];
    let options: HashMap<String, OwnedValue> = [
        ("filters", Value::from(filters)),
        (
            "current_filter",
            Value::from(("Images".to_owned(), vec![(1u32, "image/*".to_owned())])),
        ),
        (
            "current_folder",
            Value::from(b"/home/avery/Pictures\0".to_vec()),
        ),
        ("current_name", Value::from("Photo.png")),
        ("modal", Value::from(false)),
    ]
    .into_iter()
    .map(|(k, v)| {
        (
            k.to_owned(),
            OwnedValue::try_from(v).expect("an owned value"),
        )
    })
    .collect();
    parse(
        Kind::Save,
        "org.example.Paint".into(),
        "wayland:abc".into(),
        "Save".into(),
        &options,
    )
    .expect("a valid call")
}

#[test]
fn a_file_chooser_is_asked_with_its_checked_options_and_the_filter_as_an_index() {
    let ask = ask();
    assert_eq!(kind_name(ask.kind), "save");
    let options = file_options(&ask);
    assert_eq!(options["current_filter"], Value::from(1i32));
    assert_eq!(options["modal"], Value::from(false));
    assert_eq!(options["current_name"], Value::from("Photo.png"));
    assert_eq!(
        options["current_folder"],
        Value::from(b"/home/avery/Pictures".to_vec()),
        "a path goes without its terminating NUL"
    );
    assert!(!options.contains_key("accept_label"));
}

#[test]
fn a_filter_the_list_does_not_hold_is_sent_as_none() {
    let mut ask = ask();
    ask.current_filter = Some(Filter {
        name: "Other".into(),
        patterns: vec![Pattern::Glob("*".into())],
    });
    assert_eq!(file_options(&ask)["current_filter"], Value::from(-1i32));
}

#[test]
fn a_chosen_answer_names_the_filter_of_the_call_and_any_other_ended_none() {
    let ask = ask();
    let chosen = file_answer(
        &ask,
        (
            0,
            vec![b"/home/avery/Pictures/Photo.png".to_vec()],
            1,
            vec![("q".into(), "high".into())],
        ),
    );
    assert_eq!(
        chosen,
        Ended::Answered(Answer::Chosen {
            paths: vec![PathBuf::from("/home/avery/Pictures/Photo.png")],
            filter: Some(ask.filters[1].clone()),
            choices: vec![("q".into(), "high".into())],
        })
    );
    let no_filter = file_answer(&ask, (0, Vec::new(), 7, Vec::new()));
    assert!(matches!(
        no_filter,
        Ended::Answered(Answer::Chosen { filter: None, .. })
    ));
    assert_eq!(
        file_answer(&ask, (1, Vec::new(), -1, Vec::new())),
        Ended::Answered(Answer::Cancelled)
    );
    assert_eq!(
        file_answer(&ask, (2, Vec::new(), -1, Vec::new())),
        Ended::Gone
    );
}

#[test]
fn an_app_chooser_sends_its_applications_and_reads_the_token_made() {
    let choices = [app_chooser::Choice {
        id: "org.example.Writer".into(),
        name: "Writer".into(),
        icon: None,
    }];
    assert_eq!(
        app_choices(&choices),
        vec![("org.example.Writer".into(), "Writer".into(), String::new())]
    );
    assert_eq!(
        app_answer((0, "org.example.Writer".into(), "t-1".into())),
        Ended::Answered(app_chooser::Answer::Chosen {
            id: "org.example.Writer".into(),
            token: Some("t-1".into()),
        })
    );
    assert_eq!(
        app_answer((0, "org.example.Writer".into(), String::new())),
        Ended::Answered(app_chooser::Answer::Chosen {
            id: "org.example.Writer".into(),
            token: None,
        })
    );
    assert_eq!(
        app_answer((1, String::new(), String::new())),
        Ended::Answered(app_chooser::Answer::Cancelled)
    );
    assert_eq!(app_answer((2, String::new(), String::new())), Ended::Gone);
}

#[test]
fn a_screenshot_is_named_by_the_local_time() {
    let time = LocalTime::parse("2026-10-25T15:04:05").expect("a time");
    assert_eq!(time.base_name(), "Screenshot 2026-10-25 at 3-04-05 PM");
    assert!(LocalTime::parse("2026-10-25").is_none());
    assert!(LocalTime::now().is_ok());
}
