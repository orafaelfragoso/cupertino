//! The keyboard's two panels, which the on-screen displays draw (`docs/architecture/shell-host.md`, *The input-source
//! switcher and the accent picker*): the list of the input sources while a switch is under way and the candidates of a
//! held letter. The server keeps their keys, their choice and the text they change; the component shows what
//! [`crate::Event::KeyboardPanels`] says and reports a click on a candidate with [`accent_pick`]. Only the component
//! that holds the `keyboard_panels` role gets these events.

use cupertino_kit::geometry::Rect;

use crate::context::{self, ShellRequest};
use crate::outputs::OutputId;

/// The layer-surface namespace of the switcher's panel: the server knows the panel by it.
pub const SOURCES_NAMESPACE: &str = "cupertino-osd-input-sources";
/// The layer-surface namespace of the picker's panel: a press on it goes to the panel and leaves the picker open.
pub const ACCENTS_NAMESPACE: &str = "cupertino-osd-accents";

/// One input source of a switch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// The source's id in the keyboard settings, such as `us` or `fr(nodeadkeys)`.
    pub id: String,
    /// The name it is shown with.
    pub name: String,
}

/// What the server says of the panels, one change at a time.
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    /// A switch opened or its selection moved: the whole list, most recently used first, and the selected entry. The
    /// panel shows centred on `output`; `None` names an output the connection has not bound, and the panel then goes on
    /// the first one.
    Sources {
        output: Option<OutputId>,
        sources: Vec<Source>,
        selected: usize,
    },
    /// The switch ended, committed or cancelled; the panel fades out.
    SourcesClosed,
    /// A picker opened or its focus moved. `serial` counts the pickers of the session; `anchor` is the text cursor's
    /// rectangle in the logical points of `output` from its top left corner; `top` is the height at the output's top the
    /// picker stays below; `focused` is the focused candidate.
    Accents {
        serial: u32,
        output: Option<OutputId>,
        anchor: Rect,
        top: f64,
        candidates: Vec<String>,
        focused: Option<usize>,
    },
    /// The picker of `serial` is gone.
    AccentsClosed { serial: u32 },
}

/// The pointer was released on candidate `index` of the picker of `serial`: the server inserts it into the field it
/// edits, when that picker is still the open one and has the candidate.
pub fn accent_pick(serial: u32, index: u32) {
    context::push_request(ShellRequest::AccentPick { serial, index });
}
