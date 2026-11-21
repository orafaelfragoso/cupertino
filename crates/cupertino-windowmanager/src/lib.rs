//! Window manager of the Cupertino desktop, a component of the shell host ([`WindowManager`]): the stage policy of each
//! output ([`Controller`], on the `cupertino_wm::stage` model), placed in transactions, the stage strip's surface
//! ([`strip`]), whose thumbnails are holes the server draws the strip's windows in, and its settings file ([`conf`]).

#![forbid(unsafe_code)]

pub mod component;
pub mod conf;
pub mod controller;
pub mod strip;

pub use component::WindowManager;
pub use conf::{parse_conf, ConfProblem, WindowManagerFile};
pub use controller::{Change, Controller, Request, Window};
