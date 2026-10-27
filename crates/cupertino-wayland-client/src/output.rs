//! Outputs as `wl_output` describes them.

use crate::Scale;

/// An output, by its registry name; stable while it is plugged in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OutputId(pub(crate) u32);

/// What `wl_output` reported about an output, as of its last `done` event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputInfo {
    pub id: OutputId,
    /// The connector name (version 4), such as `DP-1`.
    pub name: Option<String>,
    /// A human-readable description (version 4).
    pub description: Option<String>,
    /// The integer scale the compositor renders this output at.
    pub scale: Scale,
    /// The current mode in device pixels.
    pub mode: Option<(u32, u32)>,
}

/// An output's properties as they arrive; [`OutputBuilder::done`] applies them at once.
#[derive(Clone, Debug)]
pub(crate) struct OutputBuilder {
    pub current: OutputInfo,
    pending: OutputInfo,
}

impl OutputBuilder {
    pub(crate) fn new(id: OutputId) -> Self {
        let info = OutputInfo {
            id,
            name: None,
            description: None,
            scale: Scale::ONE,
            mode: None,
        };
        Self {
            current: info.clone(),
            pending: info,
        }
    }

    pub(crate) fn pending(&mut self) -> &mut OutputInfo {
        &mut self.pending
    }

    /// Apply what arrived since the last `done`; whether anything changed.
    pub(crate) fn done(&mut self) -> bool {
        let changed = self.current != self.pending;
        self.current = self.pending.clone();
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_apply_together_at_done() {
        let mut b = OutputBuilder::new(OutputId(7));
        b.pending().scale = Scale::from_integer(2).unwrap();
        b.pending().name = Some("DP-1".into());
        assert_eq!(b.current.scale, Scale::ONE);
        assert!(b.done());
        assert_eq!(
            (b.current.scale, b.current.name.as_deref()),
            (Scale::from_integer(2).unwrap(), Some("DP-1"))
        );
        assert!(!b.done());
    }
}
