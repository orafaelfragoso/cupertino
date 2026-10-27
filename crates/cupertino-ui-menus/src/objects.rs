//! The `org.gtk.Menus` and `org.gtk.Actions` objects on the bus.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use cupertino_menu_model::{ActionName, Scope};
use zbus::fdo;
use zbus::interface;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, Signature};

use crate::wire::{WireMenus, GROUP};
use crate::{Activation, WindowId};

type Dict = HashMap<String, OwnedValue>;
/// `(enabled, parameter type, state)` of one action.
type Description = (bool, Signature, Vec<OwnedValue>);

pub(crate) type OnActivate = Arc<dyn Fn(Activation) + Send + Sync>;

pub(crate) struct Menus {
    pub menus: WireMenus,
}

#[interface(name = "org.gtk.Menus")]
impl Menus {
    /// Every menu of the subscribed groups; the object has the one group.
    fn start(&self, groups: Vec<u32>) -> Vec<(u32, u32, Vec<Dict>)> {
        if !groups.contains(&GROUP) {
            return Vec::new();
        }
        self.menus
            .iter()
            .map(|(id, items)| (GROUP, *id, items.iter().map(|i| i.to_dict()).collect()))
            .collect()
    }

    fn end(&self, _groups: Vec<u32>) {}

    #[zbus(signal)]
    pub(crate) async fn changed(
        emitter: &SignalEmitter<'_>,
        changes: Vec<(u32, u32, u32, u32, Vec<Dict>)>,
    ) -> zbus::Result<()>;
}

/// The actions of one scope: stateless and without a parameter, each enabled or not.
pub(crate) struct Actions {
    pub scope: Scope,
    pub window: Option<WindowId>,
    pub enabled: BTreeMap<String, bool>,
    pub on_activate: OnActivate,
}

impl Actions {
    fn description(enabled: bool) -> Description {
        (enabled, Signature::Unit, Vec::new())
    }
}

#[interface(name = "org.gtk.Actions")]
impl Actions {
    fn list(&self) -> Vec<String> {
        self.enabled.keys().cloned().collect()
    }

    fn describe(&self, action_name: &str) -> fdo::Result<Description> {
        self.enabled
            .get(action_name)
            .map(|e| Actions::description(*e))
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("no action {action_name:?}")))
    }

    fn describe_all(&self) -> HashMap<String, Description> {
        self.enabled
            .iter()
            .map(|(name, e)| (name.clone(), Actions::description(*e)))
            .collect()
    }

    /// Run an enabled action; a disabled one is ignored, as a disabled menu item does nothing.
    fn activate(
        &self,
        action_name: &str,
        parameter: Vec<OwnedValue>,
        _platform_data: Dict,
    ) -> fdo::Result<()> {
        let enabled = *self
            .enabled
            .get(action_name)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("no action {action_name:?}")))?;
        if !parameter.is_empty() {
            return Err(fdo::Error::InvalidArgs(format!(
                "{action_name:?} takes no parameter"
            )));
        }
        if enabled {
            let action =
                ActionName::new(self.scope, action_name).expect("exported action names are valid");
            (self.on_activate)(Activation {
                window: self.window,
                action,
            });
        }
        Ok(())
    }

    fn set_state(
        &self,
        action_name: &str,
        _value: OwnedValue,
        _platform_data: Dict,
    ) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported(format!(
            "{action_name:?} has no state"
        )))
    }

    #[zbus(signal)]
    pub(crate) async fn changed(
        emitter: &SignalEmitter<'_>,
        removed: Vec<String>,
        enabled_changed: HashMap<String, bool>,
        state_changed: Dict,
        added: HashMap<String, Description>,
    ) -> zbus::Result<()>;
}

/// The `Changed` arguments that turn `old` into `new`.
pub(crate) fn action_changes(
    old: &BTreeMap<String, bool>,
    new: &BTreeMap<String, bool>,
) -> (
    Vec<String>,
    HashMap<String, bool>,
    HashMap<String, Description>,
) {
    let removed = old
        .keys()
        .filter(|k| !new.contains_key(*k))
        .cloned()
        .collect();
    let mut enabled_changed = HashMap::new();
    let mut added = HashMap::new();
    for (name, enabled) in new {
        match old.get(name) {
            None => {
                added.insert(name.clone(), Actions::description(*enabled));
            }
            Some(was) if was != enabled => {
                enabled_changed.insert(name.clone(), *enabled);
            }
            Some(_) => {}
        }
    }
    (removed, enabled_changed, added)
}
