//! Window manager of the Cupertino desktop: the stage policy of one output ([`Controller`], on the
//! `cupertino-stage` model) and its settings file ([`conf`]). Pure logic; the `cupertino-windowmanager`
//! process (feature `app`) feeds the server's window events in and sends the requests out.

pub mod conf;
pub mod controller;

pub use conf::{parse_conf, ConfProblem, WindowManagerFile};
pub use controller::{Change, Controller, Request, Window};
