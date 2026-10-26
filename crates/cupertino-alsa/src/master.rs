//! The playback master volume of the machine: which control it is and how to read and change it.
//!
//! **Selection rule.** Among the mixer-device integer controls that can be read and written, are active, and have a
//! range, whose name ends in `Playback Volume`:
//! 1. `Master Playback Volume`;
//! 2. else `PCM Playback Volume`;
//! 3. else the first playback volume in the card's control order.
//!
//! Across cards the lowest rank wins, and the lowest card number among equals. The mute switch is the boolean
//! control with the same name but `Switch` in place of `Volume`, same index and device; a master without one cannot
//! be muted.

use std::path::Path;

use crate::abi::{ElementInfo, ElementType, Interface};
use crate::card::{card_indexes, Card, OpenMode};
use crate::error::{Error, Result};
use crate::volume::VolumeRange;

const VOLUME_SUFFIX: &str = "Playback Volume";
const MASTER_NAME: &str = "Master Playback Volume";
const PCM_NAME: &str = "PCM Playback Volume";

/// How well a control fits as the master; lower is better.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MasterRank {
    /// `Master Playback Volume`.
    Master,
    /// `PCM Playback Volume`.
    Pcm,
    /// The first playback volume of the card.
    FirstPlayback,
}

/// Whether `info` is a volume the user can turn: a usable playback volume.
fn is_playback_volume(info: &ElementInfo) -> bool {
    info.id.interface == Interface::Mixer
        && info.kind == ElementType::Integer
        && info.count > 0
        && info.access.readable()
        && info.access.writable()
        && !info.access.inactive()
        && info.range.is_some_and(|r| r.min < r.max)
        && info.id.name.ends_with(VOLUME_SUFFIX)
}

/// The master volume among the controls of one card, by the rule in the module documentation.
pub fn select_master(elements: &[ElementInfo]) -> Option<(MasterRank, &ElementInfo)> {
    let usable = || elements.iter().filter(|info| is_playback_volume(info));
    usable()
        .find(|info| info.id.name == MASTER_NAME)
        .map(|info| (MasterRank::Master, info))
        .or_else(|| usable().find(|info| info.id.name == PCM_NAME).map(|info| (MasterRank::Pcm, info)))
        .or_else(|| usable().next().map(|info| (MasterRank::FirstPlayback, info)))
}

/// The mute switch that belongs to `volume`, if the card has one.
pub fn find_switch<'a>(elements: &'a [ElementInfo], volume: &ElementInfo) -> Option<&'a ElementInfo> {
    let stem = volume.id.name.strip_suffix("Volume")?;
    let name = format!("{stem}Switch");
    elements.iter().find(|info| {
        info.id.interface == Interface::Mixer
            && info.kind == ElementType::Boolean
            && info.count > 0
            && info.access.readable()
            && info.access.writable()
            && info.id.name == name
            && info.id.index == volume.id.index
            && info.id.device == volume.id.device
    })
}

/// The playback master volume of a card, with its mute switch when it has one.
#[derive(Debug)]
pub struct Master {
    card: Card,
    volume: ElementInfo,
    switch: Option<ElementInfo>,
    range: VolumeRange,
}

impl Master {
    /// The best master among all cards below `dir` (see the module documentation); fails with
    /// [`Error::NoPlaybackVolume`] when no card has a playback volume.
    pub fn open_default(dir: &Path, mode: OpenMode) -> Result<Self> {
        let mut best: Option<(MasterRank, Card, Vec<ElementInfo>, ElementInfo)> = None;
        for index in card_indexes(dir)? {
            let card = Card::open(dir, index, mode)?;
            let elements = card.elements()?;
            let Some((rank, info)) = select_master(&elements) else { continue };
            // ascending card numbers: a later card replaces only a strictly better rank
            if best.as_ref().is_none_or(|(best_rank, ..)| rank < *best_rank) {
                let info = info.clone();
                best = Some((rank, card, elements, info));
            }
        }
        let (_, card, elements, volume) = best.ok_or(Error::NoPlaybackVolume)?;
        Self::from_elements(card, &elements, volume)
    }

    /// The best master of card `index` below `dir`; fails with [`Error::NoPlaybackVolume`] when it has none.
    pub fn open_card(dir: &Path, index: u32, mode: OpenMode) -> Result<Self> {
        let card = Card::open(dir, index, mode)?;
        let elements = card.elements()?;
        let volume = select_master(&elements).ok_or(Error::NoPlaybackVolume)?.1.clone();
        Self::from_elements(card, &elements, volume)
    }

    fn from_elements(card: Card, elements: &[ElementInfo], volume: ElementInfo) -> Result<Self> {
        let switch = find_switch(elements, &volume).cloned();
        let bounds = volume.range.expect("a selected master has a range");
        let range = VolumeRange::new(bounds.min, bounds.max, card.db_curve(&volume)?)?;
        Ok(Self { card, volume, switch, range })
    }

    /// The card the master is on.
    pub fn card(&self) -> &Card {
        &self.card
    }

    /// The master volume control.
    pub fn volume_info(&self) -> &ElementInfo {
        &self.volume
    }

    /// The mute switch, if the card has one for the master.
    pub fn switch_info(&self) -> Option<&ElementInfo> {
        self.switch.as_ref()
    }

    /// The raw value of the first channel.
    fn raw(&self) -> Result<i64> {
        self.card.read_values(&self.volume)?.first().copied().ok_or(Error::NotAnIntegerControl)
    }

    /// The level, 0 to 1, of the first channel.
    pub fn level(&self) -> Result<f64> {
        Ok(self.range.level(self.raw()?))
    }

    /// Set every channel to the raw value nearest to `level`; returns the level reached.
    pub fn set_level(&self, level: f64) -> Result<f64> {
        self.write_raw(self.range.raw(level))
    }

    /// Move one press of size `step` (a level difference) in `direction`; returns the level reached.
    pub fn step(&self, direction: i32, step: f64) -> Result<f64> {
        self.write_raw(self.range.step(self.raw()?, direction, step))
    }

    fn write_raw(&self, raw: i64) -> Result<f64> {
        self.card.write_values(&self.volume, &vec![raw; self.volume.count as usize])?;
        Ok(self.range.level(raw))
    }

    /// Whether the master is muted; `None` when it has no mute switch.
    pub fn muted(&self) -> Result<Option<bool>> {
        let Some(switch) = &self.switch else { return Ok(None) };
        // a switch value of 0 is "off": the channel is silent
        let values = self.card.read_values(switch)?;
        Ok(Some(values.first().copied().ok_or(Error::NotAnIntegerControl)? == 0))
    }

    /// Mute or unmute every channel; fails with [`Error::NoMuteSwitch`] when the master has no switch.
    pub fn set_muted(&self, muted: bool) -> Result<()> {
        let switch = self.switch.as_ref().ok_or(Error::NoMuteSwitch)?;
        self.card.write_values(switch, &vec![i64::from(!muted); switch.count as usize])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::{Access, ElementId, IntegerRange};

    fn control(numid: u32, name: &str, index: u32, kind: ElementType, access: u32) -> ElementInfo {
        let range = (kind == ElementType::Integer).then_some(IntegerRange { min: 0, max: 64, step: 1 });
        ElementInfo { id: ElementId { numid, interface: Interface::Mixer, device: 0, subdevice: 0, name: name.into(), index }, kind, access: Access(access), count: 2, range }
    }

    const RW: u32 = Access::READ | Access::WRITE;

    #[test]
    fn master_playback_volume_wins_over_everything() {
        let elements = vec![
            control(1, "Speaker Playback Volume", 0, ElementType::Integer, RW),
            control(2, "PCM Playback Volume", 0, ElementType::Integer, RW),
            control(3, "Master Playback Volume", 0, ElementType::Integer, RW),
            control(4, "Master Playback Switch", 0, ElementType::Boolean, RW),
        ];
        let (rank, info) = select_master(&elements).unwrap();
        assert_eq!((rank, info.id.numid), (MasterRank::Master, 3));
        assert_eq!(find_switch(&elements, info).unwrap().id.numid, 4);
    }

    #[test]
    fn pcm_is_next_and_then_the_first_playback_volume() {
        let mut elements = vec![control(1, "Speaker Playback Volume", 0, ElementType::Integer, RW), control(2, "PCM Playback Volume", 0, ElementType::Integer, RW)];
        assert_eq!(select_master(&elements).map(|(rank, info)| (rank, info.id.numid)), Some((MasterRank::Pcm, 2)));
        elements.remove(1);
        elements.push(control(5, "Headphone Playback Volume", 0, ElementType::Integer, RW));
        assert_eq!(select_master(&elements).map(|(rank, info)| (rank, info.id.numid)), Some((MasterRank::FirstPlayback, 1)));
    }

    #[test]
    fn capture_controls_and_unusable_playback_controls_are_skipped() {
        let elements = vec![
            control(1, "Capture Volume", 0, ElementType::Integer, RW),
            control(2, "Master Playback Volume", 0, ElementType::Integer, Access::READ),
            control(3, "Master Playback Volume", 1, ElementType::Integer, RW | Access::INACTIVE),
            control(4, "Master Playback Switch", 0, ElementType::Boolean, RW),
        ];
        assert_eq!(select_master(&elements), None);
    }

    #[test]
    fn a_switch_of_another_index_is_not_the_masters() {
        let elements = vec![control(1, "PCM Playback Volume", 0, ElementType::Integer, RW), control(2, "PCM Playback Switch", 1, ElementType::Boolean, RW)];
        let (_, info) = select_master(&elements).unwrap();
        assert_eq!(find_switch(&elements, info), None);
    }
}
