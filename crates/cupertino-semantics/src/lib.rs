//! Cupertino-owned semantic wire values. This crate knows neither widgets nor compositor policy.
use serde::{Deserialize, Serialize};
#[cfg(test)]
mod tests;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::FileExt;

/// Maximum sealed publication from one surface, including all text and edges.
pub const MAX_FRAME: usize = 512 * 1024;
/// Maximum complete observation, never a mixture of pages from different revisions.
pub const MAX_OBSERVATION: usize = 4 * 1024 * 1024;
/// Maximum nodes per publication; bounds validation and compositor work.
pub const MAX_NODES: usize = 4096;
/// Maximum bytes of any user-provided text value or label.
pub const MAX_TEXT: usize = 4096;
/// Maximum semantic ancestry; bounds consumers' tree traversal.
pub const MAX_DEPTH: usize = 64;
/// Maximum surfaces in one complete desktop observation.
pub const MAX_WINDOWS: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl Bounds {
    pub fn valid(&self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite())
            && self.width >= 0.0
            && self.height >= 0.0
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Value {
    Text {
        text: String,
    },
    Number {
        value: f64,
        min: f64,
        max: f64,
        step: Option<f64>,
    },
    Toggle {
        checked: Option<bool>,
    },
    Protected,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Click,
    Focus,
    Increment,
    Decrement,
    Expand,
    Collapse,
    ShowMenu,
    ScrollIntoView,
    SetValue,
    SetSelection,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum Data {
    Text(String),
    Number(f64),
    Selection { start: usize, end: usize },
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct States {
    pub disabled: bool,
    pub focusable: bool,
    pub focused: bool,
    pub selected: bool,
    pub expanded: Option<bool>,
    pub busy: bool,
    pub read_only: bool,
    pub required: bool,
    pub invalid: bool,
    pub modal: bool,
    pub multiselectable: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub item_count: Option<usize>,
    pub item_index: Option<usize>,
    pub column_index: Option<usize>,
    pub level: Option<usize>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    /// Text was shortened to the wire bound; values and labels remain explicitly incomplete.
    pub truncated: bool,
    pub position: Position,
    pub id: u64,
    pub role: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub value: Option<Value>,
    pub states: States,
    pub bounds: Bounds,
    pub actions: Vec<Action>,
    pub children: Vec<u64>,
}
impl Node {
    pub fn protected(&self) -> bool {
        self.role == "password-input" || matches!(self.value, Some(Value::Protected))
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub revision: u64,
    pub root: u64,
    pub focus: Option<u64>,
    pub nodes: Vec<Node>,
}
impl Snapshot {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.revision == 0 || self.nodes.is_empty() || self.nodes.len() > MAX_NODES {
            return Err("invalid semantic snapshot size or revision");
        }
        let mut nodes = BTreeMap::new();
        for n in &self.nodes {
            if n.id == 0
                || nodes.insert(n.id, n).is_some()
                || !n.bounds.valid()
                || n.role.len() > 64
                || n.role.is_empty()
                || n.actions.len() > 10
                || n.children.len() > MAX_NODES
            {
                return Err("invalid semantic node");
            }
            if n.name.as_ref().is_some_and(|s| s.len() > MAX_TEXT)
                || n.description.as_ref().is_some_and(|s| s.len() > MAX_TEXT)
            {
                return Err("semantic text exceeds limit");
            }
            match &n.value {
                Some(Value::Text { text }) if text.len() > MAX_TEXT => {
                    return Err("semantic value exceeds limit")
                }
                Some(Value::Number {
                    value,
                    min,
                    max,
                    step,
                }) if !value.is_finite()
                    || !min.is_finite()
                    || !max.is_finite()
                    || min > max
                    || step.is_some_and(|s| !s.is_finite() || s <= 0.0) =>
                {
                    return Err("invalid semantic number")
                }
                _ => {}
            }
            if n.protected() && (n.value != Some(Value::Protected) || !n.actions.is_empty()) {
                return Err("protected node disclosed a value or actions");
            }
        }
        if self.focus.is_some_and(|id| !nodes.contains_key(&id)) {
            return Err("invalid semantic focus");
        }
        let mut seen = BTreeSet::new();
        let mut work = vec![(self.root, 0)];
        while let Some((id, depth)) = work.pop() {
            if depth > MAX_DEPTH || !seen.insert(id) {
                return Err("semantic cycle or shared child");
            }
            let n = nodes.get(&id).ok_or("missing semantic node")?;
            work.extend(n.children.iter().map(|id| (*id, depth + 1)));
        }
        if seen.len() != nodes.len() {
            return Err("unreachable semantic node");
        }
        Ok(())
    }
    pub fn permits(&self, node: u64, action: Action, data: Option<&Data>) -> bool {
        let Some(n) = self.nodes.iter().find(|n| n.id == node) else {
            return false;
        };
        if n.protected() || n.states.disabled || n.states.busy || !n.actions.contains(&action) {
            return false;
        }
        match (action, data, &n.value) {
            (Action::SetValue, Some(Data::Text(s)), Some(Value::Text { .. })) => {
                !n.states.read_only && s.len() <= MAX_TEXT
            }
            (Action::SetValue, Some(Data::Number(v)), Some(Value::Number { min, max, .. })) => {
                !n.states.read_only && v.is_finite() && v >= min && v <= max
            }
            (
                Action::SetSelection,
                Some(Data::Selection { start, end }),
                Some(Value::Text { text }),
            ) => start <= end && text.is_char_boundary(*start) && text.is_char_boundary(*end),
            (Action::SetValue | Action::SetSelection, _, _) => false,
            (_, None, _) => true,
            _ => false,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invocation {
    pub window: u64,
    pub revision: u64,
    pub node: u64,
    pub action: Action,
    pub data: Option<Data>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionCall {
    pub id: u64,
    pub epoch: u64,
    pub revision: u64,
    pub node: u64,
    pub action: Action,
    pub data: Option<Data>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Applied,
    Stale,
    Unavailable,
    Unsupported,
    Denied,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionReply {
    pub id: u64,
    pub epoch: u64,
    pub revision: u64,
    pub outcome: Outcome,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub handle: u64,
    pub app_id: String,
    pub title: String,
    pub origin: [f64; 2],
    pub scale: [f64; 2],
    pub focused: bool,
    pub snapshot: Option<Snapshot>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputGeometry {
    pub revision: u32,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
    /// Wayland transform name; captured PNGs are already upright in output orientation.
    pub transform: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub geometry: OutputGeometry,
    pub epoch: u64,
    pub revision: u64,
    pub geometry_revision: u32,
    pub windows: Vec<Window>,
}

/// Immutable regular memfds keep arbitrary client pipes, disks and mutable buffers off the compositor's loop.
pub fn seal<T: Serialize>(value: &T, limit: usize) -> io::Result<(File, u32)> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > limit {
        return Err(io::Error::other("semantic payload exceeds limit"));
    }
    let fd = rustix::fs::memfd_create(
        "cupertino-semantics",
        rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING,
    )?;
    let mut file = File::from(fd);
    file.write_all(&bytes)?;
    rustix::fs::fcntl_add_seals(file.as_fd(), required_seals())?;
    Ok((file, bytes.len() as u32))
}
fn required_seals() -> rustix::fs::SealFlags {
    use rustix::fs::SealFlags as S;
    S::SEAL | S::SHRINK | S::GROW | S::WRITE
}
pub fn read<T: serde::de::DeserializeOwned>(fd: OwnedFd, len: u32, limit: usize) -> io::Result<T> {
    let file = File::from(fd);
    if len as usize > limit
        || !rustix::fs::fcntl_get_seals(file.as_fd())?.contains(required_seals())
        || file.metadata()?.len() != u64::from(len)
    {
        return Err(io::Error::other(
            "semantic payload is not a bounded sealed memfd",
        ));
    }
    let mut bytes = vec![0; len as usize];
    file.read_exact_at(&mut bytes, 0)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}
