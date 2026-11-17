//! Capability masks, device numbers, uevent properties, kernel datagrams, the udev database and error text.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use cupertino_devices::uevent::{devnode, devnum, parse_uevent_file};
use cupertino_devices::{
    parse_datagram, parse_udev_db, read_udev_properties, seat_name, Action, CapabilityMask, DevT,
    Error, ParseError, Properties, SEAT0, UDEV_DATA_DIR,
};

#[test]
fn a_mask_is_hexadecimal_words_highest_first() {
    let mask = CapabilityMask::parse("1 8000000000000002\n").unwrap();
    // the last word is the lowest: bits 1 and 63; the first is bit 64
    assert!(mask.test(1) && mask.test(63) && mask.test(64));
    assert!(!mask.test(0) && !mask.test(2) && !mask.test(65) && !mask.test(62));
}

#[test]
fn bits_past_the_printed_words_are_clear() {
    let mask = CapabilityMask::parse("ff").unwrap();
    assert!(mask.test(7));
    assert!(!mask.test(8) && !mask.test(64) && !mask.test(767) && !mask.test(u32::MAX));
}

#[test]
fn a_mask_answers_for_ranges() {
    let mask = CapabilityMask::parse("5 0000000000000f00").unwrap();
    assert!(mask.any_in(8..12));
    assert!(!mask.any_in(0..8));
    assert!(!mask.any_in(12..64));
    assert!(mask.any_in(60..70));
    assert!(!mask.any_in(5..5));
    assert_eq!(mask.count_in(0..128), 6);
    assert_eq!(mask.count_in(9..11), 2);
    assert_eq!(mask.count_in(64..66), 1);
}

#[test]
fn the_low_word_is_the_first_32_keys() {
    assert_eq!(
        CapabilityMask::parse("ffffffff00000001")
            .unwrap()
            .low_word32(),
        1
    );
    assert_eq!(
        CapabilityMask::parse("123456789abcdef0")
            .unwrap()
            .low_word32(),
        0x9abc_def0
    );
    assert_eq!(CapabilityMask::parse("0").unwrap().low_word32(), 0);
}

#[test]
fn a_mask_with_no_words_or_a_bad_word_is_refused() {
    assert_eq!(CapabilityMask::parse(""), Err(ParseError::EmptyMask));
    assert_eq!(CapabilityMask::parse("  \n"), Err(ParseError::EmptyMask));
    assert_eq!(
        CapabilityMask::parse("ff xyz"),
        Err(ParseError::InvalidNumber {
            key: "capability word",
            value: "xyz".into()
        })
    );
    // a word wider than 64 bits
    assert!(CapabilityMask::parse("1ffffffffffffffff").is_err());
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
fn a_device_number_prints_as_major_colon_minor() {
    assert_eq!(DevT::new(13, 67).to_string(), "13:67");
    assert_eq!(DevT::new(226, 0).to_string(), "226:0");
}

#[test]
fn device_numbers_order_and_hash_by_their_raw_value() {
    assert!(DevT::new(13, 64) < DevT::new(13, 65));
    assert!(DevT::new(13, 65) < DevT::new(14, 0));
    let mut set = std::collections::HashSet::new();
    set.insert(DevT::new(1, 2));
    assert!(set.contains(&DevT::new(1, 2)) && !set.contains(&DevT::new(2, 1)));
}

#[test]
fn a_uevent_file_is_key_value_lines() {
    let props =
        parse_uevent_file("MAJOR=13\nMINOR=64\nDEVNAME=input/event0\nEMPTY=\nA=b=c\n").unwrap();
    assert_eq!(props.len(), 5);
    assert_eq!(props["DEVNAME"], "input/event0");
    assert_eq!(props["EMPTY"], "");
    // only the first `=` splits
    assert_eq!(props["A"], "b=c");
    assert!(parse_uevent_file("").unwrap().is_empty());
}

#[test]
fn a_later_property_replaces_an_earlier_one() {
    let props = parse_uevent_file("A=1\nA=2\n").unwrap();
    assert_eq!(props["A"], "2");
}

#[test]
fn a_uevent_line_without_an_equals_sign_or_a_blank_line_is_refused() {
    assert_eq!(
        parse_uevent_file("A=1\nnoequals\n"),
        Err(ParseError::MalformedProperty("noequals".into()))
    );
    assert_eq!(
        parse_uevent_file("A=1\n\nB=2\n"),
        Err(ParseError::EmptyProperty)
    );
}

fn props(pairs: &[(&str, &str)]) -> Properties {
    pairs
        .iter()
        .map(|&(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

#[test]
fn the_device_number_comes_from_major_and_minor() {
    assert_eq!(
        devnum(&props(&[("MAJOR", "13"), ("MINOR", "64")])),
        Ok(Some(DevT::new(13, 64)))
    );
    assert_eq!(devnum(&props(&[("X", "1")])), Ok(None));
    assert_eq!(
        devnum(&props(&[("MAJOR", "13")])),
        Err(ParseError::IncompleteDevnum)
    );
    assert_eq!(
        devnum(&props(&[("MINOR", "1")])),
        Err(ParseError::IncompleteDevnum)
    );
    assert_eq!(
        devnum(&props(&[("MAJOR", "x"), ("MINOR", "1")])),
        Err(ParseError::InvalidNumber {
            key: "MAJOR",
            value: "x".into()
        })
    );
    assert_eq!(
        devnum(&props(&[("MAJOR", "1"), ("MINOR", "-1")])),
        Err(ParseError::InvalidNumber {
            key: "MINOR",
            value: "-1".into()
        })
    );
}

#[test]
fn the_node_is_devname_under_dev() {
    assert_eq!(
        devnode(&props(&[("DEVNAME", "dri/card0")])),
        Some(PathBuf::from("/dev/dri/card0"))
    );
    assert_eq!(devnode(&props(&[])), None);
}

#[test]
fn the_seat_defaults_to_seat0() {
    assert_eq!(SEAT0, "seat0");
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
fn a_kernel_datagram_becomes_an_event() {
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
}

#[test]
fn the_actions_are_named_and_others_pass_through() {
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
fn a_datagram_without_a_final_nul_and_without_a_node_parses() {
    let event = parse_datagram(b"remove@/devices/x\0SUBSYSTEM=input")
        .unwrap()
        .unwrap();
    assert_eq!(event.action, Action::Removed);
    assert_eq!(event.devnode, None);
    assert_eq!(event.devnum, None);
}

#[test]
fn udevds_own_messages_are_not_events() {
    let mut bytes = b"libudev\0".to_vec();
    bytes.extend_from_slice(&[0xfe, 0xed, 0xca, 0xfe, 0xff, 0xff]);
    assert_eq!(parse_datagram(&bytes), Ok(None));
    assert_eq!(parse_datagram(b"libudev\0"), Ok(None));
}

#[test]
fn a_datagram_that_does_not_parse_says_why() {
    assert_eq!(parse_datagram(b"\xff\xfe\0"), Err(ParseError::NotUtf8));
    assert_eq!(parse_datagram(b""), Err(ParseError::MissingHeader));
    assert_eq!(
        parse_datagram(b"add\0SUBSYSTEM=x\0"),
        Err(ParseError::MissingHeader)
    );
    assert_eq!(
        parse_datagram(&datagram(&["add@/d", "DEVNAME=x"])),
        Err(ParseError::MissingProperty("SUBSYSTEM"))
    );
    assert_eq!(
        parse_datagram(&datagram(&["add@/d", "SUBSYSTEM=x", "junk"])),
        Err(ParseError::MalformedProperty("junk".into()))
    );
    assert_eq!(
        parse_datagram(&datagram(&["add@/d", "SUBSYSTEM=x", "", "A=b"])),
        Err(ParseError::EmptyProperty)
    );
    assert_eq!(
        parse_datagram(&datagram(&["add@/d", "SUBSYSTEM=x", "MAJOR=1"])),
        Err(ParseError::IncompleteDevnum)
    );
}

#[test]
fn the_udev_database_keeps_e_lines_only() {
    let text = "S:input/by-path/x\nI:1234\nE:ID_INPUT=1\nE:MOUSE_DPI=1000@125\nG:seat\nTAGS=:seat:\nE:EMPTY=\nE:NOEQUALS\nE:=nokey\n";
    let props = parse_udev_db(text);
    assert_eq!(props.len(), 3);
    assert_eq!(props["ID_INPUT"], "1");
    assert_eq!(props["MOUSE_DPI"], "1000@125");
    assert_eq!(props["EMPTY"], "");
    assert!(parse_udev_db("").is_empty());
}

#[test]
fn the_udev_database_skips_entries_that_are_too_long() {
    let long = "x".repeat(4097);
    let fits = "y".repeat(4096);
    let text = format!("E:BIG={long}\nE:{long}=1\nE:OK={fits}\n");
    let props = parse_udev_db(&text);
    assert_eq!(props.keys().collect::<Vec<_>>(), ["OK"]);
    assert_eq!(props["OK"].len(), 4096);
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("udev-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn udev_properties_are_read_from_the_node_file() {
    let dir = scratch("read");
    fs::write(
        dir.join("c13:67"),
        "I:1\nE:ID_INPUT_MOUSE=1\nE:MOUSE_DPI=800\n",
    )
    .unwrap();
    let props = read_udev_properties(&dir, DevT::new(13, 67));
    assert_eq!(props["MOUSE_DPI"], "800");
    assert_eq!(props.len(), 2);
    assert!(UDEV_DATA_DIR.starts_with("/run/udev"));
}

#[test]
fn a_node_udevd_has_no_record_of_has_no_properties() {
    let dir = scratch("missing");
    assert!(read_udev_properties(&dir, DevT::new(13, 1)).is_empty());
    assert!(read_udev_properties(&dir.join("nowhere"), DevT::new(13, 1)).is_empty());
}

#[test]
fn a_database_entry_that_is_a_link_a_directory_or_too_large_is_not_read() {
    let dir = scratch("hostile");
    fs::write(dir.join("real"), "E:A=1\n").unwrap();
    symlink(dir.join("real"), dir.join("c1:1")).unwrap();
    assert!(read_udev_properties(&dir, DevT::new(1, 1)).is_empty());

    fs::create_dir(dir.join("c1:2")).unwrap();
    assert!(read_udev_properties(&dir, DevT::new(1, 2)).is_empty());

    // a megabyte of padding pushes the one property past the bound
    let mut big = vec![b'#'; (1 << 20) + 10];
    big.extend_from_slice(b"\nE:LATE=1\n");
    fs::write(dir.join("c1:3"), big).unwrap();
    assert!(read_udev_properties(&dir, DevT::new(1, 3)).is_empty());
}

#[test]
fn a_database_file_that_is_not_utf8_still_gives_its_good_lines() {
    let dir = scratch("lossy");
    fs::write(dir.join("c2:2"), b"E:NAME=caf\xe9\nE:OK=1\n").unwrap();
    let props = read_udev_properties(&dir, DevT::new(2, 2));
    assert_eq!(props["OK"], "1");
    assert!(props["NAME"].starts_with("caf"));
}

#[test]
fn errors_say_where_and_why() {
    let parse = Error::Parse {
        origin: "/sys/x/uevent".into(),
        source: ParseError::IncompleteDevnum,
    };
    assert_eq!(
        parse.to_string(),
        "/sys/x/uevent: MAJOR and MINOR must come together"
    );
    let io = Error::Io {
        path: PathBuf::from("/sys/missing"),
        source: std::io::Error::from(std::io::ErrorKind::NotFound),
    };
    assert!(io.to_string().starts_with("/sys/missing: "));
    let syscall = Error::Syscall {
        call: "bind",
        source: rustix::io::Errno::PERM,
    };
    assert!(syscall.to_string().starts_with("bind: "));
    for error in [&parse, &io, &syscall] {
        assert!(std::error::Error::source(error).is_some());
    }
}

#[test]
fn every_parse_error_has_its_own_message() {
    let all = [
        ParseError::MalformedProperty("x".into()),
        ParseError::EmptyProperty,
        ParseError::NotUtf8,
        ParseError::MissingHeader,
        ParseError::MissingProperty("SUBSYSTEM"),
        ParseError::InvalidNumber {
            key: "MAJOR",
            value: "q".into(),
        },
        ParseError::IncompleteDevnum,
        ParseError::EmptyMask,
        ParseError::Truncated,
    ];
    let messages: std::collections::HashSet<_> = all.iter().map(ToString::to_string).collect();
    assert_eq!(messages.len(), all.len());
    assert!(all[0].to_string().contains('x') && all[4].to_string().contains("SUBSYSTEM"));
    assert!(all[5].to_string().contains("MAJOR") && all[5].to_string().contains('q'));
}
