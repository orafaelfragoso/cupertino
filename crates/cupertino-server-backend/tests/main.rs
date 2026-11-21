//! The integration tests of `cupertino-server-backend`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[path = "backend-models.rs"]
mod backend_models;
#[path = "controller-deadline.rs"]
mod controller_deadline;
#[path = "session-errors.rs"]
mod session_errors;
