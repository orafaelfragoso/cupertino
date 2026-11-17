//! Finding cards and choosing the master among control descriptions, without a sound card.

use cupertino_alsa::{
    card_indexes, find_switch, select_master, Access, Card, ElementId, ElementInfo, ElementType,
    Error, IntegerRange, Interface, Master, MasterRank, OpenMode,
};
use std::path::PathBuf;

fn directory(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("alsa-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn element(numid: u32, name: &str, kind: ElementType) -> ElementInfo {
    ElementInfo {
        id: ElementId {
            numid,
            interface: Interface::Mixer,
            device: 0,
            subdevice: 0,
            name: name.to_owned(),
            index: 0,
        },
        kind,
        access: Access(Access::READ | Access::WRITE),
        count: 2,
        range: (kind == ElementType::Integer).then_some(IntegerRange {
            min: 0,
            max: 100,
            step: 1,
        }),
    }
}

fn volume(numid: u32, name: &str) -> ElementInfo {
    element(numid, name, ElementType::Integer)
}

#[test]
fn card_numbers_come_from_the_control_device_names_in_ascending_order() {
    let dir = directory("indexes");
    for name in [
        "controlC10",
        "controlC2",
        "controlC0",
        "pcmC0D0p",
        "controlCx",
        "timer",
    ] {
        std::fs::write(dir.join(name), b"").unwrap();
    }
    assert_eq!(card_indexes(&dir).unwrap(), vec![0, 2, 10]);
}

#[test]
fn a_directory_that_cannot_be_listed_is_named_in_the_error() {
    let dir = directory("missing").join("absent");
    let error = card_indexes(&dir).unwrap_err();
    assert!(matches!(&error, Error::ReadDir { dir: d, .. } if *d == dir));
    assert!(error.to_string().contains("absent"));
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn opening_a_card_without_a_control_device_says_which_card() {
    let dir = directory("nocard");
    let error = Card::open(&dir, 4, OpenMode::ReadOnly).unwrap_err();
    assert!(matches!(error, Error::NoSuchCard { card: 4 }));
    assert_eq!(error.to_string(), "sound card 4 has no control device");
}

#[test]
fn no_card_means_no_playback_volume() {
    let dir = directory("none");
    assert!(matches!(
        Master::open_default(&dir, OpenMode::ReadOnly),
        Err(Error::NoPlaybackVolume)
    ));
    assert!(matches!(
        Master::open_card(&dir, 0, OpenMode::ReadOnly),
        Err(Error::NoSuchCard { card: 0 })
    ));
}

#[test]
fn the_master_playback_volume_wins() {
    let elements = [
        volume(1, "Headphone Playback Volume"),
        volume(2, "PCM Playback Volume"),
        volume(3, "Master Playback Volume"),
    ];
    let (rank, info) = select_master(&elements).unwrap();
    assert_eq!(rank, MasterRank::Master);
    assert_eq!(info.id.numid, 3);
}

#[test]
fn pcm_is_next_and_then_the_first_playback_volume() {
    let elements = [
        volume(1, "Headphone Playback Volume"),
        volume(2, "PCM Playback Volume"),
    ];
    let (rank, info) = select_master(&elements).unwrap();
    assert_eq!((rank, info.id.numid), (MasterRank::Pcm, 2));

    let elements = [
        volume(1, "Headphone Playback Volume"),
        volume(2, "Speaker Playback Volume"),
    ];
    let (rank, info) = select_master(&elements).unwrap();
    assert_eq!((rank, info.id.numid), (MasterRank::FirstPlayback, 1));
}

#[test]
fn a_better_rank_sorts_lower() {
    assert!(MasterRank::Master < MasterRank::Pcm);
    assert!(MasterRank::Pcm < MasterRank::FirstPlayback);
}

#[test]
fn controls_that_cannot_be_turned_are_not_candidates() {
    let mut read_only = volume(1, "Master Playback Volume");
    read_only.access = Access(Access::READ);
    let mut inactive = volume(2, "Master Playback Volume");
    inactive.access = Access(Access::READ | Access::WRITE | Access::INACTIVE);
    let mut empty = volume(3, "Master Playback Volume");
    empty.count = 0;
    let mut flat = volume(4, "Master Playback Volume");
    flat.range = Some(IntegerRange {
        min: 5,
        max: 5,
        step: 1,
    });
    let mut no_range = volume(5, "Master Playback Volume");
    no_range.range = None;
    let mut other_device = volume(6, "Master Playback Volume");
    other_device.id.interface = Interface::Pcm;
    let capture = volume(7, "Capture Volume");
    let switch = element(8, "Master Playback Switch", ElementType::Boolean);
    assert!(select_master(&[
        read_only,
        inactive,
        empty,
        flat,
        no_range,
        other_device,
        capture,
        switch
    ])
    .is_none());
}

#[test]
fn a_card_without_controls_has_no_master() {
    assert!(select_master(&[]).is_none());
}

#[test]
fn the_mute_switch_has_the_volumes_name_index_and_device() {
    let master = volume(1, "Master Playback Volume");
    let elements = [
        element(2, "PCM Playback Switch", ElementType::Boolean),
        element(3, "Master Playback Switch", ElementType::Boolean),
        master.clone(),
    ];
    assert_eq!(find_switch(&elements, &master).unwrap().id.numid, 3);
}

#[test]
fn a_switch_of_another_index_device_or_kind_does_not_belong() {
    let master = volume(1, "Master Playback Volume");
    let mut other_index = element(2, "Master Playback Switch", ElementType::Boolean);
    other_index.id.index = 1;
    let mut other_device = element(3, "Master Playback Switch", ElementType::Boolean);
    other_device.id.device = 1;
    let integer = volume(4, "Master Playback Switch");
    let mut read_only = element(5, "Master Playback Switch", ElementType::Boolean);
    read_only.access = Access(Access::READ);
    assert!(find_switch(&[other_index, other_device, integer, read_only], &master).is_none());
}

#[test]
fn a_volume_whose_name_has_no_volume_word_has_no_switch() {
    let odd = volume(1, "Master Playback Level");
    let switch = element(2, "Master Playback Switch", ElementType::Boolean);
    assert!(find_switch(&[switch], &odd).is_none());
}
