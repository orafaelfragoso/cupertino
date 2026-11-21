//! What the portal asks over `org.cupertino.Dialogs1`, decoded and bounded once more.

use std::collections::HashMap;
use std::path::PathBuf;

use cupertino_shell_dialogs::ask::{
    app_choices, plain, AppAsk, FileAsk, Kind, Pattern, WireChoice, WireFilter, MAX_APPS,
    MAX_CHOICES, MAX_FILTERS, MAX_TEXT,
};
use zbus::zvariant::{OwnedValue, Value};

fn options(pairs: Vec<(&str, Value<'static>)>) -> HashMap<String, OwnedValue> {
    pairs
        .into_iter()
        .map(|(k, v)| {
            (
                k.to_owned(),
                OwnedValue::try_from(v).expect("an owned value"),
            )
        })
        .collect()
}

fn ask(kind: &str, pairs: Vec<(&str, Value<'static>)>) -> Result<FileAsk, String> {
    FileAsk::from_wire(
        kind,
        "org.example.Editor".into(),
        "wayland:abc".into(),
        "Open".into(),
        &options(pairs),
    )
}

#[test]
fn a_file_chooser_reads_its_kind_filters_choices_and_folder() {
    let filters: Vec<WireFilter> = vec![
        ("Text".into(), vec![(0, "*.txt".into())]),
        ("Images".into(), vec![(1, "image/*".into())]),
    ];
    let choices: Vec<WireChoice> = vec![(
        "encoding".into(),
        "Encoding".into(),
        vec![("utf8".into(), "UTF-8".into())],
        "utf8".into(),
    )];
    let ask = ask(
        "open",
        vec![
            ("filters", Value::from(filters)),
            ("current_filter", Value::from(1i32)),
            ("choices", Value::from(choices)),
            ("multiple", Value::from(true)),
            ("current_folder", Value::from(b"/tmp/docs\0".to_vec())),
        ],
    )
    .expect("a valid ask");
    assert_eq!(ask.kind, Kind::Open);
    assert!(ask.multiple && ask.modal && !ask.directory);
    assert_eq!(
        ask.filters[1].patterns,
        vec![Pattern::Mime("image/*".into())]
    );
    assert_eq!(ask.current_filter, Some(1));
    assert_eq!(
        ask.choices[0].options,
        vec![("utf8".into(), "UTF-8".into())]
    );
    assert_eq!(ask.current_folder, Some(PathBuf::from("/tmp/docs")));
}

#[test]
fn an_unknown_kind_and_an_unbounded_value_are_refused() {
    assert!(ask("delete", Vec::new()).is_err());
    let long = "x".repeat(MAX_TEXT + 1);
    assert!(
        FileAsk::from_wire("open", long, String::new(), String::new(), &HashMap::new()).is_err()
    );
    let filters: Vec<WireFilter> = vec![("F".into(), vec![(0, "*".into())]); MAX_FILTERS + 1];
    assert!(ask("open", vec![("filters", Value::from(filters))]).is_err());
    let choices: Vec<WireChoice> =
        vec![("c".into(), "C".into(), Vec::new(), "true".into()); MAX_CHOICES + 1];
    assert!(ask("open", vec![("choices", Value::from(choices))]).is_err());
    let kinds: Vec<WireFilter> = vec![("F".into(), vec![(7, "*".into())])];
    assert!(ask("open", vec![("filters", Value::from(kinds))]).is_err());
}

#[test]
fn a_value_of_the_wrong_type_or_a_path_that_is_not_one_is_refused() {
    assert!(ask("open", vec![("multiple", Value::from("yes"))]).is_err());
    assert!(ask(
        "open",
        vec![("current_folder", Value::from(b"relative".to_vec()))]
    )
    .is_err());
    assert!(ask(
        "open",
        vec![("current_folder", Value::from(b"/a\0b".to_vec()))]
    )
    .is_err());
    assert!(ask("save", vec![("current_name", Value::from("a/b.txt"))]).is_err());
    let files: Vec<Vec<u8>> = vec![b"../escape".to_vec()];
    assert!(ask("save-files", vec![("files", Value::from(files))]).is_err());
}

#[test]
fn several_files_and_folders_are_for_an_open_only_and_a_filter_out_of_range_is_none() {
    let filters: Vec<WireFilter> = vec![("Text".into(), vec![(0, "*.txt".into())])];
    let save = ask(
        "save",
        vec![
            ("multiple", Value::from(true)),
            ("directory", Value::from(true)),
            ("filters", Value::from(filters)),
            ("current_filter", Value::from(3i32)),
        ],
    )
    .expect("a valid ask");
    assert!(!save.multiple && !save.directory);
    assert_eq!(save.current_filter, None);
}

#[test]
fn text_the_dialog_shows_loses_control_and_direction_characters() {
    assert_eq!(plain("a\u{202e}b\u{2066}c\nd\u{200f}"), "abcd");
    let ask = FileAsk::from_wire(
        "open",
        "app".into(),
        String::new(),
        "Pick\u{202e}txt.exe".into(),
        &HashMap::new(),
    )
    .expect("a valid ask");
    assert_eq!(ask.title, "Picktxt.exe");
}

#[test]
fn an_app_chooser_names_no_icon_by_a_path_and_bounds_its_applications() {
    let apps = app_choices(vec![
        ("a".into(), "A".into(), "/etc/passwd".into()),
        ("b".into(), "B\u{202e}".into(), "text-editor".into()),
        ("c".into(), "C".into(), String::new()),
    ])
    .expect("valid applications");
    assert_eq!(apps[0].icon, None);
    assert_eq!(apps[1].icon.as_deref(), Some("text-editor"));
    assert_eq!(apps[1].name, "B");
    assert_eq!(apps[2].icon, None);
    assert!(app_choices(vec![
        (String::new(), String::new(), String::new());
        MAX_APPS + 1
    ])
    .is_err());
    let ask = AppAsk::from_wire(
        "app".into(),
        String::new(),
        String::new(),
        Vec::new(),
        String::new(),
        true,
    )
    .expect("a valid ask");
    assert_eq!(
        (ask.target, ask.last_choice, ask.wants_token),
        (None, None, true)
    );
}
