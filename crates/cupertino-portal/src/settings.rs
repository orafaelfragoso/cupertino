//! `org.freedesktop.impl.portal.Settings`: the session's appearance in the `org.freedesktop.appearance` namespace and
//! the keys GTK reads from the GNOME namespace of interface settings (overlay scroll bars), which xdg-desktop-portal hands
//! to applications, and `SettingChanged` whenever one changes.

use std::collections::HashMap;

use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, Value};
use zbus::{fdo, interface, Connection};

use crate::PORTAL_PATH;

pub const NAMESPACE: &str = "org.freedesktop.appearance";
/// The namespace of the desktop's interface settings that GTK reads through the portal.
pub const INTERFACE_NAMESPACE: &str = "org.gnome.desktop.interface";

/// What the session tells its clients about how to look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Appearance {
    pub dark: bool,
    pub increase_contrast: bool,
    pub reduce_motion: bool,
    /// Scroll views keep tracks beside their content that stay shown, rather than thin bars over it while it scrolls.
    pub always_show_scroll_bars: bool,
}

/// The value of one key, in the type its namespace gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Number(u32),
    Flag(bool),
    Text(&'static str),
}

impl Setting {
    pub fn value(self) -> Value<'static> {
        match self {
            Setting::Number(n) => Value::from(n),
            Setting::Flag(b) => Value::from(b),
            Setting::Text(s) => Value::from(s),
        }
    }

    fn owned(self) -> OwnedValue {
        self.value()
            .try_into()
            .expect("a number, a flag or a text holds no file descriptor")
    }
}

impl Appearance {
    /// The appearance after `event`; events of other kinds leave it as it is.
    #[cfg(feature = "app")]
    pub fn after(mut self, event: &cupertino_protocol_app::AppEvent) -> Self {
        use cupertino_protocol_app::AppEvent;
        match event {
            AppEvent::Appearance { dark } => self.dark = *dark,
            AppEvent::DisplayOptions(options) => {
                self.increase_contrast = options.increase_contrast;
                self.reduce_motion = options.reduce_motion;
            }
            AppEvent::ScrollBars { always } => self.always_show_scroll_bars = *always,
            _ => {}
        }
        self
    }

    /// Whether scroll bars are thin bars over the content, shown while it scrolls.
    pub fn overlay_scroll_bars(self) -> bool {
        !self.always_show_scroll_bars
    }

    /// Every key with its namespace and value. In `org.freedesktop.appearance`, `color-scheme` 1 prefers dark and 2
    /// light, and `contrast` and `reduced-motion` are 1 when asked for. In `org.gnome.desktop.interface`,
    /// `overlay-scrolling` is whether scroll bars show over the content only while it scrolls.
    pub fn values(self) -> Vec<(&'static str, &'static str, Setting)> {
        vec![
            (
                NAMESPACE,
                "color-scheme",
                Setting::Number(if self.dark { 1 } else { 2 }),
            ),
            (
                NAMESPACE,
                "contrast",
                Setting::Number(u32::from(self.increase_contrast)),
            ),
            (
                NAMESPACE,
                "reduced-motion",
                Setting::Number(u32::from(self.reduce_motion)),
            ),
            (
                INTERFACE_NAMESPACE,
                "overlay-scrolling",
                Setting::Flag(self.overlay_scroll_bars()),
            ),
        ]
    }

    /// The keys whose values differ in `new`, with their namespaces and new values.
    pub fn changes(self, new: Appearance) -> Vec<(&'static str, &'static str, Setting)> {
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
        let mut all: HashMap<String, HashMap<String, OwnedValue>> = HashMap::new();
        for (namespace, key, value) in self.appearance.values() {
            if matches(&namespaces, namespace) {
                all.entry(namespace.to_owned())
                    .or_default()
                    .insert(key.to_owned(), value.owned());
            }
        }
        all
    }

    fn read(&self, namespace: &str, key: &str) -> fdo::Result<OwnedValue> {
        self.appearance
            .values()
            .into_iter()
            .find(|(n, k, _)| *n == namespace && *k == key)
            .map(|(_, _, value)| value.owned())
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
            for (namespace, key, value) in changes {
                Settings::setting_changed(
                    interface.signal_emitter(),
                    namespace,
                    key,
                    value.value(),
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

    #[cfg(feature = "app")]
    #[test]
    fn the_session_s_events_set_the_appearance_and_other_events_leave_it() {
        use cupertino_protocol_app::AppEvent;
        let options = cupertino_defaults::accessibility::DisplayOptions {
            increase_contrast: true,
            reduce_motion: true,
            ..Default::default()
        };
        let dark = Appearance::default().after(&AppEvent::Appearance { dark: true });
        assert_eq!(
            dark.values()[0],
            (NAMESPACE, "color-scheme", Setting::Number(1))
        );
        assert_eq!(
            dark.after(&AppEvent::DisplayOptions(options))
                .after(&AppEvent::ScrollBars { always: true })
                .after(&AppEvent::SteadyCaret { on: true }),
            Appearance {
                dark: true,
                increase_contrast: true,
                reduce_motion: true,
                always_show_scroll_bars: true,
            }
        );
    }

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
        assert!(matches(&patterns(&["org.gnome.*"]), INTERFACE_NAMESPACE));
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
        assert_eq!(
            light.changes(dark),
            [(NAMESPACE, "color-scheme", Setting::Number(1))]
        );
        assert_eq!(light.changes(light), []);
        let contrast = Appearance {
            increase_contrast: true,
            reduce_motion: true,
            ..dark
        };
        assert_eq!(
            dark.changes(contrast),
            [
                (NAMESPACE, "contrast", Setting::Number(1)),
                (NAMESPACE, "reduced-motion", Setting::Number(1))
            ]
        );
    }

    #[test]
    fn scroll_bars_shown_at_all_times_turn_overlay_scrolling_off() {
        let overlay = Appearance::default();
        assert!(overlay.values().contains(&(
            INTERFACE_NAMESPACE,
            "overlay-scrolling",
            Setting::Flag(true)
        )));
        let always = Appearance {
            always_show_scroll_bars: true,
            ..overlay
        };
        assert_eq!(
            overlay.changes(always),
            [(
                INTERFACE_NAMESPACE,
                "overlay-scrolling",
                Setting::Flag(false)
            )]
        );
    }
}
