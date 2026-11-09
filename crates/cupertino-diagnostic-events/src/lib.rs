//! Versioned diagnostic events of every Cupertino process (`cupertino.diag.v1`): what failed, where and in which
//! build, reduced by a tested redaction policy, rate-limited per id, kept in a memory ring and written to a bounded store
//! (user processes) or the journal (system services).
//!
//! ```no_run
//! use cupertino_diagnostic_events::{self as diag, component, Severity, Sink, Value};
//!
//! // once, first thing in `main`
//! diag::init(component!(), "dock", Sink::UserStore);
//! // beside a failure's existing log line and error return
//! # let e = std::io::Error::other("x");
//! diag::event(Severity::Warn, "dock.icon.load_failed", "an icon could not be loaded")
//!     .field("app_id", Value::Public("org.example.App"))
//!     .field("error", Value::Public(&e.to_string()))
//!     .emit();
//! ```
//!
//! The README lists every event id, the record's fields, the limits and the redaction policy.

mod emitter;
mod event;
pub mod journal;
pub mod rate;
pub mod redact;
pub mod store;
pub mod time;

pub use cupertino_build_info::{component, Component};
pub use emitter::{
    boot_id, dropped_before_install, exit_error, flush, init, install_panic_hook, recent,
    set_context, Emitter, FatalRecord, Sink, FATAL_LINE, RING_BYTES, RING_EVENTS,
    STORE_UNAVAILABLE, TEST_PANIC_ENV,
};
pub use event::{
    event, AddressKind, Corrupt, Event, Fields, Identity, Origin, Record, Severity, Value,
    FIELD_TEXT_LIMIT, SCHEMA,
};
pub use rate::{RATE_PER_ID, RATE_WINDOW};
pub use store::{EVENT_BYTES, RETENTION_DAYS, SEGMENT_BYTES, STORE_BYTES};
