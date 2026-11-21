//! The keyring's prompter through the crate's interface: the secret exchange both ways
//! (`cupertino_prompts::keyring::exchange`), the callers and their questions (`keyring::model`), and the offer about a
//! default keyring that login does not unlock (`keyring::offer`): its flow and the move over a stand-in store. The
//! prompter on the bus is tested as a program (`crates/cupertino-session-tests/tests/prompts.rs`).
#![expect(
    clippy::disallowed_methods,
    reason = "a test awaits the move over its stand-in store on its own thread"
)]

use std::collections::BTreeMap;
use std::future::{self, Future};

use cupertino_prompts::keyring::exchange::{base64_decode, base64_encode, Exchange, ExchangeError};
use cupertino_prompts::keyring::model::{
    label_or, Caller, Effect, Field, Kind, Phase, PromptError, Prompter, Reply, Value, MAX_WAITING,
};
use cupertino_prompts::keyring::offer::{
    move_items, Answer, FailedStep, Flow, Moved, Notice, Observed, Offer, Outcome, Record,
    Remembered, Step, Store, StoreError, Unlocked, LOGIN,
};
use futures_lite::future::block_on;
use zeroize::Zeroizing;

fn caller(n: u32) -> Caller {
    Caller {
        name: format!(":1.{n}"),
        path: "/org/gnome/keyring/Prompt/p1".to_owned(),
    }
}

/// The prompter's exchange message of a `Ready` effect, with its caller and reply.
fn ready(effect: &Effect) -> (&Caller, Reply, &str) {
    match effect {
        Effect::Ready {
            caller,
            reply,
            exchange,
            ..
        } => (caller, *reply, exchange),
        Effect::Done { .. } => panic!("not ready: {effect:?}"),
    }
}

#[test]
fn the_exchange_carries_a_secret_both_ways_and_base64_round_trips() {
    let mut caller = Exchange::new().unwrap();
    let mut prompter = Exchange::with_private(&[7; 32]);
    assert!(!prompter.shared());
    assert_eq!(
        prompter.send(Some(b"x")).unwrap_err(),
        ExchangeError::NoPeer
    );
    // the caller's first message carries no secret
    assert_eq!(prompter.receive(&caller.begin()).unwrap(), None);
    assert!(prompter.shared());
    let message = prompter.send(Some(b"hunter2")).unwrap();
    assert!(!message.contains("hunter2"));
    assert!(message.starts_with("[sx-aes-1]"));
    let secret = caller.receive(&message).unwrap().unwrap();
    assert_eq!(secret.as_slice(), b"hunter2");
    // and back
    let back = caller.send(Some(b"\xff\x00two")).unwrap();
    assert_eq!(
        prompter.receive(&back).unwrap().unwrap().as_slice(),
        b"\xff\x00two"
    );
    assert!(prompter.receive("[sx-aes-1]\nnothing=here\n").is_err());
    for bytes in [&b""[..], b"a", b"ab", b"abc", b"\x00\xfe\xff\x10"] {
        assert_eq!(
            base64_decode(&base64_encode(bytes)).unwrap().as_slice(),
            bytes
        );
    }
    assert!(base64_decode("@@@@").is_err());
    assert!(!format!("{prompter:?}").contains("hunter2"));
}

#[test]
fn a_password_question_answers_through_the_exchange() {
    let mut prompter = Prompter::new();
    let mut side = Exchange::new().unwrap();
    let effects = prompter.begin(caller(1)).unwrap();
    let (who, reply, message) = ready(&effects[0]);
    assert_eq!((who, reply), (&caller(1), Reply::Ready));
    assert_eq!(side.receive(message).unwrap(), None);
    assert!(
        prompter.shown().is_none(),
        "no alert before the first question"
    );

    let changed = vec![
        (
            "message".to_owned(),
            Value::Str("Unlock the keyring".to_owned()),
        ),
        (
            "continue-label".to_owned(),
            Value::Str("_Unlock".to_owned()),
        ),
        ("unknown".to_owned(), Value::Int(3)),
    ];
    let effects = prompter
        .perform(&caller(1), Kind::Password, changed, &side.begin())
        .unwrap();
    assert!(effects.is_empty());
    let prompt = prompter.shown().unwrap();
    assert_eq!((prompt.phase, prompt.asked), (Phase::Asking, 1));
    assert_eq!(prompt.properties.heading(), "Unlock the keyring");
    assert_eq!(prompt.properties.continue_title(), "Unlock");
    assert_eq!(prompt.properties.cancel_title(), "Cancel");
    // another question while this one is open is refused
    assert_eq!(
        prompter
            .perform(&caller(1), Kind::Password, [], &side.begin())
            .unwrap_err(),
        PromptError::InProgress
    );
    prompter.set_field(Field::Password, "hunter2");
    // a control character is not taken
    prompter.set_field(Field::Password, "a\u{7}");
    assert!(!prompter.prompt().unwrap().field_empty(Field::Password));
    let effects = prompter.answer();
    let (_, reply, message) = ready(&effects[0]);
    assert_eq!(reply, Reply::Yes);
    assert_eq!(
        side.receive(message).unwrap().unwrap().as_slice(),
        b"hunter2"
    );
    assert!(!format!("{effects:?}").contains(message));
    match &effects[0] {
        Effect::Ready { properties, .. } => {
            assert!(properties.contains(&("password-strength", Value::Int(1))));
        }
        Effect::Done { .. } => unreachable!(),
    }
    // busy until the caller asks again or stops
    assert_eq!(prompter.shown().unwrap().phase, Phase::Checking);
    assert!(prompter.stop(&caller(1)).is_empty());
    assert!(prompter.prompt().is_none());
}

#[test]
fn a_new_password_must_match_its_confirmation() {
    let mut prompter = Prompter::new();
    let side = Exchange::new().unwrap();
    prompter.begin(caller(1)).unwrap();
    let new = [("password-new".to_owned(), Value::Bool(true))];
    prompter
        .perform(&caller(1), Kind::Password, new, &side.begin())
        .unwrap();
    assert!(prompter.prompt().unwrap().confirms());
    prompter.set_field(Field::Password, "one");
    prompter.set_field(Field::Confirm, "two");
    assert!(prompter.answer().is_empty());
    let prompt = prompter.prompt().unwrap();
    assert!(prompt.mismatch);
    assert!(prompt.field_empty(Field::Confirm));
    prompter.set_field(Field::Confirm, "one");
    assert!(!prompter.prompt().unwrap().mismatch);
    assert_eq!(ready(&prompter.answer()[0]).1, Reply::Yes);
}

#[test]
fn a_confirmation_carries_no_secret_and_cancel_says_no() {
    let mut prompter = Prompter::new();
    let mut side = Exchange::new().unwrap();
    let effects = prompter.begin(caller(1)).unwrap();
    side.receive(ready(&effects[0]).2).unwrap();
    let choice = [("choice-label".to_owned(), Value::Str("_Always".to_owned()))];
    prompter
        .perform(&caller(1), Kind::Confirm, choice, &side.begin())
        .unwrap();
    let prompt = prompter.prompt().unwrap();
    assert!(prompt.has_choice());
    assert_eq!(prompt.properties.choice_title(), "Always");
    prompter.toggle_choice();
    let effects = prompter.answer();
    let (_, reply, message) = ready(&effects[0]);
    assert_eq!(reply, Reply::Yes);
    assert_eq!(side.receive(message).unwrap(), None);
    match &effects[0] {
        Effect::Ready { properties, .. } => {
            assert!(properties.contains(&("choice-chosen", Value::Bool(true))))
        }
        Effect::Done { .. } => unreachable!(),
    }
    prompter
        .perform(&caller(1), Kind::Confirm, [], &side.begin())
        .unwrap();
    assert_eq!(ready(&prompter.cancel()[0]).1, Reply::No);
}

#[test]
fn callers_wait_in_turn_and_only_the_active_one_may_ask() {
    let mut prompter = Prompter::new();
    let side = Exchange::new().unwrap();
    prompter.begin(caller(1)).unwrap();
    assert!(prompter.begin(caller(2)).unwrap().is_empty());
    assert_eq!(
        prompter.begin(caller(2)).unwrap_err(),
        PromptError::InProgress
    );
    assert_eq!(
        prompter
            .perform(&caller(2), Kind::Password, [], &side.begin())
            .unwrap_err(),
        PromptError::InProgress
    );
    assert!(matches!(
        prompter
            .perform(&caller(1), Kind::Password, [], "garbage")
            .unwrap_err(),
        PromptError::Exchange(_)
    ));
    for n in 3..=(MAX_WAITING as u32 + 1) {
        prompter.begin(caller(n)).unwrap();
    }
    assert_eq!(prompter.waiting(), MAX_WAITING);
    assert_eq!(prompter.begin(caller(99)).unwrap_err(), PromptError::Busy);
    // the active caller leaves the bus: the next one is ready
    let effects = prompter.vanished(":1.1");
    assert_eq!(ready(&effects[0]).0, &caller(2));
    // a waiting one stops
    assert!(prompter.stop(&caller(3)).is_empty());
    assert_eq!(prompter.waiting(), MAX_WAITING - 2);
}

#[test]
fn a_session_that_goes_away_answers_no_and_ends_the_prompt() {
    let mut prompter = Prompter::new();
    let side = Exchange::new().unwrap();
    prompter.begin(caller(1)).unwrap();
    prompter.begin(caller(2)).unwrap();
    prompter
        .perform(&caller(1), Kind::Password, [], &side.begin())
        .unwrap();
    let effects = prompter.set_away(true);
    assert_eq!(ready(&effects[0]).1, Reply::No);
    assert_eq!(effects[1], Effect::Done { caller: caller(1) });
    assert_eq!(effects.len(), 2, "the waiting caller waits");
    let effects = prompter.set_away(false);
    assert_eq!(ready(&effects[0]).0, &caller(2));
    // a caller that begins while away waits for the session
    prompter.set_away(true);
    assert!(prompter.begin(caller(5)).unwrap().is_empty());
    assert!(prompter.prompt().is_none());
}

#[test]
fn escape_cancels_unless_the_caller_asked_that_it_only_dismisses() {
    let mut prompter = Prompter::new();
    let side = Exchange::new().unwrap();
    prompter.begin(caller(1)).unwrap();
    prompter
        .perform(&caller(1), Kind::Confirm, [], &side.begin())
        .unwrap();
    let effects = prompter.escape();
    assert_eq!((effects.len(), ready(&effects[0]).1), (1, Reply::No));
    let dismisses = [("escape-dismisses".to_owned(), Value::Bool(true))];
    prompter
        .perform(&caller(1), Kind::Confirm, dismisses, &side.begin())
        .unwrap();
    let effects = prompter.escape();
    assert_eq!(ready(&effects[0]).1, Reply::No);
    assert_eq!(effects[1], Effect::Done { caller: caller(1) });
    assert!(prompter.prompt().is_none());
}

#[test]
fn mnemonic_marks_leave_a_label() {
    assert_eq!(label_or("_Unlock", "Continue"), "Unlock");
    assert_eq!(label_or("Save__As", "Continue"), "Save_As");
    assert_eq!(label_or("  ", "Continue"), "Continue");
    assert_eq!(Kind::parse("password"), Some(Kind::Password));
    assert_eq!(Kind::parse("other"), None);
}

const OLD: &str = "/org/freedesktop/secrets/collection/Default";

fn observed(default: Option<&str>, login_locked: Option<bool>) -> Observed {
    Observed {
        default: default.map(|d| (d.to_owned(), "Default keyring".to_owned())),
        login_locked,
    }
}

#[test]
fn the_offer_is_made_only_for_another_default_beside_an_unlocked_login_keyring() {
    assert!(Offer::of(None).is_none());
    assert!(Offer::of(Some(&observed(None, Some(false)))).is_none());
    assert!(Offer::of(Some(&observed(Some(LOGIN), Some(false)))).is_none());
    assert!(Offer::of(Some(&observed(Some(OLD), Some(true)))).is_none());
    assert!(Offer::of(Some(&observed(Some(OLD), None))).is_none());
    let offer = Offer::of(Some(&observed(Some(OLD), Some(false)))).unwrap();
    assert_eq!(offer.collection, OLD);
    assert!(offer
        .description()
        .contains("\u{201c}Default keyr\u{2026}\u{201d}"));
    assert_eq!(Remembered::parse("declined\n"), Remembered::Declined);
    assert_eq!(
        Remembered::parse(Remembered::Moved.as_str()),
        Remembered::Moved
    );
    assert_eq!(Remembered::parse("?"), Remembered::Nothing);
}

#[test]
fn the_flow_asks_once_and_remembers_only_a_decline_or_a_move() {
    let (_, steps) = Flow::start(Remembered::Declined, false);
    assert_eq!(steps, vec![Step::End(Outcome::Quiet)]);
    let (_, steps) = Flow::start(Remembered::Nothing, true);
    assert_eq!(steps, vec![Step::End(Outcome::Quiet)]);

    let (mut flow, steps) = Flow::start(Remembered::Nothing, false);
    assert_eq!(steps, vec![Step::Observe]);
    let steps = flow.observed(Some(&observed(Some(OLD), Some(false))));
    assert!(matches!(steps.as_slice(), [Step::Ask(_)]));
    assert_eq!(
        flow.answered(Answer::No),
        vec![
            Step::Remember(Remembered::Declined),
            Step::End(Outcome::Declined)
        ]
    );
    assert!(flow.ended());

    let (mut flow, _) = Flow::start(Remembered::Nothing, false);
    flow.observed(Some(&observed(Some(OLD), Some(false))));
    assert_eq!(
        flow.answered(Answer::Dismissed),
        vec![Step::End(Outcome::Left)]
    );

    let (mut flow, _) = Flow::start(Remembered::Nothing, false);
    flow.observed(Some(&observed(Some(OLD), Some(false))));
    assert_eq!(
        flow.answered(Answer::Yes),
        vec![Step::Unlock(OLD.to_owned())]
    );
    // the unlock fails: the person is told and may try again
    assert_eq!(
        flow.unlocked(Unlocked::Failed),
        vec![Step::Tell(Notice::unlock())]
    );
    assert_eq!(
        flow.answered(Answer::Yes),
        vec![Step::Unlock(OLD.to_owned())]
    );
    assert_eq!(
        flow.unlocked(Unlocked::Yes),
        vec![Step::Move(OLD.to_owned())]
    );
    let moved = Moved {
        copied: 2,
        already: 0,
    };
    assert_eq!(
        flow.moved(Ok(moved)),
        vec![
            Step::Remember(Remembered::Moved),
            Step::End(Outcome::Moved(moved))
        ]
    );

    let (mut flow, _) = Flow::start(Remembered::Nothing, false);
    flow.observed(Some(&observed(Some(OLD), Some(false))));
    flow.answered(Answer::Yes);
    assert_eq!(
        flow.unlocked(Unlocked::Cancelled),
        vec![Step::End(Outcome::Left)]
    );
    assert!(
        flow.answered(Answer::Yes).is_empty(),
        "nothing after the end"
    );
}

/// A Secret Service in memory, with a call that fails on demand.
#[derive(Default)]
struct Keyrings {
    default: String,
    items: BTreeMap<String, (String, Record)>,
    next: u32,
    fail: Option<&'static str>,
    /// `set_default` is taken but the default reads as it was.
    stubborn: bool,
}

fn record(name: &str, secret: &[u8]) -> Record {
    Record {
        label: name.to_owned(),
        attributes: BTreeMap::from([("service".to_owned(), name.to_owned())]),
        secret: Zeroizing::new(secret.to_vec()),
        content_type: "text/plain".to_owned(),
    }
}

impl Keyrings {
    fn with(items: &[(&str, Record)]) -> Keyrings {
        let mut store = Keyrings {
            default: OLD.to_owned(),
            ..Keyrings::default()
        };
        for (collection, record) in items {
            store.add(collection, record.clone());
        }
        store
    }

    fn add(&mut self, collection: &str, record: Record) -> String {
        self.next += 1;
        let path = format!("{collection}/{}", self.next);
        self.items
            .insert(path.clone(), (collection.to_owned(), record));
        path
    }

    fn of(&self, collection: &str) -> usize {
        self.items.values().filter(|(c, _)| c == collection).count()
    }

    fn check(&self, call: &'static str) -> Result<(), StoreError> {
        match self.fail {
            Some(failing) if failing == call => {
                Err(StoreError::new(call, "org.freedesktop.DBus.Error.Failed"))
            }
            _ => Ok(()),
        }
    }
}

impl Store for Keyrings {
    fn default_collection(&mut self) -> impl Future<Output = Result<String, StoreError>> + Send {
        future::ready(self.check("default").map(|()| self.default.clone()))
    }

    fn set_default(
        &mut self,
        collection: &str,
    ) -> impl Future<Output = Result<(), StoreError>> + Send {
        let result = self.check("set_default");
        if result.is_ok() && !self.stubborn {
            self.default = collection.to_owned();
        }
        future::ready(result)
    }

    fn items(
        &mut self,
        collection: &str,
    ) -> impl Future<Output = Result<Vec<String>, StoreError>> + Send {
        let items = self
            .items
            .iter()
            .filter(|(_, (c, _))| c == collection)
            .map(|(path, _)| path.clone())
            .collect();
        future::ready(self.check("items").map(|()| items))
    }

    fn attributes(
        &mut self,
        item: &str,
    ) -> impl Future<Output = Result<BTreeMap<String, String>, StoreError>> + Send {
        let attributes = self.items.get(item).map(|(_, r)| r.attributes.clone());
        future::ready(attributes.ok_or_else(|| StoreError::new("attributes", "NoSuchObject")))
    }

    fn read(&mut self, item: &str) -> impl Future<Output = Result<Record, StoreError>> + Send {
        let record = self.items.get(item).map(|(_, r)| r.clone());
        future::ready(
            self.check("read")
                .and_then(|()| record.ok_or_else(|| StoreError::new("read", "NoSuchObject"))),
        )
    }

    fn create(
        &mut self,
        collection: &str,
        record: &Record,
    ) -> impl Future<Output = Result<String, StoreError>> + Send {
        let result = self
            .check("create")
            .map(|()| self.add(collection, record.clone()));
        future::ready(result)
    }

    fn delete(&mut self, item: &str) -> impl Future<Output = Result<(), StoreError>> + Send {
        self.items.remove(item);
        future::ready(Ok(()))
    }
}

#[test]
fn the_move_copies_every_item_then_makes_the_login_keyring_the_default() {
    let mut store = Keyrings::with(&[
        (OLD, record("mail", b"one")),
        (OLD, record("chat", b"two")),
        (LOGIN, record("chat", b"two")),
        (LOGIN, record("web", b"three")),
    ]);
    let moved = block_on(move_items(&mut store, OLD, LOGIN)).unwrap();
    assert_eq!(
        moved,
        Moved {
            copied: 1,
            already: 1
        }
    );
    assert_eq!(store.default, LOGIN);
    assert_eq!(
        (store.of(OLD), store.of(LOGIN)),
        (2, 3),
        "the old keyring keeps its items"
    );
    // run again, nothing is copied twice
    let moved = block_on(move_items(&mut store, OLD, LOGIN)).unwrap();
    assert_eq!(
        moved,
        Moved {
            copied: 0,
            already: 2
        }
    );
}

#[test]
fn a_failed_move_takes_its_copies_back_and_leaves_the_default() {
    let mut store = Keyrings::with(&[(OLD, record("mail", b"one")), (OLD, record("chat", b"two"))]);
    store.fail = Some("set_default");
    let error = block_on(move_items(&mut store, OLD, LOGIN)).unwrap_err();
    assert_eq!((error.step, error.left), (FailedStep::Default, 0));
    assert_eq!((store.default.as_str(), store.of(LOGIN)), (OLD, 0));
    assert_eq!(Notice::of(&error).heading, "The passwords were not moved");

    store.fail = None;
    store.stubborn = true;
    let error = block_on(move_items(&mut store, OLD, LOGIN)).unwrap_err();
    assert_eq!(error.step, FailedStep::Default);
    assert_eq!(store.of(LOGIN), 0);

    store.stubborn = false;
    store.fail = Some("create");
    let error = block_on(move_items(&mut store, OLD, LOGIN)).unwrap_err();
    assert_eq!(error.step, FailedStep::Copy);
    assert!(!format!("{error:?} {error}").contains("one"));

    store.fail = None;
    store.default = "/org/freedesktop/secrets/collection/other".to_owned();
    let error = block_on(move_items(&mut store, OLD, LOGIN)).unwrap_err();
    assert_eq!(error.step, FailedStep::Changed);
    assert!(!format!("{:?}", record("mail", b"hunter2")).contains("hunter2"));
}
