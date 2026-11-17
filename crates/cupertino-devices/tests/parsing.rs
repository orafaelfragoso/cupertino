//! Capability masks, device numbers, uevent properties, kernel datagrams and the udev database.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use cupertino_devices::uevent::{devnode, devnum, parse_uevent_file};
use cupertino_devices::{
    parse_datagram, parse_udev_db, read_udev_properties, seat_name, Action, CapabilityMask, DevT,
    ParseError, Properties,
};

#[test]
fn a_mask_is_hexadecimal_words_highest_first_with_bits_past_them_clear() {
    // the last word is the lowest: bits 1 and 63; the first is bit 64
    let mask = CapabilityMask::parse("1 8000000000000002\n").unwrap();
    for (bit, expected) in [
        (0, false),
        (1, true),
        (2, false),
        (62, false),
        (63, true),
        (64, true),
        (65, false),
    ] {
        assert_eq!(mask.test(bit), expected, "{bit}");
    }
    let short = CapabilityMask::parse("ff").unwrap();
    for (bit, expected) in [
        (7, true),
        (8, false),
        (64, false),
        (767, false),
        (u32::MAX, false),
    ] {
        assert_eq!(short.test(bit), expected, "ff {bit}");
    }

    let ranged = CapabilityMask::parse("5 0000000000000f00").unwrap();
    for (range, any, count) in [
        (8..12, true, 4),
        (0..8, false, 0),
        (12..64, false, 0),
        (60..70, true, 2),
        (5..5, false, 0),
        (0..128, true, 6),
        (9..11, true, 2),
        (64..66, true, 1),
    ] {
        assert_eq!(ranged.any_in(range.clone()), any, "{range:?}");
        assert_eq!(ranged.count_in(range.clone()), count, "{range:?}");
    }

    for (text, low) in [
        ("ffffffff00000001", 1),
        ("123456789abcdef0", 0x9abc_def0),
        ("0", 0),
    ] {
        assert_eq!(
            CapabilityMask::parse(text).unwrap().low_word32(),
            low,
            "{text}"
        );
    }
}

#[test]
fn a_mask_with_no_words_or_a_bad_word_is_refused() {
    for (text, expected) in [
        ("", Some(ParseError::EmptyMask)),
        ("  \n", Some(ParseError::EmptyMask)),
        (
            "ff xyz",
            Some(ParseError::InvalidNumber {
                key: "capability word",
                value: "xyz".into(),
            }),
        ),
        ("1ffffffffffffffff", None),
    ] {
        let result = CapabilityMask::parse(text);
        match expected {
            Some(error) => assert_eq!(result, Err(error), "{text:?}"),
            None => assert!(result.is_err(), "a word wider than 64 bits"),
        }
    }
}

#[test]
fn device_numbers_round_trip_major_and_minor() {
    for (major, minor) in [(0, 0), (13, 64), (226, 128), (4095, 255), (0x1234, 0x12345)] {
        let dev = DevT::new(major, minor);
        assert_eq!((dev.major(), dev.minor()), (major, minor));
        assert_eq!(DevT::from_raw(dev.as_raw()), dev);
    }
}

#[test]
fn a_uevent_file_is_key_value_lines_split_at_the_first_equals_the_last_value_winning() {
    for (text, expected) in [
        (
            "MAJOR=13\nMINOR=64\nDEVNAME=input/event0\nEMPTY=\nA=b=c\n",
            Ok(vec![
                ("A", "b=c"),
                ("DEVNAME", "input/event0"),
                ("EMPTY", ""),
                ("MAJOR", "13"),
                ("MINOR", "64"),
            ]),
        ),
        ("", Ok(vec![])),
        ("A=1\nA=2\n", Ok(vec![("A", "2")])),
        (
            "A=1\nnoequals\n",
            Err(ParseError::MalformedProperty("noequals".into())),
        ),
        ("A=1\n\nB=2\n", Err(ParseError::EmptyProperty)),
    ] {
        let parsed = parse_uevent_file(text).map(|props| {
            let mut pairs: Vec<_> = props.into_iter().collect();
            pairs.sort();
            pairs
        });
        let expected = expected.map(|pairs| {
            pairs
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect::<Vec<_>>()
        });
        assert_eq!(parsed, expected, "{text:?}");
    }
}

fn props(pairs: &[(&str, &str)]) -> Properties {
    pairs
        .iter()
        .map(|&(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

#[test]
fn the_device_number_node_and_seat_come_from_the_properties() {
    for (pairs, expected) in [
        (
            &[("MAJOR", "13"), ("MINOR", "64")][..],
            Ok(Some(DevT::new(13, 64))),
        ),
        (&[("X", "1")], Ok(None)),
        (&[("MAJOR", "13")], Err(ParseError::IncompleteDevnum)),
        (&[("MINOR", "1")], Err(ParseError::IncompleteDevnum)),
        (
            &[("MAJOR", "x"), ("MINOR", "1")],
            Err(ParseError::InvalidNumber {
                key: "MAJOR",
                value: "x".into(),
            }),
        ),
        (
            &[("MAJOR", "1"), ("MINOR", "-1")],
            Err(ParseError::InvalidNumber {
                key: "MINOR",
                value: "-1".into(),
            }),
        ),
    ] {
        assert_eq!(devnum(&props(pairs)), expected, "{pairs:?}");
    }
    assert_eq!(
        devnode(&props(&[("DEVNAME", "dri/card0")])),
        Some(PathBuf::from("/dev/dri/card0"))
    );
    assert_eq!(devnode(&props(&[])), None);
    assert_eq!(seat_name(&props(&[])), "seat0");
    assert_eq!(seat_name(&props(&[("ID_SEAT", "seat1")])), "seat1");
}

fn datagram(parts: &[&str]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for part in parts {
        bytes.extend_from_slice(part.as_bytes());
        bytes.push(0);
    }
    bytes
}

#[test]
fn a_kernel_datagram_becomes_an_event_with_its_action_named_or_passed_through() {
    let bytes = datagram(&[
        "add@/devices/pci0000:00/drm/card1",
        "ACTION=add",
        "DEVPATH=/devices/pci0000:00/drm/card1",
        "SUBSYSTEM=drm",
        "MAJOR=226",
        "MINOR=1",
        "DEVNAME=dri/card1",
        "SEQNUM=4711",
    ]);
    let event = parse_datagram(&bytes).unwrap().unwrap();
    assert_eq!(event.action, Action::Added);
    assert_eq!(event.devpath, "/devices/pci0000:00/drm/card1");
    assert_eq!(event.subsystem, "drm");
    assert_eq!(event.devnode, Some(PathBuf::from("/dev/dri/card1")));
    assert_eq!(event.devnum, Some(DevT::new(226, 1)));
    assert_eq!(event.properties["SEQNUM"], "4711");
    assert_eq!(event.properties.len(), 7);

    // without a final nul and without a node
    let event = parse_datagram(b"remove@/devices/x\0SUBSYSTEM=input")
        .unwrap()
        .unwrap();
    assert_eq!(
        (event.action, event.devnode, event.devnum),
        (Action::Removed, None, None)
    );

    for (word, action) in [
        ("add", Action::Added),
        ("remove", Action::Removed),
        ("change", Action::Changed),
        ("bind", Action::Other("bind".into())),
        ("unbind", Action::Other("unbind".into())),
        ("online", Action::Other("online".into())),
        ("", Action::Other(String::new())),
    ] {
        let bytes = datagram(&[&format!("{word}@/devices/x"), "SUBSYSTEM=input"]);
        assert_eq!(
            parse_datagram(&bytes).unwrap().unwrap().action,
            action,
            "{word}"
        );
    }
}

#[test]
fn udevds_own_messages_are_not_events_and_a_bad_datagram_says_why() {
    let mut udevd = b"libudev\0".to_vec();
    udevd.extend_from_slice(&[0xfe, 0xed, 0xca, 0xfe, 0xff, 0xff]);
    for bytes in [udevd, b"libudev\0".to_vec()] {
        assert_eq!(parse_datagram(&bytes), Ok(None));
    }
    for (bytes, expected) in [
        (b"\xff\xfe\0".to_vec(), ParseError::NotUtf8),
        (b"".to_vec(), ParseError::MissingHeader),
        (b"add\0SUBSYSTEM=x\0".to_vec(), ParseError::MissingHeader),
        (
            datagram(&["add@/d", "DEVNAME=x"]),
            ParseError::MissingProperty("SUBSYSTEM"),
        ),
        (
            datagram(&["add@/d", "SUBSYSTEM=x", "junk"]),
            ParseError::MalformedProperty("junk".into()),
        ),
        (
            datagram(&["add@/d", "SUBSYSTEM=x", "", "A=b"]),
            ParseError::EmptyProperty,
        ),
        (
            datagram(&["add@/d", "SUBSYSTEM=x", "MAJOR=1"]),
            ParseError::IncompleteDevnum,
        ),
    ] {
        assert_eq!(
            parse_datagram(&bytes).map(drop),
            Err(expected.clone()),
            "{expected:?}"
        );
    }
}

#[test]
fn the_udev_database_keeps_e_lines_that_fit() {
    let long = "x".repeat(4097);
    let fits = "y".repeat(4096);
    for (text, expected) in [
        (
            "S:input/by-path/x\nI:1234\nE:ID_INPUT=1\nE:MOUSE_DPI=1000@125\nG:seat\nTAGS=:seat:\nE:EMPTY=\nE:NOEQUALS\nE:=nokey\n"
                .to_owned(),
            vec![("EMPTY", ""), ("ID_INPUT", "1"), ("MOUSE_DPI", "1000@125")],
        ),
        (String::new(), vec![]),
        (
            format!("E:BIG={long}\nE:{long}=1\nE:OK={fits}\n"),
            vec![("OK", fits.as_str())],
        ),
    ] {
        let mut props: Vec<_> = parse_udev_db(&text).into_iter().collect();
        props.sort();
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        assert_eq!(props, expected);
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("udev-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn udev_properties_are_read_from_the_node_file_and_never_from_a_link_directory_or_huge_file() {
    let dir = scratch("read");
    fs::write(
        dir.join("c13:67"),
        "I:1\nE:ID_INPUT_MOUSE=1\nE:MOUSE_DPI=800\n",
    )
    .unwrap();
    let props = read_udev_properties(&dir, DevT::new(13, 67));
    assert_eq!(props["MOUSE_DPI"], "800");
    assert_eq!(props.len(), 2);

    // a file that is not UTF-8 still gives its good lines
    fs::write(dir.join("c2:2"), b"E:NAME=caf\xe9\nE:OK=1\n").unwrap();
    let props = read_udev_properties(&dir, DevT::new(2, 2));
    assert_eq!(props["OK"], "1");
    assert!(props["NAME"].starts_with("caf"));

    fs::write(dir.join("real"), "E:A=1\n").unwrap();
    symlink(dir.join("real"), dir.join("c1:1")).unwrap();
    fs::create_dir(dir.join("c1:2")).unwrap();
    // a megabyte of padding pushes the one property past the bound
    let mut big = vec![b'#'; (1 << 20) + 10];
    big.extend_from_slice(b"\nE:LATE=1\n");
    fs::write(dir.join("c1:3"), big).unwrap();
    for (name, dir, dev) in [
        ("no record", dir.clone(), DevT::new(13, 1)),
        ("no directory", dir.join("nowhere"), DevT::new(13, 1)),
        ("a link", dir.clone(), DevT::new(1, 1)),
        ("a directory", dir.clone(), DevT::new(1, 2)),
        ("too large", dir.clone(), DevT::new(1, 3)),
    ] {
        assert!(read_udev_properties(&dir, dev).is_empty(), "{name}");
    }
}
