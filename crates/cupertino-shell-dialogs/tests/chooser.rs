//! One file chooser and one app chooser, without drawing: the folders they read, what the person does, and the answer
//! each sends to its call.

// a test writes its folders on its own thread, as the I/O pool would read them
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use cupertino_kit::controls::Selection;
use cupertino_kit::controls::TextChange;
use cupertino_kit::core::Command;
use cupertino_kit::files::panel::{ChoiceValue, PanelEvent};
use cupertino_kit::files::ViewEvent;
use cupertino_shell_dialogs::apps::{AppDialog, AppEvent};
use cupertino_shell_dialogs::ask::{
    AppAnswer, AppAsk, AppChoice, Choice, FileAnswer, FileAsk, Filter, Kind, Pattern,
};
use cupertino_shell_dialogs::chooser::{glob, matches, takes, Accept};
use cupertino_shell_dialogs::file::{FileDialog, FileEvent};
use cupertino_shell_dialogs::Message;

/// A folder of its own for each test, with `files` and `folders` in it.
fn folder(test: &str, files: &[&str], folders: &[&str]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("dialogs")
        .join(test);
    let _ = std::fs::remove_dir_all(&dir);
    for f in folders {
        std::fs::create_dir_all(dir.join(f)).expect("a folder");
    }
    std::fs::create_dir_all(&dir).expect("the folder");
    for f in files {
        std::fs::write(dir.join(f), b"x").expect("a file");
    }
    dir
}

fn ask(kind: Kind, dir: &Path) -> FileAsk {
    FileAsk {
        kind,
        app_id: "org.example.Editor".into(),
        parent_window: String::new(),
        title: String::new(),
        accept_label: None,
        modal: true,
        multiple: false,
        directory: false,
        filters: Vec::new(),
        current_filter: None,
        choices: Vec::new(),
        current_name: None,
        current_folder: Some(dir.to_path_buf()),
        current_file: None,
        files: Vec::new(),
    }
}

/// The dialog with its folders read, as the component's tasks would, and the receiver of its answer.
fn dialog(ask: FileAsk, home: &Path) -> (FileDialog, async_channel::Receiver<FileAnswer>) {
    let (reply, answer) = async_channel::bounded(1);
    let mut d = FileDialog::new(1, "/request/1".into(), ask, reply, home);
    read(&mut d);
    (d, answer)
}

/// Read the folder the dialog asks for, as the component's task would.
fn read(d: &mut FileDialog) {
    while let Some(request) = d.take_request() {
        let (done, _) = d.update(FileEvent::Listing(request.run()), &[]);
        assert!(!done, "a listing ends no dialog");
    }
}

/// Carry out `event` and read the folder it moves to; true when the dialog is done.
fn act(d: &mut FileDialog, event: FileEvent) -> bool {
    let (done, _) = d.update(event, &[]);
    if !done {
        read(d);
    }
    done
}

fn row(d: &FileDialog, name: &str) -> usize {
    d.items()
        .iter()
        .position(|e| e.name == name)
        .unwrap_or_else(|| panic!("no row {name}"))
}

fn select(d: &mut FileDialog, row: usize) {
    let mut rows = Selection::default();
    rows.select_only(row);
    act(d, FileEvent::List(ViewEvent::Select(rows)));
}

fn answer(d: FileDialog, answer: &async_channel::Receiver<FileAnswer>) -> FileAnswer {
    d.finish();
    answer.try_recv().expect("an answer")
}

#[test]
fn globs_and_types_match_names_without_regard_to_case() {
    assert!(glob("*.TXT", "notes.txt"));
    assert!(glob("[a-c]?.md", "B1.md"));
    assert!(!glob("[!a-c]*", "apple"));
    assert!(glob("[unclosed", "[unclosed"));
    let images = Filter {
        name: "Images".into(),
        patterns: vec![Pattern::Mime("image/*".into())],
    };
    assert!(matches(&images, "Photo.PNG"));
    assert!(!matches(&images, "notes.txt"));
    let check = Choice {
        id: "c".into(),
        label: "C".into(),
        options: Vec::new(),
        initial: "true".into(),
    };
    assert!(takes(&check, "false") && !takes(&check, "maybe"));
}

#[test]
fn an_open_chooses_the_file_selected_under_its_filter() {
    let dir = folder("open", &["a.txt", "b.png"], &["sub"]);
    let mut ask = ask(Kind::Open, &dir);
    ask.filters = vec![Filter {
        name: "Text".into(),
        patterns: vec![Pattern::Glob("*.txt".into())],
    }];
    let (mut d, answer_rx) = dialog(ask, &dir);
    let names: Vec<&str> = d.items().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        ["a.txt", "sub"],
        "the filter hides the picture, never a folder"
    );
    assert_eq!(
        d.chooser().accept(),
        Accept::Nothing,
        "nothing is selected yet"
    );
    let a = row(&d, "a.txt");
    select(&mut d, a);
    assert!(act(&mut d, FileEvent::Return));
    assert_eq!(
        answer(d, &answer_rx),
        FileAnswer::Chosen {
            paths: vec![dir.join("a.txt")],
            filter: Some(0),
            choices: Vec::new(),
        }
    );
}

#[test]
fn return_on_a_folder_opens_it_and_enclosing_folder_goes_back() {
    let dir = folder("folders", &[], &["sub"]);
    std::fs::write(dir.join("sub/inner.txt"), b"x").expect("a file");
    let (mut d, _answer) = dialog(ask(Kind::Open, &dir), &dir);
    let sub = row(&d, "sub");
    select(&mut d, sub);
    assert!(!act(&mut d, FileEvent::Return));
    assert_eq!(d.chooser().picker().dir(), dir.join("sub"));
    assert_eq!(d.items()[0].name, "inner.txt");
    assert!(!act(&mut d, FileEvent::Panel(PanelEvent::Parent)));
    assert_eq!(d.chooser().picker().dir(), dir);
}

#[test]
fn a_save_takes_the_typed_name_and_asks_before_it_replaces_a_file() {
    let dir = folder("save", &["taken.txt"], &[]);
    let mut ask = ask(Kind::Save, &dir);
    ask.current_name = Some("Untitled.txt".into());
    ask.choices = vec![Choice {
        id: "encoding".into(),
        label: "Encoding".into(),
        options: vec![
            ("utf8".into(), "UTF-8".into()),
            ("latin1".into(), "Latin-1".into()),
        ],
        initial: "utf8".into(),
    }];
    let (mut d, answer_rx) = dialog(ask, &dir);
    act(
        &mut d,
        FileEvent::Panel(PanelEvent::Choice(0, ChoiceValue::Option(1))),
    );
    let typed = |text: &str| {
        FileEvent::Panel(PanelEvent::Name(TextChange {
            text: text.into(),
            revision: 3,
        }))
    };
    act(&mut d, typed("taken.txt"));
    assert!(
        !act(&mut d, FileEvent::Return),
        "a file of that name is asked about first"
    );
    assert_eq!(d.replacing(), Some(dir.join("taken.txt").as_path()));
    act(&mut d, FileEvent::Panel(PanelEvent::KeepEditing));
    assert_eq!(d.replacing(), None);
    act(&mut d, typed("Report.txt"));
    assert!(act(&mut d, FileEvent::Panel(PanelEvent::Accept)));
    assert_eq!(
        answer(d, &answer_rx),
        FileAnswer::Chosen {
            paths: vec![dir.join("Report.txt")],
            filter: None,
            choices: vec![("encoding".into(), "latin1".into())],
        }
    );
}

#[test]
fn replacing_a_file_answers_with_it() {
    let dir = folder("replace", &["taken.txt"], &[]);
    let mut ask = ask(Kind::Save, &dir);
    ask.current_name = Some("taken.txt".into());
    let (mut d, answer_rx) = dialog(ask, &dir);
    assert!(!act(&mut d, FileEvent::Panel(PanelEvent::Accept)));
    assert!(
        act(&mut d, FileEvent::Return),
        "Return is the question's Replace"
    );
    assert!(
        matches!(answer(d, &answer_rx), FileAnswer::Chosen { paths, .. } if paths == [dir.join("taken.txt")])
    );
}

#[test]
fn saving_files_puts_them_in_the_folder_shown() {
    let dir = folder("save-files", &[], &[]);
    let mut ask = ask(Kind::SaveFiles, &dir);
    ask.files = vec!["one.txt".into(), "two.txt".into()];
    let (mut d, answer_rx) = dialog(ask, &dir);
    assert!(act(&mut d, FileEvent::Panel(PanelEvent::Accept)));
    assert!(matches!(
        answer(d, &answer_rx),
        FileAnswer::Chosen { paths, .. } if paths == [dir.join("one.txt"), dir.join("two.txt")]
    ));
}

#[test]
fn cancel_answers_cancelled_and_a_dialog_dropped_unanswered_ends_its_call() {
    let dir = folder("cancel", &[], &[]);
    let (mut d, answer_rx) = dialog(ask(Kind::Open, &dir), &dir);
    assert!(act(&mut d, FileEvent::Cancel));
    assert_eq!(answer(d, &answer_rx), FileAnswer::Cancelled);
    let (d, answer_rx) = dialog(ask(Kind::Open, &dir), &dir);
    drop(d);
    assert!(answer_rx.try_recv().is_err() && answer_rx.is_closed());
}

fn apps(wants_token: bool) -> (AppDialog, async_channel::Receiver<AppAnswer>) {
    let (reply, answer) = async_channel::bounded(1);
    let choice = |id: &str| AppChoice {
        id: id.into(),
        name: id.to_uppercase(),
        icon: None,
    };
    let ask = AppAsk {
        app_id: "org.example.Mail".into(),
        parent_window: String::new(),
        target: Some("Notes.txt".into()),
        choices: vec![choice("a"), choice("b"), choice("c")],
        last_choice: Some("b".into()),
        wants_token,
    };
    (AppDialog::new(2, "/request/2".into(), ask, reply), answer)
}

#[test]
fn an_app_chooser_starts_on_the_last_choice_and_keeps_it_when_the_list_changes() {
    let (mut d, _answer) = apps(false);
    assert_eq!(d.chosen().map(|c| c.id.as_str()), Some("b"));
    d.update_choices(vec![AppChoice {
        id: "b".into(),
        name: "B".into(),
        icon: None,
    }]);
    assert_eq!(d.rows().cursor(), Some(0));
    d.update_choices(Vec::new());
    assert_eq!(d.chosen(), None);
    assert!(!d.update(AppEvent::Open).0, "nothing to open");
}

#[test]
fn an_app_chooser_answers_the_application_chosen() {
    let (mut d, answer) = apps(false);
    let mut rows = Selection::default();
    rows.select_only(2);
    d.update(AppEvent::Select(rows));
    assert!(d.update(AppEvent::Open).0);
    d.finish();
    assert_eq!(
        answer.try_recv(),
        Ok(AppAnswer::Chosen {
            id: "c".into(),
            token: None
        })
    );
}

#[test]
fn an_app_chooser_that_wants_a_token_answers_once_the_server_made_it() {
    let (mut d, answer) = apps(true);
    let (done, commands) = d.update(AppEvent::Open);
    assert!(!done);
    let Some(Command::ActivationToken {
        app_id, on_token, ..
    }) = commands.into_iter().next()
    else {
        panic!("an activation token is asked for");
    };
    assert_eq!(app_id.as_deref(), Some("b"));
    let Message::App(2, opened) = on_token(Some("token-1".into())) else {
        panic!("the token comes back to this dialog");
    };
    assert!(d.update(opened).0);
    d.finish();
    assert_eq!(
        answer.try_recv(),
        Ok(AppAnswer::Chosen {
            id: "b".into(),
            token: Some("token-1".into())
        })
    );
}

#[test]
fn the_app_choosers_question_names_its_target_shortened() {
    use cupertino_shell_dialogs::view::question;
    assert_eq!(question(Some("Notes.txt"), true), "Open “Notes.txt” with:");
    assert_eq!(question(None, false), "There is no app to open it with.");
    let long = "x".repeat(200);
    assert_eq!(
        question(Some(&long), true).chars().count(),
        "Open “” with:".chars().count() + 81
    );
}
