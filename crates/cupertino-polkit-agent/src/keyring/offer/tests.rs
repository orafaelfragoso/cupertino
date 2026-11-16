use super::*;

const OTHER: &str = "/org/freedesktop/secrets/collection/other";

fn observed(default: Option<&str>, login_locked: Option<bool>) -> Observed {
    Observed {
        default: default.map(|path| (path.to_owned(), "Other".to_owned())),
        login_locked,
    }
}

fn offer() -> Offer {
    Offer {
        collection: OTHER.into(),
        label: "Other".into(),
    }
}

fn record(label: &str, account: &str, secret: &str) -> Record {
    Record {
        label: label.into(),
        attributes: [
            ("service".to_owned(), "mail".to_owned()),
            ("account".to_owned(), account.to_owned()),
        ]
        .into(),
        secret: Zeroizing::new(secret.as_bytes().to_vec()),
        content_type: "text/plain".into(),
    }
}

/// Keyrings in memory. A call named in `fail` fails; `corrupt` stores a copy that differs from what was asked.
#[derive(Default)]
struct Fake {
    default: String,
    items: Vec<(String, String, Record)>,
    next: u32,
    fail: Vec<&'static str>,
    /// Fail the creation of the item with this number (counted from 0) of a run.
    fail_create_at: Option<usize>,
    created: usize,
    corrupt: bool,
    /// The store accepts a new default and keeps the old one.
    ignore_default: bool,
}

impl Fake {
    fn new(other: &[Record], login: &[Record]) -> Fake {
        let mut fake = Fake {
            default: OTHER.into(),
            ..Fake::default()
        };
        for record in other {
            fake.put(OTHER, record.clone());
        }
        for record in login {
            fake.put(LOGIN, record.clone());
        }
        fake
    }

    fn put(&mut self, collection: &str, record: Record) -> String {
        self.next += 1;
        let path = format!("{collection}/{}", self.next);
        self.items
            .push((path.clone(), collection.to_owned(), record));
        path
    }

    fn records(&self, collection: &str) -> Vec<Record> {
        self.items
            .iter()
            .filter(|(_, c, _)| c == collection)
            .map(|(_, _, r)| r.clone())
            .collect()
    }

    fn check(&self, call: &'static str) -> Result<(), StoreError> {
        if self.fail.contains(&call) {
            return Err(StoreError::new(call, "org.test.Error.Failed"));
        }
        Ok(())
    }
}

impl Store for Fake {
    fn default_collection(&mut self) -> Result<String, StoreError> {
        self.check("default")?;
        Ok(self.default.clone())
    }

    fn set_default(&mut self, collection: &str) -> Result<(), StoreError> {
        self.check("set_default")?;
        if !self.ignore_default {
            self.default = collection.to_owned();
        }
        Ok(())
    }

    fn items(&mut self, collection: &str) -> Result<Vec<String>, StoreError> {
        self.check("items")?;
        Ok(self
            .items
            .iter()
            .filter(|(_, c, _)| c == collection)
            .map(|(path, _, _)| path.clone())
            .collect())
    }

    fn attributes(&mut self, item: &str) -> Result<BTreeMap<String, String>, StoreError> {
        self.check("attributes")?;
        self.items
            .iter()
            .find(|(path, _, _)| path == item)
            .map(|(_, _, r)| r.attributes.clone())
            .ok_or(StoreError::new("attributes", "org.test.Error.NoSuchObject"))
    }

    fn read(&mut self, item: &str) -> Result<Record, StoreError> {
        self.check("read")?;
        self.items
            .iter()
            .find(|(path, _, _)| path == item)
            .map(|(_, _, r)| r.clone())
            .ok_or(StoreError::new("read", "org.test.Error.NoSuchObject"))
    }

    fn create(&mut self, collection: &str, record: &Record) -> Result<String, StoreError> {
        self.check("create")?;
        if self.fail_create_at == Some(self.created) {
            return Err(StoreError::new("create", "org.test.Error.Failed"));
        }
        self.created += 1;
        let mut record = record.clone();
        if self.corrupt {
            record.secret.push(b'!');
        }
        Ok(self.put(collection, record))
    }

    fn delete(&mut self, item: &str) -> Result<(), StoreError> {
        self.check("delete")?;
        self.items.retain(|(path, _, _)| path != item);
        Ok(())
    }
}

fn three() -> Vec<Record> {
    vec![
        record("Mail", "ana", "s3cret-ana"),
        record("Mail", "ben", "s3cret-ben"),
        record("Chat", "ana", "s3cret-chat"),
    ]
}

// ---- the question ----

#[test]
fn the_question_is_asked_only_for_a_default_keyring_other_than_an_unlocked_login_one() {
    assert_eq!(
        Offer::of(Some(&observed(Some(OTHER), Some(false)))),
        Some(offer())
    );
    // no Secret Service, no default, the login keyring the default, missing or locked
    assert_eq!(Offer::of(None), None);
    assert_eq!(Offer::of(Some(&observed(None, Some(false)))), None);
    assert_eq!(Offer::of(Some(&observed(Some(LOGIN), Some(false)))), None);
    assert_eq!(Offer::of(Some(&observed(Some(OTHER), None))), None);
    assert_eq!(Offer::of(Some(&observed(Some(OTHER), Some(true)))), None);
}

#[test]
fn the_texts_name_the_keyring_and_say_that_a_decline_is_final() {
    let text = offer().description();
    assert!(text.contains("\u{201c}Other\u{201d}"), "{text}");
    assert!(text.contains(KEEP), "{text}");
    let unnamed = Offer {
        label: " \n".into(),
        ..offer()
    };
    assert!(unnamed
        .description()
        .starts_with("The default keyring stays"));
    let long = Offer {
        label: "A keyring\nwith a long name".into(),
        ..offer()
    };
    assert!(long
        .description()
        .starts_with("The keyring \u{201c}A keyringwit\u{2026}\u{201d} stays"));
}

#[test]
fn what_is_remembered_reads_back_and_anything_else_is_nothing() {
    for kept in [Remembered::Declined, Remembered::Moved] {
        assert_eq!(Remembered::parse(kept.as_str()), kept);
    }
    assert_eq!(Remembered::parse("declined\n"), Remembered::Declined);
    assert_eq!(Remembered::parse(""), Remembered::Nothing);
    assert_eq!(Remembered::parse("yes"), Remembered::Nothing);
}

// ---- the flow ----

/// A flow whose question is on screen.
fn asking() -> Flow {
    let (mut flow, steps) = Flow::start(Remembered::Nothing, false);
    assert_eq!(steps, [Step::Observe]);
    assert_eq!(
        flow.observed(Some(&observed(Some(OTHER), Some(false)))),
        [Step::Ask(offer())]
    );
    flow
}

#[test]
fn nothing_is_read_or_asked_after_a_remembered_answer_or_a_second_time_in_a_login() {
    for (remembered, asked) in [
        (Remembered::Declined, false),
        (Remembered::Moved, false),
        (Remembered::Nothing, true),
    ] {
        let (flow, steps) = Flow::start(remembered, asked);
        assert_eq!(steps, [Step::End(Outcome::Quiet)]);
        assert!(flow.ended());
    }
}

#[test]
fn nothing_is_asked_when_there_is_nothing_to_offer() {
    for seen in [
        None,
        Some(observed(Some(LOGIN), Some(false))),
        Some(observed(Some(OTHER), Some(true))),
        Some(observed(Some(OTHER), None)),
    ] {
        let (mut flow, _) = Flow::start(Remembered::Nothing, false);
        assert_eq!(flow.observed(seen.as_ref()), [Step::End(Outcome::Quiet)]);
        assert!(flow.ended());
    }
}

#[test]
fn accepting_unlocks_the_old_keyring_moves_and_remembers() {
    let mut flow = asking();
    assert_eq!(flow.answered(Answer::Yes), [Step::Unlock(OTHER.into())]);
    assert_eq!(flow.unlocked(Unlocked::Yes), [Step::Move(OTHER.into())]);
    let moved = Moved {
        copied: 3,
        already: 0,
    };
    assert_eq!(
        flow.moved(Ok(moved)),
        [
            Step::Remember(Remembered::Moved),
            Step::End(Outcome::Moved(moved))
        ]
    );
    assert!(flow.ended());
}

#[test]
fn declining_is_remembered_and_changes_nothing() {
    let mut flow = asking();
    assert_eq!(
        flow.answered(Answer::No),
        [
            Step::Remember(Remembered::Declined),
            Step::End(Outcome::Declined)
        ]
    );
    assert!(flow.ended());
    // the next login reads the remembered answer
    let (_, steps) = Flow::start(Remembered::Declined, false);
    assert_eq!(steps, [Step::End(Outcome::Quiet)]);
}

#[test]
fn a_question_the_session_took_away_is_no_answer() {
    let mut flow = asking();
    assert_eq!(flow.answered(Answer::Dismissed), [Step::End(Outcome::Left)]);
}

#[test]
fn cancelling_the_old_keyrings_password_is_not_a_decline() {
    let mut flow = asking();
    flow.answered(Answer::Yes);
    assert_eq!(
        flow.unlocked(Unlocked::Cancelled),
        [Step::End(Outcome::Left)]
    );
    // nothing was remembered, so the next login asks again, and this login (which asked) does not
    let (_, steps) = Flow::start(Remembered::Nothing, true);
    assert_eq!(steps, [Step::End(Outcome::Quiet)]);
    let (_, steps) = Flow::start(Remembered::Nothing, false);
    assert_eq!(steps, [Step::Observe]);
}

#[test]
fn a_failure_is_told_remembers_nothing_and_may_be_tried_again() {
    let failure = MoveError {
        step: FailedStep::Copy,
        cause: Some(StoreError::new("create", "org.test.Error.Failed")),
        left: 0,
    };
    let mut flow = asking();
    flow.answered(Answer::Yes);
    flow.unlocked(Unlocked::Yes);
    let steps = flow.moved(Err(failure.clone()));
    let [Step::Tell(notice)] = &steps[..] else {
        panic!("{steps:?}");
    };
    assert!(
        notice.description.contains("Nothing was changed"),
        "{notice:?}"
    );
    // try again: the keyring is unlocked and moved again
    assert_eq!(flow.answered(Answer::Yes), [Step::Unlock(OTHER.into())]);
    assert_eq!(flow.unlocked(Unlocked::Yes), [Step::Move(OTHER.into())]);
    flow.moved(Err(failure));
    assert_eq!(flow.answered(Answer::No), [Step::End(Outcome::Failed)]);
    assert!(flow.ended());

    // an unlock that fails is told too
    let mut flow = asking();
    flow.answered(Answer::Yes);
    let steps = flow.unlocked(Unlocked::Failed);
    assert!(matches!(&steps[..], [Step::Tell(_)]), "{steps:?}");
    assert_eq!(
        flow.answered(Answer::Dismissed),
        [Step::End(Outcome::Failed)]
    );
}

#[test]
fn copies_left_behind_are_told() {
    let notice = Notice::of(&MoveError {
        step: FailedStep::Default,
        cause: None,
        left: 2,
    });
    assert!(!notice.description.contains("Nothing was changed"));
    assert!(notice.description.contains("could not be removed"));
}

#[test]
fn reports_out_of_turn_change_nothing() {
    let mut flow = asking();
    assert_eq!(flow.unlocked(Unlocked::Yes), []);
    assert_eq!(
        flow.moved(Ok(Moved {
            copied: 0,
            already: 0
        })),
        []
    );
    assert_eq!(flow.observed(None), []);
    assert_eq!(flow.answered(Answer::Yes), [Step::Unlock(OTHER.into())]);
    assert_eq!(flow.answered(Answer::No), []);
}

// ---- the move ----

#[test]
fn every_item_is_copied_and_then_the_login_keyring_is_the_default() {
    let mut store = Fake::new(&three(), &[record("Wi-Fi", "home", "s3cret-wifi")]);
    assert_eq!(
        move_items(&mut store, OTHER, LOGIN),
        Ok(Moved {
            copied: 3,
            already: 0
        })
    );
    assert_eq!(store.default, LOGIN);
    // the old keyring keeps its items; the login keyring holds its own and the copies
    assert_eq!(store.records(OTHER), three());
    let mut login = vec![record("Wi-Fi", "home", "s3cret-wifi")];
    login.extend(three());
    assert_eq!(store.records(LOGIN), login);
}

#[test]
fn an_empty_keyring_only_changes_the_default() {
    let mut store = Fake::new(&[], &[]);
    assert_eq!(
        move_items(&mut store, OTHER, LOGIN),
        Ok(Moved {
            copied: 0,
            already: 0
        })
    );
    assert_eq!(store.default, LOGIN);
}

#[test]
fn a_login_item_that_differs_is_kept_beside_the_copy() {
    let mine = record("Mail", "ana", "an-older-value");
    let mut store = Fake::new(&three(), std::slice::from_ref(&mine));
    assert_eq!(
        move_items(&mut store, OTHER, LOGIN).map(|m| m.copied),
        Ok(3)
    );
    let login = store.records(LOGIN);
    assert!(login.contains(&mine));
    assert!(login.contains(&three()[0]));
    assert_eq!(login.len(), 4);
}

#[test]
fn identical_items_of_the_old_keyring_each_get_a_copy() {
    let twice = vec![record("Mail", "ana", "same"), record("Mail", "ana", "same")];
    let mut store = Fake::new(&twice, &[]);
    assert_eq!(
        move_items(&mut store, OTHER, LOGIN).map(|m| m.copied),
        Ok(2)
    );
    assert_eq!(store.records(LOGIN), twice);
}

/// The store as it was before a move: its default and every item.
fn snapshot(store: &Fake) -> (String, Vec<(String, String, Record)>) {
    (store.default.clone(), store.items.clone())
}

#[test]
fn a_failure_at_any_step_leaves_the_default_and_both_keyrings_as_they_were() {
    type Break = fn(&mut Fake);
    let cases: [(Break, FailedStep); 8] = [
        (|s| s.default = LOGIN.into(), FailedStep::Changed),
        (|s| s.fail.push("default"), FailedStep::Read),
        (|s| s.fail.push("items"), FailedStep::Read),
        (|s| s.fail.push("read"), FailedStep::Read),
        (|s| s.fail.push("attributes"), FailedStep::Read),
        // the third item cannot be saved: the two copies made are taken back
        (|s| s.fail_create_at = Some(2), FailedStep::Copy),
        (|s| s.corrupt = true, FailedStep::Verify),
        (|s| s.fail.push("set_default"), FailedStep::Default),
    ];
    for (breaking, step) in cases {
        let mut store = Fake::new(&three(), &[record("Wi-Fi", "home", "s3cret-wifi")]);
        breaking(&mut store);
        let before = snapshot(&store);
        let error = move_items(&mut store, OTHER, LOGIN).unwrap_err();
        assert_eq!(error.step, step);
        assert_eq!(error.left, 0, "{step:?}");
        assert_eq!(snapshot(&store), before, "{step:?}");
    }
}

#[test]
fn a_default_the_store_did_not_take_is_a_failure() {
    let mut store = Fake::new(&three(), &[]);
    store.ignore_default = true;
    let before = snapshot(&store);
    let error = move_items(&mut store, OTHER, LOGIN).unwrap_err();
    assert_eq!(error.step, FailedStep::Default);
    assert_eq!(snapshot(&store), before);
}

#[test]
fn a_move_stopped_halfway_is_repeated_without_a_second_copy_of_any_item() {
    // the first run saves two items and stops; the copies it made could not be taken back
    let mut store = Fake::new(&three(), &[]);
    store.fail_create_at = Some(2);
    store.fail.push("delete");
    let error = move_items(&mut store, OTHER, LOGIN).unwrap_err();
    assert_eq!((error.step, error.left), (FailedStep::Copy, 2));
    assert_eq!(store.default, OTHER);
    assert_eq!(store.records(LOGIN).len(), 2);

    store.fail.clear();
    store.fail_create_at = None;
    store.created = 0;
    assert_eq!(
        move_items(&mut store, OTHER, LOGIN),
        Ok(Moved {
            copied: 1,
            already: 2
        })
    );
    assert_eq!(store.records(LOGIN), three());
    assert_eq!(store.default, LOGIN);
}

#[test]
fn no_secret_is_in_what_an_error_or_a_record_prints() {
    let mut store = Fake::new(&three(), &[]);
    store.corrupt = true;
    let error = move_items(&mut store, OTHER, LOGIN).unwrap_err();
    let printed = format!("{error} {error:?} {:?} {:?}", three(), Notice::of(&error));
    for word in ["s3cret", "ana", "ben", "Mail"] {
        assert!(!printed.contains(word), "{word} in {printed}");
    }
}
