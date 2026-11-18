use crate::ComponentName;
use serde::{Deserialize, Serialize};

/// A transaction named by its creator, the server's number for that instance, and the serial chosen by the creator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Id {
    pub component: ComponentName,
    pub instance: u32,
    pub serial: u32,
}
