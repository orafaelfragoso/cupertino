//! Program tests: the headless backend, which drives an application's runtime without a display, and what a driver is
//! built from and reads, and the capture command's shared part. Only a program's dev-dependency enables it (the `test` feature).

pub mod capture;

pub use crate::start::{each_turn, preferences};
pub use cupertino_ui_core::platform::{Capabilities, WindowEvent, WindowStates};
pub use cupertino_ui_core::{PlaceholderIcons, Runtime, StepStatus};
pub use cupertino_ui_test::{HeadlessWindowSystem, ManualClock, TestDriver};
pub use cupertino_ui_text::FontIndex;
