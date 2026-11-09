//! Results that arrive later than the query: each query starts a new [`Generation`], every slow provider (the file
//! search, anything a host runs off the loop) is asked with it and with a [`Cancel`] token, and its reply comes back as
//! an [`Update`] tagged with the generation it answers. [`Tracker`] keeps the current generation's state per provider:
//! a reply for an older generation is stale and dropped, a provider that has not answered by the deadline times out,
//! and starting a new generation cancels the previous one's tokens so a host can stop the work.
//!
//! The tracker tells the panel what to say when nothing shows ([`Status`]): still searching, no results, or that a
//! source failed. "No results" and "failed" are kept apart: a failure is never shown as an empty answer.
//!
//! The tracker is pure; the clock is passed in.

use crate::engine::Section;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The query a reply answers. Each new query takes the next one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(pub u64);

/// A provider that answers off the loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Provider {
    /// The file search over the built file index.
    Files,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Provider::Files => "Files",
        }
    }
}

/// How long a provider may take before its answer is given up, in seconds.
pub const DEADLINE: f64 = 0.5;

/// A provider's reply.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    /// Its sections (possibly none: it found nothing).
    Sections(Vec<Section>),
    /// It could not answer; the reason is for the log and assistive technology.
    Failed(String),
}

/// A reply tagged with the generation it answers.
#[derive(Clone, Debug, PartialEq)]
pub struct Update {
    pub generation: Generation,
    pub provider: Provider,
    pub reply: Reply,
}

/// Whether an update was taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    Taken,
    /// It answers an older query, a provider that was not asked, or one that already answered or timed out.
    Stale,
}

/// One provider's state in the current generation.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Pending,
    Ready(Vec<Section>),
    Failed(String),
    TimedOut,
}

/// What the panel says when no result shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// No query: nothing to say.
    Idle,
    /// Results show.
    Results,
    /// Nothing yet, and a provider is still searching.
    Searching,
    /// Every provider answered and none found anything.
    NoResults,
    /// Nothing found, and a provider failed or timed out: what is shown may be incomplete.
    Failed(Vec<Provider>),
}

/// A token a host checks while it works for a generation: it is cancelled once a newer generation starts.
#[derive(Clone, Debug)]
pub struct Cancel {
    current: Arc<AtomicU64>,
    generation: Generation,
}

impl Cancel {
    pub fn generation(&self) -> Generation {
        self.generation
    }

    pub fn is_cancelled(&self) -> bool {
        self.current.load(Ordering::Acquire) != self.generation.0
    }
}

/// The current generation and its providers' states.
#[derive(Debug)]
pub struct Tracker {
    generation: Generation,
    current: Arc<AtomicU64>,
    sources: BTreeMap<Provider, Source>,
    deadline: f64,
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

impl Tracker {
    pub fn new() -> Self {
        Self {
            generation: Generation(0),
            current: Arc::new(AtomicU64::new(0)),
            sources: BTreeMap::new(),
            deadline: f64::INFINITY,
        }
    }

    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// Start a new generation at `now` that waits for `providers`; the previous generation's tokens are cancelled.
    pub fn begin(&mut self, providers: &[Provider], now: f64) -> Generation {
        self.generation = Generation(self.generation.0 + 1);
        self.current.store(self.generation.0, Ordering::Release);
        self.sources = providers.iter().map(|&p| (p, Source::Pending)).collect();
        self.deadline = if providers.is_empty() {
            f64::INFINITY
        } else {
            now + DEADLINE
        };
        self.generation
    }

    /// Cancel the current generation without starting another (the panel closed).
    pub fn cancel(&mut self) {
        self.begin(&[], 0.0);
    }

    /// The token for work done for the current generation.
    pub fn token(&self) -> Cancel {
        Cancel {
            current: Arc::clone(&self.current),
            generation: self.generation,
        }
    }

    /// When the pending providers time out, while some are pending.
    pub fn deadline(&self) -> Option<f64> {
        self.pending().then_some(self.deadline)
    }

    pub fn pending(&self) -> bool {
        self.sources.values().any(|s| *s == Source::Pending)
    }

    pub fn source(&self, provider: Provider) -> Option<&Source> {
        self.sources.get(&provider)
    }

    /// Take `update` if it answers the current generation for a provider still pending.
    pub fn apply(&mut self, update: Update) -> Applied {
        if update.generation != self.generation {
            return Applied::Stale;
        }
        match self.sources.get_mut(&update.provider) {
            Some(source @ Source::Pending) => {
                *source = match update.reply {
                    Reply::Sections(sections) => Source::Ready(sections),
                    Reply::Failed(reason) => Source::Failed(reason),
                };
                Applied::Taken
            }
            _ => Applied::Stale,
        }
    }

    /// Give up on the providers still pending at `now` once the deadline passed; their tokens are cancelled. Returns
    /// whether any timed out.
    pub fn expire(&mut self, now: f64) -> bool {
        if now < self.deadline || !self.pending() {
            return false;
        }
        for source in self.sources.values_mut() {
            if *source == Source::Pending {
                *source = Source::TimedOut;
            }
        }
        // what still runs for this generation stops; a late reply is stale anyway
        self.current.store(u64::MAX, Ordering::Release);
        true
    }

    /// Every section the providers answered so far, in provider order.
    pub fn sections(&self) -> Vec<Section> {
        self.sources
            .values()
            .filter_map(|s| match s {
                Source::Ready(sections) => Some(sections.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// What to say with `shown` results for a query that is `typed` or not.
    pub fn status(&self, typed: bool, shown: usize) -> Status {
        if !typed {
            return Status::Idle;
        }
        if shown > 0 {
            return Status::Results;
        }
        if self.pending() {
            return Status::Searching;
        }
        let failed: Vec<Provider> = self
            .sources
            .iter()
            .filter(|(_, s)| matches!(s, Source::Failed(_) | Source::TimedOut))
            .map(|(&p, _)| p)
            .collect();
        if failed.is_empty() {
            Status::NoResults
        } else {
            Status::Failed(failed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Action, ItemKind, ResultItem, SectionKind};

    fn section(title: &str) -> Section {
        Section {
            kind: SectionKind::Files,
            title: "Files".into(),
            items: vec![ResultItem {
                id: format!("file:/{title}"),
                kind: ItemKind::File,
                title: title.into(),
                subtitle: String::new(),
                icon: None,
                score: 1.0,
                highlights: Vec::new(),
                action: Action::OpenPath(format!("/{title}").into()),
            }],
        }
    }

    fn update(generation: Generation, reply: Reply) -> Update {
        Update {
            generation,
            provider: Provider::Files,
            reply,
        }
    }

    #[test]
    fn a_reply_for_an_older_query_is_stale_and_its_token_cancelled() {
        let mut t = Tracker::new();
        let first = t.begin(&[Provider::Files], 0.0);
        let old = t.token();
        assert!(!old.is_cancelled());
        let second = t.begin(&[Provider::Files], 0.1);
        assert!(second > first);
        assert!(
            old.is_cancelled(),
            "a newer query cancels the older one's work"
        );
        assert!(!t.token().is_cancelled());
        assert_eq!(
            t.apply(update(first, Reply::Sections(vec![section("old")]))),
            Applied::Stale
        );
        assert!(t.sections().is_empty());
        assert_eq!(
            t.apply(update(second, Reply::Sections(vec![section("new")]))),
            Applied::Taken
        );
        assert_eq!(t.sections()[0].items[0].title, "new");
        // a second reply for the same provider changes nothing
        assert_eq!(
            t.apply(update(second, Reply::Sections(Vec::new()))),
            Applied::Stale
        );
        assert_eq!(t.sections().len(), 1);
    }

    #[test]
    fn no_results_and_a_failure_are_told_apart() {
        let mut t = Tracker::new();
        let g = t.begin(&[Provider::Files], 0.0);
        assert_eq!(t.status(true, 0), Status::Searching);
        assert_eq!(t.status(true, 2), Status::Results);
        assert_eq!(t.status(false, 0), Status::Idle);
        t.apply(update(g, Reply::Sections(Vec::new())));
        assert_eq!(t.status(true, 0), Status::NoResults);

        let g = t.begin(&[Provider::Files], 1.0);
        t.apply(update(g, Reply::Failed("index unreadable".into())));
        assert_eq!(t.status(true, 0), Status::Failed(vec![Provider::Files]));
        assert_eq!(
            t.source(Provider::Files),
            Some(&Source::Failed("index unreadable".into()))
        );
        // results from the synchronous providers still show over a failure
        assert_eq!(t.status(true, 1), Status::Results);
    }

    #[test]
    fn a_provider_that_misses_its_deadline_times_out_and_its_late_reply_is_dropped() {
        let mut t = Tracker::new();
        let g = t.begin(&[Provider::Files], 10.0);
        let token = t.token();
        assert_eq!(t.deadline(), Some(10.0 + DEADLINE));
        assert!(!t.expire(10.0 + DEADLINE / 2.0));
        assert_eq!(t.status(true, 0), Status::Searching);
        assert!(t.expire(10.0 + DEADLINE));
        assert!(token.is_cancelled(), "the timed-out work is told to stop");
        assert_eq!(t.status(true, 0), Status::Failed(vec![Provider::Files]));
        assert_eq!(t.deadline(), None);
        assert_eq!(
            t.apply(update(g, Reply::Sections(vec![section("late")]))),
            Applied::Stale
        );
        assert!(t.sections().is_empty());
    }

    #[test]
    fn a_query_without_slow_providers_has_nothing_pending() {
        let mut t = Tracker::new();
        t.begin(&[], 0.0);
        assert!(!t.pending());
        assert_eq!(t.deadline(), None);
        assert_eq!(t.status(true, 0), Status::NoResults);
        t.cancel();
        assert!(!t.pending());
    }
}
