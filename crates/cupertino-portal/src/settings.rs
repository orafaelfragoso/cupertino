//! `org.freedesktop.impl.portal.Settings`: the session's appearance in the `org.freedesktop.appearance` namespace, which
//! xdg-desktop-portal hands to applications, and `SettingChanged` whenever it changes.

use std::collections::HashMap;

use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, Value};
use zbus::{fdo, interface, Connection};

use crate::PORTAL_PATH;

pub const NAMESPACE: &str = "org.freedesktop.appearance";

/// What the session tells its clients about how to look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Appearance {
    pub dark: bool,
    pub increase_contrast: bool,
    pub reduce_motion: bool,
}

impl Appearance {
    /// The namespace's keys and values: `color-scheme` 1 prefers dark and 2 light; `contrast` and `reduced-motion` are 1
    /// when asked for.
    pub fn values(self) -> [(&'static str, u32); 3] {
        [
            ("color-scheme", if self.dark { 1 } else { 2 }),
            ("contrast", u32::from(self.increase_contrast)),
            ("reduced-motion", u32::from(self.reduce_motion)),
        ]
    }

    /// The keys whose values differ in `new`, with their new values.
    pub fn changes(self, new: Appearance) -> Vec<(&'static str, u32)> {
        self.values()
            .into_iter()
            .zip(new.values())
            .filter(|(old, new)| old != new)
            .map(|(_, new)| new)
            .collect()
    }
}

/// Whether `namespace` is asked for: no patterns or an empty one asks for every namespace, and a pattern may end in `*`.
pub fn matches(patterns: &[String], namespace: &str) -> bool {
    patterns.is_empty()
        || patterns.iter().any(|p| match p.strip_suffix('*') {
            Some(prefix) => namespace.starts_with(prefix),
            None => p.is_empty() || p == namespace,
        })
}

pub struct Settings {
    appearance: Appearance,
}

#[interface(name = "org.freedesktop.impl.portal.Settings")]
impl Settings {
    fn read_all(&self, namespaces: Vec<String>) -> HashMap<String, HashMap<String, OwnedValue>> {
        if !matches(&namespaces, NAMESPACE) {
            return HashMap::new();
        }
        let values = self
            .appearance
            .values()
            .into_iter()
            .map(|(key, value)| (key.to_owned(), OwnedValue::from(value)))
            .collect();
        HashMap::from([(NAMESPACE.to_owned(), values)])
    }

    fn read(&self, namespace: &str, key: &str) -> fdo::Result<OwnedValue> {
        self.appearance
            .values()
            .into_iter()
            .find(|(k, _)| namespace == NAMESPACE && *k == key)
            .map(|(_, value)| OwnedValue::from(value))
            .ok_or_else(|| fdo::Error::UnknownProperty(format!("{namespace} {key}")))
    }

    #[zbus(signal)]
    async fn setting_changed(
        emitter: &SignalEmitter<'_>,
        namespace: &str,
        key: &str,
        value: Value<'_>,
    ) -> zbus::Result<()>;

    #[zbus(property)]
    fn version(&self) -> u32 {
        1
    }
}

/// The served interface; it stays on the bus while the connection lives.
pub struct Service {
    connection: Connection,
}

impl Service {
    /// Serves the interface at the portal's path with `appearance`.
    pub fn serve(
        connection: &zbus::blocking::Connection,
        appearance: Appearance,
    ) -> zbus::Result<Self> {
        connection
            .object_server()
            .at(PORTAL_PATH, Settings { appearance })?;
        Ok(Self {
            connection: connection.inner().clone(),
        })
    }

    /// Takes on `appearance` and emits `SettingChanged` for each key it changes.
    pub fn set(&self, appearance: Appearance) -> zbus::Result<()> {
        zbus::block_on(async {
            let interface = self
                .connection
                .object_server()
                .interface::<_, Settings>(PORTAL_PATH)
                .await?;
            let mut settings = interface.get_mut().await;
            let changes = settings.appearance.changes(appearance);
            settings.appearance = appearance;
            for (key, value) in changes {
                Settings::setting_changed(
                    interface.signal_emitter(),
                    NAMESPACE,
                    key,
                    Value::from(value),
                )
                .await?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_matches_its_namespace_every_namespace_or_a_trailing_prefix() {
        let patterns = |p: &[&str]| p.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(matches(&[], NAMESPACE));
        assert!(matches(&patterns(&[""]), NAMESPACE));
        assert!(matches(
            &patterns(&["org.freedesktop.appearance"]),
            NAMESPACE
        ));
        assert!(matches(&patterns(&["org.freedesktop.*"]), NAMESPACE));
        assert!(!matches(&patterns(&["org.gnome.*"]), NAMESPACE));
        assert!(!matches(
            &patterns(&["org.freedesktop.appearance.x"]),
            NAMESPACE
        ));
    }

    #[test]
    fn only_the_keys_that_differ_change() {
        let light = Appearance::default();
        let dark = Appearance {
            dark: true,
            ..light
        };
        assert_eq!(light.changes(dark), [("color-scheme", 1)]);
        assert_eq!(light.changes(light), []);
        let contrast = Appearance {
            increase_contrast: true,
            reduce_motion: true,
            ..dark
        };
        assert_eq!(
            dark.changes(contrast),
            [("contrast", 1), ("reduced-motion", 1)]
        );
    }
}
