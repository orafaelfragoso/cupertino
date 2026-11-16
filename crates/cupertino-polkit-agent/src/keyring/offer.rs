//! The offer to use the login keyring for everything, as a pure model. Login unlocks only the keyring named `login`;
//! when an application made another keyring the default one, that keyring stays locked after login and asks for its
//! own password in every session. Nothing is changed silently: once, the person is asked whether the default
//! keyring's items should be copied into the login keyring and the login keyring made the default.
//!
//! - [`Flow`]: what happens after each answer. A decline by the question's own button is remembered for good, a
//!   finished move too; a question that went away without a button (Escape, or the session took it away), a cancelled
//!   password and a failure are not, and the question may come back at the next login.
//! - [`move_items`]: the move over a [`Store`]: every item copied and read back before the default changes, and
//!   everything it made taken back when a step fails, unless the default may already be the login keyring: then the
//!   copies stay. Items the login keyring already holds are not copied again, so a move that was interrupted can be
//!   repeated.
//!
//! A secret's bytes are compared and carried, never printed: [`Record`]'s `Debug` and every error leave them out.

use std::collections::BTreeMap;
use std::fmt;

use zeroize::Zeroizing;

/// The login keyring's collection, the one login unlocks.
pub const LOGIN: &str = "/org/freedesktop/secrets/collection/login";

/// Characters of a keyring's label the question shows.
const MAX_LABEL: usize = 12;
pub const MOVE: &str = "Move Items";
pub const KEEP: &str = "Don\u{2019}t Move";
pub const TRY_AGAIN: &str = "Try Again";
pub const NOT_NOW: &str = "Not Now";

/// What an earlier login left: the person's answer, kept for good.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Remembered {
    Nothing,
    /// The person declined: the question never comes back.
    Declined,
    /// The items were moved.
    Moved,
}

impl Remembered {
    /// What the file of the answer says; anything unknown counts as nothing.
    pub fn parse(text: &str) -> Remembered {
        match text.trim() {
            "declined" => Remembered::Declined,
            "moved" => Remembered::Moved,
            _ => Remembered::Nothing,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Remembered::Nothing => "",
            Remembered::Declined => "declined",
            Remembered::Moved => "moved",
        }
    }
}

/// What the Secret Service says about its keyrings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observed {
    /// The default collection's path and label; `None` when there is none.
    pub default: Option<(String, String)>,
    /// Whether the login collection is locked; `None` when it does not exist.
    pub login_locked: Option<bool>,
}

/// The question to ask: the default collection that is not the login one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub collection: String,
    pub label: String,
}

impl Offer {
    /// The question when `observed` calls for one: a default collection other than the login one, and a login
    /// collection that exists and is unlocked. `None` for `observed` means there is no Secret Service.
    pub fn of(observed: Option<&Observed>) -> Option<Offer> {
        let observed = observed?;
        let (collection, label) = observed.default.as_ref()?;
        if collection == LOGIN || observed.login_locked != Some(false) {
            return None;
        }
        Some(Offer {
            collection: collection.clone(),
            label: label.clone(),
        })
    }

    /// The keyring's name as the text says it, at the start of a sentence; a long label is cut so the alert shows
    /// the whole text.
    fn name(&self) -> String {
        let label: String = self.label.chars().filter(|c| !c.is_control()).collect();
        let label = label.trim();
        if label.is_empty() {
            return "The default keyring".to_owned();
        }
        let mut shown: String = label.chars().take(MAX_LABEL).collect();
        if label.chars().count() > MAX_LABEL {
            shown.push('\u{2026}');
        }
        format!("The keyring \u{201c}{shown}\u{201d}")
    }

    pub fn heading(&self) -> String {
        "Move saved passwords to your login keyring?".to_owned()
    }

    pub fn description(&self) -> String {
        format!(
            "{name} stays locked when you log in and asks for its own password. Its items can be copied to the \
             login keyring; the keyring itself is kept. {KEEP} is remembered.",
            name = self.name()
        )
    }
}

/// How the person answered a question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Yes,
    /// The question's own "no" button.
    No,
    /// The question went away without a button: Escape, or the session took it away (it was locked or left, or could
    /// not show it). Not now, and nothing to remember.
    Dismissed,
}

/// How unlocking the old keyring went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unlocked {
    Yes,
    /// The person cancelled the password prompt.
    Cancelled,
    Failed,
}

/// How the flow ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing was asked: an earlier answer, a question already asked in this login, or nothing to offer.
    Quiet,
    Declined,
    /// The question went unanswered, or the password prompt was cancelled.
    Left,
    Moved(Moved),
    /// The move failed and the person did not try again.
    Failed,
}

/// What the host does next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Read the Secret Service's keyrings and report [`Flow::observed`].
    Observe,
    /// Note that this login asked, show the question and report [`Flow::answered`].
    Ask(Offer),
    /// Unlock the old keyring through its own prompt and report [`Flow::unlocked`].
    Unlock(String),
    /// Run [`move_items`] from this collection to the login one and report [`Flow::moved`].
    Move(String),
    /// Keep the answer for good.
    Remember(Remembered),
    /// Say that nothing was moved, offer to try again and report [`Flow::answered`].
    Tell(Notice),
    End(Outcome),
}

/// What the person is told when the move did not happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub heading: String,
    pub description: String,
}

impl Notice {
    /// The old keyring did not unlock.
    pub fn unlock() -> Notice {
        Notice {
            heading: "The passwords were not moved".to_owned(),
            description: "The keyring could not be unlocked. Nothing was changed.".to_owned(),
        }
    }

    /// The move failed at a step.
    pub fn of(error: &MoveError) -> Notice {
        if error.step == FailedStep::Unconfirmed {
            // the copies were kept: the default may be the login keyring, which must then hold the items
            return Notice {
                heading: "The move could not be confirmed".to_owned(),
                description: format!(
                    "{} The items are in both keyrings; nothing was removed.",
                    error.step
                ),
            };
        }
        let unchanged = if error.left == 0 {
            "Nothing was changed: both keyrings are as they were."
        } else {
            "The default keyring is unchanged, but some copies could not be removed from the login keyring."
        };
        Notice {
            heading: "The passwords were not moved".to_owned(),
            description: format!("{} {unchanged}", error.step),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    Observing,
    Asking(Offer),
    Unlocking(Offer),
    Moving(Offer),
    /// The failure is on screen.
    Telling(Offer),
    Ended,
}

/// The offer from the session's start to its end. Each call returns the steps the host carries out, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flow {
    state: State,
}

impl Flow {
    /// Begin with what was `remembered` and whether this login already `asked`.
    pub fn start(remembered: Remembered, asked: bool) -> (Flow, Vec<Step>) {
        if remembered != Remembered::Nothing || asked {
            return (
                Flow {
                    state: State::Ended,
                },
                vec![Step::End(Outcome::Quiet)],
            );
        }
        (
            Flow {
                state: State::Observing,
            },
            vec![Step::Observe],
        )
    }

    pub fn ended(&self) -> bool {
        self.state == State::Ended
    }

    fn end(&mut self, outcome: Outcome) -> Vec<Step> {
        self.state = State::Ended;
        vec![Step::End(outcome)]
    }

    /// The keyrings were read; `None` when there is no Secret Service.
    pub fn observed(&mut self, observed: Option<&Observed>) -> Vec<Step> {
        if self.state != State::Observing {
            return Vec::new();
        }
        match Offer::of(observed) {
            Some(offer) => {
                self.state = State::Asking(offer.clone());
                vec![Step::Ask(offer)]
            }
            None => self.end(Outcome::Quiet),
        }
    }

    /// The person answered the question on screen: the offer, or the failure's "try again".
    pub fn answered(&mut self, answer: Answer) -> Vec<Step> {
        match (std::mem::replace(&mut self.state, State::Ended), answer) {
            (State::Asking(offer) | State::Telling(offer), Answer::Yes) => {
                let collection = offer.collection.clone();
                self.state = State::Unlocking(offer);
                vec![Step::Unlock(collection)]
            }
            (State::Asking(_), Answer::No) => {
                vec![
                    Step::Remember(Remembered::Declined),
                    Step::End(Outcome::Declined),
                ]
            }
            (State::Asking(_), Answer::Dismissed) => vec![Step::End(Outcome::Left)],
            (State::Telling(_), Answer::No | Answer::Dismissed) => {
                vec![Step::End(Outcome::Failed)]
            }
            (state, _) => {
                self.state = state;
                Vec::new()
            }
        }
    }

    /// The old keyring's unlock ended.
    pub fn unlocked(&mut self, unlocked: Unlocked) -> Vec<Step> {
        let State::Unlocking(offer) = self.state.clone() else {
            return Vec::new();
        };
        self.state = State::Ended;
        match unlocked {
            Unlocked::Yes => {
                let collection = offer.collection.clone();
                self.state = State::Moving(offer);
                vec![Step::Move(collection)]
            }
            // not a decline: the question may come back at the next login
            Unlocked::Cancelled => vec![Step::End(Outcome::Left)],
            Unlocked::Failed => {
                self.state = State::Telling(offer);
                vec![Step::Tell(Notice::unlock())]
            }
        }
    }

    /// The move ended.
    pub fn moved(&mut self, result: Result<Moved, MoveError>) -> Vec<Step> {
        let State::Moving(offer) = self.state.clone() else {
            return Vec::new();
        };
        self.state = State::Ended;
        match result {
            Ok(moved) => vec![
                Step::Remember(Remembered::Moved),
                Step::End(Outcome::Moved(moved)),
            ],
            Err(error) => {
                self.state = State::Telling(offer);
                vec![Step::Tell(Notice::of(&error))]
            }
        }
    }
}

/// One item of a keyring: all that a copy must equal.
#[derive(Clone, PartialEq, Eq)]
pub struct Record {
    pub label: String,
    pub attributes: BTreeMap<String, String>,
    pub secret: Zeroizing<Vec<u8>>,
    pub content_type: String,
}

impl fmt::Debug for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // neither the secret nor what would name the account it belongs to
        f.debug_struct("Record")
            .field("attributes", &self.attributes.len())
            .field("content_type", &self.content_type)
            .finish_non_exhaustive()
    }
}

/// A call to the store that failed: what was asked and the store's name for the error, never a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreError {
    pub call: &'static str,
    pub name: String,
}

impl StoreError {
    pub fn new(call: &'static str, name: impl Into<String>) -> StoreError {
        StoreError {
            call,
            name: name.into(),
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.call, self.name)
    }
}

impl std::error::Error for StoreError {}

/// The keyrings the move works on. Items and collections are named by their paths.
pub trait Store {
    /// The default collection.
    fn default_collection(&mut self) -> Result<String, StoreError>;
    /// Make `collection` the default one.
    fn set_default(&mut self, collection: &str) -> Result<(), StoreError>;
    fn items(&mut self, collection: &str) -> Result<Vec<String>, StoreError>;
    /// An item's attributes alone, without its secret.
    fn attributes(&mut self, item: &str) -> Result<BTreeMap<String, String>, StoreError>;
    fn read(&mut self, item: &str) -> Result<Record, StoreError>;
    /// A new item in `collection`, beside any item with the same attributes; its path.
    fn create(&mut self, collection: &str, record: &Record) -> Result<String, StoreError>;
    fn delete(&mut self, item: &str) -> Result<(), StoreError>;
}

/// What a finished move did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Moved {
    /// Items copied by this run.
    pub copied: usize,
    /// Items the target already held, identical.
    pub already: usize,
}

/// The step of the move that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailedStep {
    /// The default collection is no longer the one that was offered.
    Changed,
    Read,
    Copy,
    /// A copy read back differs from its item.
    Verify,
    /// The login collection did not become the default: the default is still the offered one.
    Default,
    /// The default was changed, could not be read back and could not be put back: which keyring is the default now
    /// is not known.
    Unconfirmed,
}

impl fmt::Display for FailedStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FailedStep::Changed => "The default keyring changed in the meantime.",
            FailedStep::Read => "An item of the keyring could not be read.",
            FailedStep::Copy => "An item could not be saved in the login keyring.",
            FailedStep::Verify => "A copy did not match its item.",
            FailedStep::Default => "The login keyring could not be made the default.",
            FailedStep::Unconfirmed => "The login keyring may now be the default.",
        })
    }
}

/// Why the move did not finish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveError {
    pub step: FailedStep,
    /// The store's error, when a call failed.
    pub cause: Option<StoreError>,
    /// Copies this run made and could not take back. Not the copies kept on purpose after
    /// [`FailedStep::Unconfirmed`].
    pub left: usize,
}

impl fmt::Display for MoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.step)?;
        if let Some(cause) = &self.cause {
            write!(f, " ({cause})")?;
        }
        if self.left > 0 {
            write!(f, ", {} copies left behind", self.left)?;
        }
        Ok(())
    }
}

impl std::error::Error for MoveError {}

/// Copy every item of `from` into `to`, read every copy back, and only then make `to` the default collection. `from`
/// keeps its items. An item `to` already holds identical (label, attributes, secret and content type) is not copied
/// again; an item of `to` that differs is never replaced. When a step fails the copies this run made are deleted and
/// the default stays as it was. The one exception is a default that was changed and can neither be read back nor put
/// back ([`FailedStep::Unconfirmed`]): it may be `to`, so the copies stay, and `to` is never left the default without
/// the items. A default that is already `to` (such a run, tried again) is a move to finish: what is missing is copied
/// and the default is left alone.
pub fn move_items(store: &mut impl Store, from: &str, to: &str) -> Result<Moved, MoveError> {
    let mut created = Vec::new();
    let result = copy_and_switch(store, from, to, &mut created);
    result.map_err(|(step, cause)| {
        let left = if step == FailedStep::Unconfirmed {
            0
        } else {
            created
                .iter()
                .filter(|item| store.delete(item).is_err())
                .count()
        };
        MoveError { step, cause, left }
    })
}

type Failure = (FailedStep, Option<StoreError>);

fn copy_and_switch(
    store: &mut impl Store,
    from: &str,
    to: &str,
    created: &mut Vec<String>,
) -> Result<Moved, Failure> {
    let read = |e| (FailedStep::Read, Some(e));
    let default = store.default_collection().map_err(read)?;
    if default != from && default != to {
        return Err((FailedStep::Changed, None));
    }
    let mut sources = Vec::new();
    for item in store.items(from).map_err(read)? {
        sources.push(store.read(&item).map_err(read)?);
    }
    // what the target holds, by attributes: only an item with an item's own attributes has its secret read
    let mut held = Vec::new();
    for item in store.items(to).map_err(read)? {
        held.push((store.attributes(&item).map_err(read)?, item));
    }
    // each item's copy: one made now, or an identical item that no other item of this run counts as its own
    let mut copies: Vec<String> = Vec::new();
    let mut already = 0;
    for record in &sources {
        let mut found = None;
        for (attributes, candidate) in &held {
            if *attributes == record.attributes
                && !copies.contains(candidate)
                && store.read(candidate).map_err(read)? == *record
            {
                found = Some(candidate.clone());
                break;
            }
        }
        match found {
            Some(item) => {
                already += 1;
                copies.push(item);
            }
            None => {
                let item = store
                    .create(to, record)
                    .map_err(|e| (FailedStep::Copy, Some(e)))?;
                created.push(item.clone());
                copies.push(item);
            }
        }
    }
    for (record, copy) in sources.iter().zip(&copies) {
        let stored = store
            .read(copy)
            .map_err(|e| (FailedStep::Verify, Some(e)))?;
        if stored != *record {
            return Err((FailedStep::Verify, None));
        }
    }
    let moved = Moved {
        copied: created.len(),
        already,
    };
    if default == to {
        // an earlier run already made it the default
        created.clear();
        return Ok(moved);
    }
    if let Err(error) = store.set_default(to) {
        return Err((FailedStep::Default, Some(error)));
    }
    match store.default_collection() {
        Ok(default) if default == to => {
            // the move stands: nothing of it is taken back
            created.clear();
            Ok(moved)
        }
        // the store kept the old default: nothing changed, and the copies go
        Ok(default) if default == from => Err((FailedStep::Default, None)),
        // another keyring was made the default meanwhile: it is left alone, and the copies go
        Ok(_) => Err((FailedStep::Changed, None)),
        // the default was set and is not known: only with the old one back for certain do the copies go
        Err(unread) => match store.set_default(from) {
            Ok(()) => Err((FailedStep::Default, Some(unread))),
            Err(_) => Err((FailedStep::Unconfirmed, Some(unread))),
        },
    }
}

#[cfg(test)]
mod tests;
