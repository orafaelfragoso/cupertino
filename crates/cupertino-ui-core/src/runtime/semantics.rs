//! Export the runtime's existing semantics; actions take the same native route as assistive actions.
use super::*;
use cupertino_semantics as wire;
#[cfg(test)]
#[path = "semantic-tests.rs"]
mod tests;

fn id(node: NodeId) -> u64 {
    (u64::from(node.generation().get()) << 32) | u64::from(node.index())
}
fn action(a: crate::Action) -> wire::Action {
    match a {
        crate::Action::Click => wire::Action::Click,
        crate::Action::Focus => wire::Action::Focus,
        crate::Action::Increment => wire::Action::Increment,
        crate::Action::Decrement => wire::Action::Decrement,
        crate::Action::Expand => wire::Action::Expand,
        crate::Action::Collapse => wire::Action::Collapse,
        crate::Action::ShowMenu => wire::Action::ShowMenu,
        crate::Action::ScrollIntoView => wire::Action::ScrollIntoView,
        crate::Action::SetValue => wire::Action::SetValue,
        crate::Action::SetSelection => wire::Action::SetSelection,
    }
}
fn name(role: crate::Role) -> String {
    let mut out = String::new();
    for (c, n) in format!("{role:?}").chars().enumerate() {
        if n.is_ascii_uppercase() && c != 0 {
            out.push('-');
        }
        out.push(n.to_ascii_lowercase());
    }
    out
}
fn bounded(text: &str) -> String {
    let mut end = text.len().min(wire::MAX_TEXT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}
fn node(key: NodeId, n: &AccessibilityNode) -> wire::Node {
    let protected = n.role == crate::Role::PasswordInput
        || matches!(n.value, Some(crate::AccessibleValue::Protected));
    let value = if protected {
        Some(wire::Value::Protected)
    } else {
        n.value.as_ref().map(|v| match v {
            crate::AccessibleValue::Protected => wire::Value::Protected,
            crate::AccessibleValue::Text { value, .. } => wire::Value::Text {
                text: bounded(value),
            },
            crate::AccessibleValue::Number {
                value,
                min,
                max,
                step,
            } => wire::Value::Number {
                value: *value,
                min: *min,
                max: *max,
                step: *step,
            },
            crate::AccessibleValue::Toggle(v) => wire::Value::Toggle {
                checked: match v {
                    crate::Checked::True => Some(true),
                    crate::Checked::False => Some(false),
                    crate::Checked::Mixed => None,
                },
            },
        })
    };
    let s = n.states;
    wire::Node {
        truncated: n.name.as_ref().is_some_and(|s| s.len() > wire::MAX_TEXT)
            || n.description
                .as_ref()
                .is_some_and(|s| s.len() > wire::MAX_TEXT)
            || (!protected
                && matches!(&n.value,Some(crate::AccessibleValue::Text{value,..}) if value.len()>wire::MAX_TEXT)),
        position: wire::Position {
            item_count: n.item_count,
            item_index: n.item_index,
            column_index: n.column_index,
            level: n.level,
        },
        id: id(key),
        role: name(n.role),
        name: n.name.as_deref().map(bounded),
        description: n.description.as_deref().map(bounded),
        value,
        bounds: wire::Bounds {
            x: n.bounds.x(),
            y: n.bounds.y(),
            width: n.bounds.width(),
            height: n.bounds.height(),
        },
        states: wire::States {
            disabled: s.disabled,
            focusable: s.focusable,
            focused: s.focused,
            selected: s.selected,
            expanded: s.expanded,
            busy: s.busy,
            read_only: s.read_only,
            required: s.required,
            invalid: s.invalid,
            modal: s.modal,
            multiselectable: s.multiselectable,
        },
        actions: if protected {
            Vec::new()
        } else {
            n.actions.iter().copied().map(action).collect()
        },
        children: n.children.iter().copied().map(id).collect(),
    }
}
impl<A: Application, W: WindowSystem> Runtime<A, W> {
    pub(super) fn semantic_actions(&mut self) {
        for (window, call) in self.system.semantic_requests() {
            let outcome = if let Some(i) = self.windows.iter().position(|w| w.id == window) {
                let w = &self.windows[i];
                let target = w
                    .accessibility
                    .keys()
                    .copied()
                    .find(|n| id(*n) == call.node);
                if !self.system.semantics_active(window) {
                    wire::Outcome::Unavailable
                } else if w.dirty
                    || !w.prepared
                    || w.semantic_snapshot
                        .as_ref()
                        .is_none_or(|s| s.revision != call.revision)
                {
                    wire::Outcome::Stale
                } else if !w
                    .semantic_snapshot
                    .as_ref()
                    .is_some_and(|s| s.permits(call.node, call.action, call.data.as_ref()))
                {
                    wire::Outcome::Denied
                } else if let Some(target) = target.filter(|n| self.tree.contains(*n)) {
                    let a = w.accessibility[&target]
                        .actions
                        .iter()
                        .copied()
                        .find(|a| action(*a) == call.action);
                    if let Some(a) = a {
                        let data = call.data.as_ref().map(|d| match d {
                            wire::Data::Text(s) => crate::ActionData::Text(s.clone()),
                            wire::Data::Number(n) => crate::ActionData::Number(*n),
                            wire::Data::Selection { start, end } => {
                                crate::ActionData::Selection(*start..*end)
                            }
                        });
                        let request = crate::ActionRequest {
                            target,
                            action: a,
                            data,
                        };
                        // whatever came last, a menu the action opens shows its selection, as one the Menu key
                        // opens does
                        self.keyboard = true;
                        let mut effects = Effects::default();
                        route(
                            &mut self.tree,
                            &self.windows[i],
                            self.options.clock.now(),
                            &mut effects,
                        )
                        .notify(target, &Event::Action(request));
                        self.apply(i, effects);
                        wire::Outcome::Applied
                    } else {
                        wire::Outcome::Unsupported
                    }
                } else {
                    wire::Outcome::Stale
                }
            } else {
                wire::Outcome::Unavailable
            };
            self.semantic_finishing.push((window, call, outcome));
        }
    }
    pub(super) fn sync_semantics(&mut self) {
        for w in &mut self.windows {
            if w.fade.is_some() {
                continue;
            }
            if !self.system.semantics_active(w.id) {
                w.semantic_snapshot = None;
                continue;
            }
            let Some(root) = w.root.filter(|_| w.prepared) else {
                continue;
            };
            let mut snapshot = wire::Snapshot {
                revision: w.semantic_revision,
                root: id(root),
                focus: w.accessibility_focus.map(id),
                nodes: w
                    .accessibility
                    .iter()
                    .map(|(key, n)| node(*key, n))
                    .collect(),
            };
            let changed = w.semantic_snapshot.as_ref().is_none_or(|s| {
                s.root != snapshot.root || s.focus != snapshot.focus || s.nodes != snapshot.nodes
            });
            if changed {
                let Some(revision) = w.semantic_revision.checked_add(1) else {
                    continue;
                };
                w.semantic_revision = revision;
                snapshot.revision = revision;
                w.semantic_snapshot = Some(snapshot);
            }
            if let Some(snapshot) = &w.semantic_snapshot {
                self.system.publish_semantics(w.id, snapshot);
            }
        }
        let finishing = std::mem::take(&mut self.semantic_finishing);
        for (window, call, outcome) in finishing {
            if outcome == wire::Outcome::Applied
                && self
                    .windows
                    .iter()
                    .find(|w| w.id == window)
                    .is_some_and(|w| !w.prepared || w.dirty)
            {
                self.semantic_finishing.push((window, call, outcome));
                continue;
            }
            let revision = self
                .windows
                .iter()
                .find(|w| w.id == window)
                .map_or(0, |w| w.semantic_revision);
            self.system
                .semantic_finished(window, &call, outcome, revision);
        }
    }
    pub(super) fn semantics_behind(&self) -> bool {
        self.windows.iter().any(|w| {
            self.system.semantics_active(w.id) && w.semantic_snapshot.is_none() && w.prepared
        })
    }
}
