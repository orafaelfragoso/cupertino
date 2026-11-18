use serde::{Deserialize, Serialize};

/// The registry name of an output, shared by every component while the output is present.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OutputId(u32);
