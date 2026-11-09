//! The diagnostic events a test reads back: those its own process recorded (an in-process server with a memory-only
//! emitter; nextest runs every test in a process of its own), or those a server binary wrote to its store.

use std::path::Path;

use cupertino_diagnostic_events::{self as diag, Record};

/// Installs this test process's emitter, which keeps its events in memory only.
pub fn memory() {
    diag::init(diag::component!(), "server", diag::Sink::Memory);
}

/// The events of `id` this process recorded, oldest first.
pub fn recorded(id: &str) -> Vec<Record> {
    diag::recent()
        .iter()
        .filter_map(|line| Record::parse(line.trim_end()).ok())
        .filter(|record| record.id == id)
        .collect()
}

/// The events of `id` in the store in `dir` (`$XDG_STATE_HOME/cupertino/events`), oldest first; none without the store.
pub fn stored(dir: &Path, id: &str) -> Vec<Record> {
    diag::store::read(dir)
        .map(|contents| {
            contents
                .records
                .into_iter()
                .map(|stored| stored.record)
                .filter(|record| record.id == id)
                .collect()
        })
        .unwrap_or_default()
}
