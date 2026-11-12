//! The pages the window shows: each pane, and the pages some panes open from a navigation row. The page of a pane
//! with a header ([`has_header`]) starts with a header row (its tile, title and [`description`]); every
//! page shows its title in the toolbar row too. The sections a sub-page holds leave the pane's page, which
//! shows a row for each sub-page where the first of them stood. The back and forward history holds pages.

use cupertino_defaults::desktop_control::Service;

use crate::pane::{Enclosure, PaneId};
use crate::panes::Section;
use crate::search::Hit;

/// A page of the window: a pane, or one of its sub-pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Page {
    pub pane: PaneId,
    pub sub: Option<SubPage>,
}

impl Page {
    /// The pane's own page.
    pub fn pane(pane: PaneId) -> Page {
        Page { pane, sub: None }
    }

    /// The title the window and its toolbar show: the sub-page's, else the pane's.
    pub fn title(self, has_battery: Option<bool>) -> &'static str {
        match self.sub {
            Some(sub) => sub.title(),
            None => self.pane.title(has_battery),
        }
    }
}

/// A page a pane opens from one of its rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SubPage {
    Display,
    LoginScreen,
    ScreenReader,
    Pointer,
    Keyboard,
    HotCorners,
    ClipboardAccess,
    ScreenSharing,
    DesktopControl,
    ScreenCapture,
    About,
    SoftwareUpdate,
    Storage,
    DateTime,
    LanguageRegion,
    LoginItems,
}

impl SubPage {
    pub const ALL: [SubPage; 16] = [
        SubPage::Display,
        SubPage::LoginScreen,
        SubPage::ScreenReader,
        SubPage::Pointer,
        SubPage::Keyboard,
        SubPage::HotCorners,
        SubPage::ClipboardAccess,
        SubPage::ScreenSharing,
        SubPage::DesktopControl,
        SubPage::ScreenCapture,
        SubPage::About,
        SubPage::SoftwareUpdate,
        SubPage::Storage,
        SubPage::DateTime,
        SubPage::LanguageRegion,
        SubPage::LoginItems,
    ];

    /// The pane it opens from.
    pub fn pane(self) -> PaneId {
        match self {
            SubPage::Display
            | SubPage::LoginScreen
            | SubPage::ScreenReader
            | SubPage::Pointer
            | SubPage::Keyboard => PaneId::Accessibility,
            SubPage::HotCorners => PaneId::DesktopDock,
            SubPage::ClipboardAccess
            | SubPage::ScreenSharing
            | SubPage::DesktopControl
            | SubPage::ScreenCapture => PaneId::Privacy,
            SubPage::About
            | SubPage::SoftwareUpdate
            | SubPage::Storage
            | SubPage::DateTime
            | SubPage::LanguageRegion
            | SubPage::LoginItems => PaneId::General,
        }
    }

    /// Its title, which is also the header of the pane's section it holds.
    pub fn title(self) -> &'static str {
        match self {
            SubPage::Display => "Display",
            SubPage::LoginScreen => "Login Screen",
            SubPage::ScreenReader => "Screen Reader",
            SubPage::Pointer => "Pointer",
            SubPage::Keyboard => "Keyboard",
            SubPage::HotCorners => "Hot Corners",
            SubPage::ClipboardAccess => "Clipboard Access",
            SubPage::ScreenSharing => "Screen Sharing",
            SubPage::DesktopControl => crate::desktop_control::title(Service::Control),
            SubPage::ScreenCapture => crate::desktop_control::title(Service::Capture),
            SubPage::About => "About",
            SubPage::SoftwareUpdate => "Software Update",
            SubPage::Storage => "Storage",
            SubPage::DateTime => "Date & Time",
            SubPage::LanguageRegion => "Language & Region",
            SubPage::LoginItems => "Login Items",
        }
    }

    /// The `symbol.*` template symbol of its row's tile, and the tile's colour.
    pub fn symbol(self) -> &'static str {
        match self {
            SubPage::Display => "symbol.computer.fill",
            SubPage::LoginScreen => "symbol.lock-screen.fill",
            SubPage::ScreenReader => "symbol.speaker.high.fill",
            SubPage::Pointer => "symbol.mouse.fill",
            SubPage::Keyboard => "symbol.keyboard.fill",
            SubPage::HotCorners => "symbol.grid.fill",
            SubPage::ClipboardAccess => "symbol.document.fill",
            SubPage::ScreenSharing => "symbol.screen-mirroring.fill",
            SubPage::DesktopControl => "symbol.desktop.fill",
            SubPage::ScreenCapture => "symbol.photo.fill",
            SubPage::About => "symbol.computer.fill",
            SubPage::SoftwareUpdate => "symbol.download.fill",
            SubPage::Storage => "symbol.externaldrive.fill",
            SubPage::DateTime => "symbol.recents.fill",
            SubPage::LanguageRegion => "symbol.network.fill",
            SubPage::LoginItems => "symbol.applications.fill",
        }
    }

    pub fn enclosure(self) -> Enclosure {
        match self {
            SubPage::HotCorners => Enclosure::Black,
            SubPage::LoginScreen | SubPage::About | SubPage::Storage | SubPage::LoginItems => {
                Enclosure::Gray
            }
            SubPage::SoftwareUpdate => Enclosure::Red,
            _ => Enclosure::Blue,
        }
    }

    /// The group of rows it stands in on its pane's page: the pages of one group sit together, set apart from the
    /// next group.
    pub fn group(self) -> u8 {
        match self {
            SubPage::About | SubPage::SoftwareUpdate | SubPage::Storage => 0,
            SubPage::DateTime | SubPage::LanguageRegion | SubPage::LoginItems => 1,
            _ => 0,
        }
    }

    /// The pane's sub-page whose title is `title`.
    pub fn of(pane: PaneId, title: &str) -> Option<SubPage> {
        SubPage::ALL
            .into_iter()
            .find(|s| s.pane() == pane && s.title() == title)
    }
}

/// Every `symbol.*` name a sub-page's row draws.
pub fn symbols() -> Vec<&'static str> {
    SubPage::ALL.iter().map(|s| s.symbol()).collect()
}

/// Whether the pane's own page starts with a header row: its icon, title and description, and the pane's master
/// switch where it has one. Every other pane, and every sub-page, goes straight to its sections.
pub fn has_header(pane: PaneId) -> bool {
    matches!(
        pane,
        PaneId::Wifi
            | PaneId::Bluetooth
            | PaneId::Network
            | PaneId::Accessibility
            | PaneId::Privacy
    )
}

/// Whether the pane's page starts with a large card: its icon over its title and description.
pub fn has_card(pane: PaneId) -> bool {
    matches!(pane, PaneId::General)
}

/// The pane's one-line description on its header card.
pub fn description(pane: PaneId, has_battery: Option<bool>) -> &'static str {
    match pane {
        PaneId::Account => "Your name, picture and how you sign in.",
        PaneId::General => {
            "See what this computer is, and set its time, language and startup items."
        }
        PaneId::MenuBar => {
            "Choose what the menu bar shows, and how the clock and recent items look."
        }
        PaneId::Wifi => "Turn wireless networking on or off and join the networks around you.",
        PaneId::Bluetooth => "Pair and connect keyboards, headphones and other wireless devices.",
        PaneId::Network => "See this computer's network connections and how they are set up.",
        PaneId::Vpn => "Connect and disconnect the VPN connections the system holds.",
        PaneId::Search => "Choose what the launcher finds and which folders it leaves out.",
        PaneId::Users => "See who has an account on this computer.",
        PaneId::Battery if has_battery == Some(false) => {
            "Choose when the computer goes to sleep to save energy."
        }
        PaneId::Battery => "Watch the battery's charge and choose when the computer goes to sleep.",
        PaneId::Accessibility => {
            "Adjust the display, pointer, keyboard and screen reader to the way you work."
        }
        PaneId::Appearance => {
            "Choose a light or dark look, an accent colour and how scroll bars behave."
        }
        PaneId::ControlPanel => "Choose which control panel items also show in the menu bar.",
        PaneId::DesktopDock => {
            "Set the Dock's size, position and behaviour, and what the screen corners do."
        }
        PaneId::Displays => "Set the size and brightness of each display.",
        PaneId::Wallpaper => "Pick the picture behind your desktop, from ours or your own.",
        PaneId::Sound => "Choose where sound plays and how loud it is.",
        PaneId::Notifications => {
            "Choose what notifications show on the lock screen, for every app or one at a time."
        }
        PaneId::Focus => "Silence notifications for a while when you need to concentrate.",
        PaneId::LockScreen => {
            "Choose when the display turns off and when your password is asked for."
        }
        PaneId::Privacy => {
            "Choose which apps may use the clipboard, see your screen or control the desktop."
        }
        PaneId::Keyboard => "Set key repeat, the modifier keys and what the fn key does.",
        PaneId::Mouse => "Set the pointer's speed and the way scrolling moves.",
        PaneId::Trackpad => "Set the pointer's speed and the way scrolling moves on the trackpad.",
    }
}

/// What a page shows, top to bottom after its header card.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Section(Section),
    /// The rows that open these sub-pages, in one group.
    Links(Vec<SubPage>),
}

/// The sub-page each of the pane's `sections` belongs to: a section with a sub-page's title opens that page, and a
/// section without a header stays with the one before it.
fn owners(pane: PaneId, sections: &[Section]) -> Vec<Option<SubPage>> {
    let mut owner = None;
    sections
        .iter()
        .map(|section| {
            if let Some(header) = &section.header {
                owner = SubPage::of(pane, header);
            }
            owner
        })
        .collect()
}

/// The blocks of `page` from its pane's `sections`. A pane's page keeps the sections no sub-page holds and shows the
/// rows of its sub-pages where the first of them stood; a sub-page shows its own sections, the first without the
/// header the toolbar already shows.
pub fn blocks(page: Page, sections: Vec<Section>) -> Vec<Block> {
    let owners = owners(page.pane, &sections);
    match page.sub {
        None => {
            let mut subs: Vec<SubPage> = Vec::new();
            for sub in owners.iter().flatten() {
                if !subs.contains(sub) {
                    subs.push(*sub);
                }
            }
            let mut out = Vec::new();
            let mut linked: Vec<u8> = Vec::new();
            for (section, owner) in sections.into_iter().zip(&owners) {
                match owner {
                    None => out.push(Block::Section(section)),
                    Some(sub) if !linked.contains(&sub.group()) => {
                        linked.push(sub.group());
                        out.push(Block::Links(
                            subs.iter()
                                .copied()
                                .filter(|s| s.group() == sub.group())
                                .collect(),
                        ));
                    }
                    Some(_) => {}
                }
            }
            out
        }
        Some(sub) => sections
            .into_iter()
            .zip(&owners)
            .filter(|(_, owner)| **owner == Some(sub))
            .map(|(mut section, _)| {
                if section.header.as_deref() == Some(sub.title()) {
                    section.header = None;
                }
                Block::Section(section)
            })
            .collect(),
    }
}

/// The page a search result shows: the sub-page that holds its section, else its pane's page.
pub fn of_hit(hit: &Hit) -> Page {
    Page {
        pane: hit.pane,
        sub: hit
            .subtitle()
            .and_then(|title| SubPage::of(hit.pane, title)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::Control;
    use crate::panes::{Backend, ControlId, Row};
    use crate::search::Rank;

    fn section(header: Option<&str>, label: &str) -> Section {
        Section {
            header: header.map(str::to_owned),
            rows: vec![Row {
                id: ControlId::Reading,
                label: label.into(),
                control: Control::Text {
                    value: String::new(),
                },
                backend: Backend::Service("x"),
                error: None,
                disabled: None,
                icon: None,
                detail: None,
            }],
            footer: None,
        }
    }

    fn labels(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .map(|b| match b {
                Block::Section(s) => {
                    format!("{}:{}", s.header.as_deref().unwrap_or("-"), s.rows[0].label)
                }
                Block::Links(subs) => format!(
                    "links:{}",
                    subs.iter().map(|s| s.title()).collect::<Vec<_>>().join(",")
                ),
            })
            .collect()
    }

    #[test]
    fn a_pane_shows_rows_for_its_sub_pages_where_the_first_stood_and_keeps_the_rest() {
        let sections = vec![
            section(Some("Dock"), "size"),
            section(Some("Hot Corners"), "top left"),
            section(None, "more corners"),
        ];
        let page = Page::pane(PaneId::DesktopDock);
        assert_eq!(
            labels(&blocks(page, sections.clone())),
            ["Dock:size", "links:Hot Corners"]
        );
        let sub = Page {
            sub: Some(SubPage::HotCorners),
            ..page
        };
        assert_eq!(
            labels(&blocks(sub, sections)),
            ["-:top left", "-:more corners"],
            "a headerless section stays with the one before it, and the first loses the title the toolbar shows"
        );
    }

    #[test]
    fn a_sub_page_without_its_section_has_no_row() {
        let sections = vec![
            section(Some("Display"), "reduce motion"),
            section(Some("Screen Reader"), "reader"),
        ];
        assert_eq!(
            labels(&blocks(Page::pane(PaneId::Accessibility), sections)),
            ["links:Display,Screen Reader"],
            "no Login Screen row while the section is not offered"
        );
    }

    #[test]
    fn every_sub_page_names_a_section_of_its_pane_and_a_result_opens_it() {
        for sub in SubPage::ALL {
            assert_eq!(SubPage::of(sub.pane(), sub.title()), Some(sub));
            assert!(
                sub.symbol().starts_with("symbol.") && sub.symbol().ends_with(".fill"),
                "a tile wears a filled symbol: {}",
                sub.symbol()
            );
        }
        let hit = Hit {
            pane: PaneId::Accessibility,
            section: Some(3),
            rank: Rank::Section,
        };
        assert_eq!(hit.subtitle(), Some("Pointer"));
        assert_eq!(
            of_hit(&hit),
            Page {
                pane: PaneId::Accessibility,
                sub: Some(SubPage::Pointer)
            }
        );
        let top = Hit {
            section: None,
            ..hit
        };
        assert_eq!(of_hit(&top), Page::pane(PaneId::Accessibility));
    }

    #[test]
    fn every_pane_has_a_one_line_description() {
        for pane in crate::sidebar::rows(&[]) {
            let text = description(pane, Some(true));
            assert!(!text.is_empty() && text.ends_with('.'), "{pane:?}");
            assert!(text.len() < 90, "{pane:?} fits on one line: {text}");
        }
    }
}
