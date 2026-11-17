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

fn switch(numid: u32, name: &str) -> ElementInfo {
    element(numid, name, ElementType::Boolean)
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
fn a_missing_directory_or_card_is_reported_as_what_is_missing() {
    let absent = directory("missing").join("absent");
    let error = card_indexes(&absent).unwrap_err();
    assert!(
        matches!(&error, Error::ReadDir { dir, .. } if *dir == absent),
        "{error:?}"
    );

    let dir = directory("nocard");
    for (name, result) in [
        (
            "a card",
            Card::open(&dir, 4, OpenMode::ReadOnly).map(drop),
        ),
        (
            "the default master",
            Master::open_default(&dir, OpenMode::ReadOnly).map(drop),
        ),
        (
            "a card's master",
            Master::open_card(&dir, 0, OpenMode::ReadOnly).map(drop),
        ),
    ] {
        let expected = match (name, &result) {
            ("a card", Err(Error::NoSuchCard { card: 4 })) => true,
            ("the default master", Err(Error::NoPlaybackVolume)) => true,
            ("a card's master", Err(Error::NoSuchCard { card: 0 })) => true,
            _ => false,
        };
        assert!(expected, "{name}: {result:?}");
    }
}

#[test]
fn the_master_is_the_master_volume_then_pcm_then_the_first_playback_volume_that_can_be_turned() {
    let mut read_only = volume(11, "Master Playback Volume");
    read_only.access = Access(Access::READ);
    let mut inactive = volume(12, "Master Playback Volume");
    inactive.access = Access(Access::READ | Access::WRITE | Access::INACTIVE);
    let mut empty = volume(13, "Master Playback Volume");
    empty.count = 0;
    let mut flat = volume(14, "Master Playback Volume");
    flat.range = Some(IntegerRange {
        min: 5,
        max: 5,
        step: 1,
    });
    let mut no_range = volume(15, "Master Playback Volume");
    no_range.range = None;
    let mut other_device = volume(16, "Master Playback Volume");
    other_device.id.interface = Interface::Pcm;
    let not_candidates = vec![
        read_only,
        inactive,
        empty,
        flat,
        no_range,
        other_device,
        volume(17, "Capture Volume"),
        switch(18, "Master Playback Switch"),
    ];

    for (name, elements, expected) in [
        (
            "master",
            vec![
                volume(1, "Headphone Playback Volume"),
                volume(2, "PCM Playback Volume"),
                volume(3, "Master Playback Volume"),
            ],
            Some((MasterRank::Master, 3)),
        ),
        (
            "pcm",
            vec![
                volume(1, "Headphone Playback Volume"),
                volume(2, "PCM Playback Volume"),
            ],
            Some((MasterRank::Pcm, 2)),
        ),
        (
            "the first playback volume",
            vec![
                volume(1, "Headphone Playback Volume"),
                volume(2, "Speaker Playback Volume"),
            ],
            Some((MasterRank::FirstPlayback, 1)),
        ),
        ("controls that cannot be turned", not_candidates, None),
        ("no controls", vec![], None),
    ] {
        assert_eq!(
            select_master(&elements).map(|(rank, info)| (rank, info.id.numid)),
            expected,
            "{name}"
        );
    }
}

#[test]
fn the_mute_switch_is_the_writable_switch_of_the_volumes_name_index_and_device() {
    let master = volume(1, "Master Playback Volume");
    let mut other_index = switch(4, "Master Playback Switch");
    other_index.id.index = 1;
    let mut other_device = switch(5, "Master Playback Switch");
    other_device.id.device = 1;
    let mut read_only = switch(6, "Master Playback Switch");
    read_only.access = Access(Access::READ);
    for (name, volume_info, elements, expected) in [
        (
            "the matching switch",
            master.clone(),
            vec![
                switch(2, "PCM Playback Switch"),
                switch(3, "Master Playback Switch"),
                master.clone(),
            ],
            Some(3),
        ),
        (
            "another index, device or kind, or read-only",
            master,
            vec![
                other_index,
                other_device,
                volume(7, "Master Playback Switch"),
                read_only,
            ],
            None,
        ),
        (
            "a volume without the volume word",
            volume(1, "Master Playback Level"),
            vec![switch(2, "Master Playback Switch")],
            None,
        ),
    ] {
        assert_eq!(
            find_switch(&elements, &volume_info).map(|info| info.id.numid),
            expected,
            "{name}"
        );
    }
}
