//! The offer carried out: [`run`] takes the [`Flow`]'s steps one by one on a thread of its own, away from the
//! prompter's loop and from login. Its questions are asked as the keyring daemon asks its own: [`ask`] is a caller of
//! the session's prompter, so they show in the same alert, wait their turn behind another prompt and are never on
//! screen while the session is locked or away. The person's answer is kept in a file of the account; that a login
//! asked is kept in the runtime directory, so a prompter that starts again asks no second time.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use crate::keyring::exchange::Exchange;
use crate::keyring::model::ESCAPE_DISMISSES;
use crate::keyring::offer::{
    move_items, Answer, FailedStep, Flow, MoveError, Outcome, Remembered, Step, KEEP, LOGIN, MOVE,
    NOT_NOW, TRY_AGAIN,
};
use crate::keyring::prompter::{CALLBACK_INTERFACE, INTERFACE, PATH};
use crate::keyring::secrets::{self, Secrets};

/// The object the prompter answers the offer's questions on.
const CALLBACK: &str = "/org/cupertino/keyring/Offer";
/// How often the Secret Service is looked for while it may still be starting.
const LOOK_EVERY: Duration = Duration::from_millis(500);

/// A yes-or-no question for the prompter's alert.
#[derive(Clone, Copy, Debug)]
pub struct Question<'a> {
    pub heading: &'a str,
    pub description: &'a str,
    pub yes: &'a str,
    pub no: &'a str,
}

/// Where the offer runs and what it keeps.
#[derive(Clone, Debug)]
pub struct Setup {
    /// The bus name of the prompter that shows the questions.
    pub prompter: String,
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
    pub fn of_session(prompter: String, login: String) -> Option<Setup> {
        let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        let state = set("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| set("HOME").map(|home| PathBuf::from(home).join(".local/state")))?;
        let runtime = PathBuf::from(set("XDG_RUNTIME_DIR")?);
        Some(Setup {
            prompter,
            answer: state.join("cupertino/keyring-offer"),
            asked: runtime.join("cupertino-keyring-offer"),
            login,
            wait: Duration::from_secs(20),
        })
    }

    fn remembered(&self) -> Remembered {
        std::fs::read_to_string(&self.answer)
            .map(|text| Remembered::parse(&text))
            .unwrap_or(Remembered::Nothing)
    }

    fn remember(&self, answer: Remembered) -> std::io::Result<()> {
        if let Some(dir) = self.answer.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.answer, format!("{}\n", answer.as_str()))
    }

    /// Whether this login already asked.
    fn asked(&self) -> bool {
        std::fs::read_to_string(&self.asked).is_ok_and(|text| text.trim_end() == self.login)
    }

    fn note_asked(&self) -> std::io::Result<()> {
        std::fs::write(&self.asked, format!("{}\n", self.login))
    }
}

/// Run the offer to its end on `bus`, a connection of the caller's that nothing else reads. Blocks while the person
/// decides.
pub fn run(bus: &Connection, setup: &Setup) -> Outcome {
    let (mut flow, steps) = Flow::start(setup.remembered(), setup.asked());
    let mut steps = VecDeque::from(steps);
    while let Some(step) = steps.pop_front() {
        let next = match step {
            Step::Observe => {
                let observed = service(bus, setup.wait)
                    .then(|| secrets::observe(bus))
                    .and_then(|observed| observed.map_err(|e| eprintln!("{LOG}: {e}")).ok());
                flow.observed(observed.as_ref())
            }
            Step::Ask(offer) => {
                // a login that cannot note it asked does not ask, or it would ask again each time this starts
                let answer = match setup.note_asked() {
                    Ok(()) => {
                        let (heading, description) = (offer.heading(), offer.description());
                        ask(
                            bus,
                            &setup.prompter,
                            &Question {
                                heading: &heading,
                                description: &description,
                                yes: MOVE,
                                no: KEEP,
                            },
                        )
                    }
                    Err(error) => {
                        eprintln!("{LOG}: noting that this login asked: {error}");
                        Answer::Dismissed
                    }
                };
                flow.answered(answer)
            }
            Step::Unlock(collection) => flow.unlocked(secrets::unlock(bus, &collection)),
            Step::Move(collection) => {
                let result = Secrets::open(bus)
                    .map_err(|cause| MoveError {
                        step: FailedStep::Read,
                        cause: Some(cause),
                        left: 0,
                    })
                    .and_then(|mut store| move_items(&mut store, &collection, LOGIN));
                if let Err(error) = &result {
                    eprintln!("{LOG}: the move to the login keyring did not finish: {error}");
                }
                flow.moved(result)
            }
            Step::Remember(answer) => {
                if let Err(error) = setup.remember(answer) {
                    eprintln!("{LOG}: keeping the answer about the default keyring: {error}");
                }
                Vec::new()
            }
            Step::Tell(notice) => flow.answered(ask(
                bus,
                &setup.prompter,
                &Question {
                    heading: &notice.heading,
                    description: &notice.description,
                    yes: TRY_AGAIN,
                    no: NOT_NOW,
                },
            )),
            Step::End(outcome) => return outcome,
        };
        steps.extend(next);
    }
    Outcome::Quiet
}

const LOG: &str = "cupertino-keyring-prompter";

/// Whether a Secret Service is on the bus, looked for until `wait` is over.
fn service(bus: &Connection, wait: Duration) -> bool {
    let until = Instant::now() + wait;
    loop {
        if secrets::present(bus) {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(LOOK_EVERY.min(wait));
    }
}

/// Ask `question` in the alert of the prompter named `prompter` and wait for the answer. Only the alert's buttons
/// answer: Escape, and an answer the prompter gave itself because the session went away or the alert could not be
/// shown, is [`Answer::Dismissed`], and so is any failure to ask.
pub fn ask(bus: &Connection, prompter: &str, question: &Question<'_>) -> Answer {
    match try_ask(bus, prompter, question) {
        Ok(answer) => answer,
        Err(error) => {
            eprintln!("{LOG}: asking about the default keyring: {error}");
            Answer::Dismissed
        }
    }
}

/// Send a call to the prompter without waiting; the serial its reply names.
fn send(
    bus: &Connection,
    prompter: &str,
    member: &str,
    body: &(impl zbus::export::serde::Serialize + zbus::zvariant::DynamicType),
) -> zbus::Result<std::num::NonZeroU32> {
    let message = zbus::Message::method_call(PATH, member)?
        .destination(prompter)?
        .interface(INTERFACE)?
        .build(body)?;
    bus.send(&message)?;
    Ok(message.primary_header().serial_num())
}

fn try_ask(bus: &Connection, prompter: &str, question: &Question<'_>) -> zbus::Result<Answer> {
    let callback = ObjectPath::try_from(CALLBACK)?;
    // the prompter's name as the bus writes it on what the prompter sends
    let owner = zbus::blocking::fdo::DBusProxy::new(bus)?
        .get_name_owner(prompter.try_into()?)?
        .to_string();
    let mut exchange =
        Exchange::new().map_err(|e| zbus::Error::Failure(format!("the exchange: {e}")))?;
    // everything the connection receives, in the order it came: the prompter's answers and the replies to the calls
    let incoming = MessageIterator::from(bus);
    let begin = send(bus, prompter, "BeginPrompting", &(&callback,))?;
    let mut asked = None;
    // the person's answer and the call that ends the prompt, once both are known
    let mut stopping: Option<(Answer, std::num::NonZeroU32)> = None;
    for message in incoming {
        let message = message?;
        let header = message.header();
        match header.message_type() {
            Type::MethodCall
                if header.path().map(|p| p.as_str()) == Some(CALLBACK)
                    && header.interface().map(|i| i.as_str()) == Some(CALLBACK_INTERFACE)
                    && header.sender().map(|s| s.as_str()) == Some(owner.as_str()) =>
            {
                bus.reply(&header, &())?;
                match header.member().map(|m| m.as_str()) {
                    Some("PromptReady") => {
                        let (reply, _, theirs): (String, HashMap<String, OwnedValue>, String) =
                            message.body().deserialize()?;
                        if stopping.is_some() {
                            continue;
                        }
                        if asked.is_none() {
                            // the prompter is ready for the question
                            exchange
                                .receive(&theirs)
                                .map_err(|e| zbus::Error::Failure(format!("the exchange: {e}")))?;
                            let properties: HashMap<&str, Value<'_>> = HashMap::from([
                                ("title", Value::from(question.heading)),
                                ("message", Value::from(question.heading)),
                                ("description", Value::from(question.description)),
                                ("continue-label", Value::from(question.yes)),
                                ("cancel-label", Value::from(question.no)),
                                // only the button is a no: Escape ends the prompt, which is no answer
                                (ESCAPE_DISMISSES, Value::from(true)),
                            ]);
                            asked = Some(send(
                                bus,
                                prompter,
                                "PerformPrompt",
                                &(&callback, "confirm", properties, exchange.begin()),
                            )?);
                        } else {
                            let answer = if reply == "yes" {
                                Answer::Yes
                            } else {
                                Answer::No
                            };
                            let stop = send(bus, prompter, "StopPrompting", &(&callback,))?;
                            stopping = Some((answer, stop));
                        }
                    }
                    // the prompter ended the prompt itself: a no it sent just before was not the person's
                    Some("PromptDone") => match stopping {
                        Some((Answer::Yes, _)) => {}
                        _ => return Ok(Answer::Dismissed),
                    },
                    _ => {}
                }
            }
            Type::MethodReturn | Type::Error => {
                let serial = header.reply_serial();
                if let Some((answer, stop)) = stopping {
                    if serial == Some(stop) {
                        return Ok(answer);
                    }
                }
                if header.message_type() == Type::Error
                    && (serial == Some(begin) || serial.is_some() && serial == asked)
                {
                    // refused: too many prompts wait, or the session went away before the question
                    let _ = send(bus, prompter, "StopPrompting", &(&callback,));
                    return Ok(Answer::Dismissed);
                }
            }
            _ => {}
        }
    }
    Err(zbus::Error::Failure("the bus closed".into()))
}
