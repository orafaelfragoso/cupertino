//! The offer carried out: [`run`] takes the [`Flow`]'s steps one by one, a future on Kit's loop. Its questions are
//! asked as the keyring daemon asks its own: [`ask`] is a caller of the session's prompter inside the process, so they
//! show in the same alert, wait their turn behind another prompt and are never on screen while the session is locked
//! or away. The person's answer is kept in a file of the account; that a login asked is kept in the runtime directory,
//! so a program that starts again asks no second time. The files are read and written on Kit's I/O pool ([`Io`]).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use zbus::Connection;

use crate::keyring::exchange::Exchange;
use crate::keyring::model::{Caller, Effect, Kind, Reply, Value, ESCAPE_DISMISSES};
use crate::keyring::offer::{
    move_items, Answer, FailedStep, Flow, MoveError, Outcome, Remembered, Step, KEEP, LOGIN, MOVE,
    NOT_NOW, TRY_AGAIN,
};
use crate::keyring::prompter::{self, Call, Request};
use crate::keyring::secrets::{self, Secrets};

/// The bus name the offer's questions come from: none on the bus, so no bus caller can be it.
pub const INSIDE: &str = "";
/// The object the offer's questions are answered on.
pub const CALLBACK: &str = "/org/cupertino/keyring/Offer";
/// How long to wait for a Secret Service that is not on the bus yet.
pub const SERVICE_WAIT: Duration = Duration::from_secs(20);

/// The offer's own caller.
pub fn caller() -> Caller {
    Caller {
        name: INSIDE.to_owned(),
        path: CALLBACK.to_owned(),
    }
}

/// Work for the I/O pool, done once.
type Work = Box<dyn FnOnce() + Send>;

/// Work the offer hands to the program to run on Kit's I/O pool, once. `Clone` so a message can carry it.
#[derive(Clone)]
pub struct Io(Arc<Mutex<Option<Work>>>);

impl Io {
    fn new(work: impl FnOnce() + Send + 'static) -> Io {
        Io(Arc::new(Mutex::new(Some(Box::new(work)))))
    }

    /// Do the work, the first time only. Blocks: run it on the I/O pool.
    pub fn run(&self) {
        let work = self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(work) = work {
            work();
        }
    }
}

impl std::fmt::Debug for Io {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Io(..)")
    }
}

/// What the offer needs of the program: its I/O pool, and the prompter's model, whose callbacks to [`caller`] come
/// back on `callbacks`.
pub struct Host {
    pub io: Box<dyn Fn(Io) + Send + Sync>,
    pub requests: Box<dyn Fn(Request) + Send + Sync>,
    pub callbacks: async_channel::Receiver<Effect>,
}

impl Host {
    /// `work`'s result, from the I/O pool; `None` when the program dropped it.
    async fn on_io<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let (sender, result) = async_channel::bounded(1);
        (self.io)(Io::new(move || {
            let _ = sender.try_send(work());
        }));
        result.recv().await.ok()
    }

    /// Ask the prompter `request`, made with the call it answers; how it answered.
    async fn call(&self, request: impl FnOnce(Call) -> Request) -> bool {
        let (sender, answer) = async_channel::bounded(1);
        (self.requests)(request(Call::Inside(sender)));
        matches!(answer.recv().await, Ok(Ok(())))
    }
}

/// A yes-or-no question for the prompter's alert.
#[derive(Clone, Copy, Debug)]
pub struct Question<'a> {
    pub heading: &'a str,
    pub description: &'a str,
    pub yes: &'a str,
    pub no: &'a str,
}

/// Where the offer keeps what it knows.
#[derive(Clone, Debug)]
pub struct Setup {
    /// The file of the person's answer, kept for good.
    pub answer: PathBuf,
    /// The file that names the login that last asked.
    pub asked: PathBuf,
    /// This login, as the file names it.
    pub login: String,
    /// How long to wait for a Secret Service that is not on the bus yet.
    pub wait: Duration,
}

impl Setup {
    /// The files of this account's session: the answer under the state directory, the login that asked under the
    /// runtime directory. `None` without a runtime directory, where nothing can note that a login asked.
    pub fn of_session(login: String) -> Option<Setup> {
        let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        let state = set("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| set("HOME").map(|home| PathBuf::from(home).join(".local/state")))?;
        let runtime = PathBuf::from(set("XDG_RUNTIME_DIR")?);
        Some(Setup {
            answer: state.join("cupertino/keyring-offer"),
            asked: runtime.join("cupertino-keyring-offer"),
            login,
            wait: SERVICE_WAIT,
        })
    }

    /// What an earlier login left, and whether this login already asked. Reads files: run it on the I/O pool.
    #[expect(
        clippy::disallowed_methods,
        reason = "runs on the I/O pool (kit::task::io)"
    )]
    fn read(&self) -> (Remembered, bool) {
        let remembered = std::fs::read_to_string(&self.answer)
            .map(|text| Remembered::parse(&text))
            .unwrap_or(Remembered::Nothing);
        let asked =
            std::fs::read_to_string(&self.asked).is_ok_and(|text| text.trim_end() == self.login);
        (remembered, asked)
    }

    /// Writes a file: run it on the I/O pool.
    #[expect(
        clippy::disallowed_methods,
        reason = "runs on the I/O pool (kit::task::io)"
    )]
    fn remember(&self, answer: Remembered) -> std::io::Result<()> {
        if let Some(dir) = self.answer.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.answer, format!("{}\n", answer.as_str()))
    }

    /// Writes a file: run it on the I/O pool.
    #[expect(
        clippy::disallowed_methods,
        reason = "runs on the I/O pool (kit::task::io)"
    )]
    fn note_asked(&self) -> std::io::Result<()> {
        std::fs::write(&self.asked, format!("{}\n", self.login))
    }
}

const LOG: &str = "cupertino-prompts";

/// Run the offer to its end on the session bus `bus`. Waits while the person decides.
pub async fn run(bus: Connection, setup: Setup, host: Host) -> Outcome {
    let read = setup.clone();
    let (remembered, asked) = host
        .on_io(move || read.read())
        .await
        .unwrap_or((Remembered::Nothing, true));
    let (mut flow, steps) = Flow::start(remembered, asked);
    let mut steps = VecDeque::from(steps);
    while let Some(step) = steps.pop_front() {
        let next = match step {
            Step::Observe => {
                let observed = if secrets::present(&bus, setup.wait).await {
                    secrets::observe(&bus)
                        .await
                        .map_err(|e| eprintln!("{LOG}: {e}"))
                        .ok()
                } else {
                    None
                };
                flow.observed(observed.as_ref())
            }
            Step::Ask(offer) => {
                // a login that cannot note it asked does not ask, or it would ask again each time this starts
                let note = setup.clone();
                let noted = host.on_io(move || note.note_asked()).await;
                let answer = match noted {
                    Some(Ok(())) => {
                        let (heading, description) = (offer.heading(), offer.description());
                        let question = Question {
                            heading: &heading,
                            description: &description,
                            yes: MOVE,
                            no: KEEP,
                        };
                        ask(&host, &question).await
                    }
                    Some(Err(error)) => {
                        eprintln!("{LOG}: noting that this login asked: {error}");
                        Answer::Dismissed
                    }
                    None => Answer::Dismissed,
                };
                flow.answered(answer)
            }
            Step::Unlock(collection) => flow.unlocked(secrets::unlock(&bus, &collection).await),
            Step::Move(collection) => {
                let result = match Secrets::open(&bus).await {
                    Ok(mut store) => {
                        let moved = move_items(&mut store, &collection, LOGIN).await;
                        store.close().await;
                        moved
                    }
                    Err(cause) => Err(MoveError {
                        step: FailedStep::Read,
                        cause: Some(cause),
                        left: 0,
                    }),
                };
                if let Err(error) = &result {
                    eprintln!("{LOG}: the move to the login keyring did not finish: {error}");
                }
                flow.moved(result)
            }
            Step::Remember(answer) => {
                let keep = setup.clone();
                if let Some(Err(error)) = host.on_io(move || keep.remember(answer)).await {
                    eprintln!("{LOG}: keeping the answer about the default keyring: {error}");
                }
                Vec::new()
            }
            Step::Tell(notice) => {
                let question = Question {
                    heading: &notice.heading,
                    description: &notice.description,
                    yes: TRY_AGAIN,
                    no: NOT_NOW,
                };
                flow.answered(ask(&host, &question).await)
            }
            Step::End(outcome) => return outcome,
        };
        steps.extend(next);
    }
    Outcome::Quiet
}

/// Ask `question` in the prompter's alert and wait for the answer. Only the alert's buttons answer: Escape, and an
/// answer the prompter gave itself because the session went away or the alert could not be shown, is
/// [`Answer::Dismissed`], and so is a question the prompter refused.
pub async fn ask(host: &Host, question: &Question<'_>) -> Answer {
    let Ok(mut exchange) = Exchange::new() else {
        eprintln!("{LOG}: asking about the default keyring: no random bytes for the exchange");
        return Answer::Dismissed;
    };
    // callbacks of an earlier question that ended are of no use now
    while host.callbacks.try_recv().is_ok() {}
    if !host
        .call(|call| Request::Begin {
            caller: caller(),
            call,
        })
        .await
    {
        return Answer::Dismissed;
    }
    let mut asked = false;
    while let Ok(effect) = host.callbacks.recv().await {
        match effect {
            Effect::Ready {
                reply,
                exchange: theirs,
                ..
            } if !asked => {
                if reply != Reply::Ready || exchange.receive(&theirs).is_err() {
                    break;
                }
                let properties = vec![
                    ("title".to_owned(), Value::Str(question.heading.to_owned())),
                    (
                        "message".to_owned(),
                        Value::Str(question.heading.to_owned()),
                    ),
                    (
                        "description".to_owned(),
                        Value::Str(question.description.to_owned()),
                    ),
                    (
                        "continue-label".to_owned(),
                        Value::Str(question.yes.to_owned()),
                    ),
                    (
                        "cancel-label".to_owned(),
                        Value::Str(question.no.to_owned()),
                    ),
                    // only the button is a no: Escape ends the prompt, which is no answer
                    (ESCAPE_DISMISSES.to_owned(), Value::Bool(true)),
                ];
                asked = true;
                let mine = exchange.begin();
                let performed = host
                    .call(|call| Request::Perform {
                        caller: caller(),
                        kind: Kind::Confirm,
                        properties,
                        exchange: prompter::Exchange(mine),
                        call,
                    })
                    .await;
                if !performed {
                    break;
                }
            }
            Effect::Ready { reply, .. } => {
                let answer = if reply == Reply::Yes {
                    Answer::Yes
                } else {
                    Answer::No
                };
                stop(host).await;
                return answer;
            }
            // the prompter ended the prompt itself: a no it sent just before was not the person's
            Effect::Done { .. } => return Answer::Dismissed,
        }
    }
    stop(host).await;
    Answer::Dismissed
}

/// The offer stops prompting.
async fn stop(host: &Host) {
    host.call(|call| Request::Stop {
        caller: caller(),
        call,
    })
    .await;
}
