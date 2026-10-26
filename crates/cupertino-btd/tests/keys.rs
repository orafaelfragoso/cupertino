//! The key file: round trip, permissions, atomic writes, and refusal of a file that is corrupt or
//! readable by others.

use std::fs;
use std::os::unix::fs::PermissionsExt;

use cupertino_bluetooth::{Address, AddressType, DeviceId, KeyValue, LinkKey, LinkKeyType, LongTermKey, LongTermKeyType};
use cupertino_btd::keys::{KeyFileError, KeyFileErrorKind, KeySet, KeyStore, KeyStoreError};

const ADAPTER: Address = Address::from_wire([0x01, 0xEE, 0xDD, 0xCC, 0xBB, 0xAA]);

fn id(last: u8, kind: AddressType) -> DeviceId {
    DeviceId { address: Address::from_wire([last, 0x55, 0x44, 0x33, 0x22, 0x11]), kind }
}

fn sample() -> KeySet {
    let mut keys = KeySet::default();
    keys.set_link_key(LinkKey { device: id(1, AddressType::BrEdr), key_type: LinkKeyType::AuthenticatedP256, value: KeyValue(*b"0123456789abcdef"), pin_length: 4 });
    assert!(keys.set_name(&id(1, AddressType::BrEdr), "Café Headset"));
    for (central, size) in [(true, 16), (false, 7)] {
        keys.set_long_term_key(LongTermKey {
            device: id(2, AddressType::LeRandom),
            key_type: LongTermKeyType::AuthenticatedP256,
            central,
            encryption_size: size,
            diversifier: 0xBEEF,
            random: [1, 2, 3, 4, 5, 6, 7, 8],
            value: KeyValue([0xA5; 16]),
        });
    }
    keys
}

/// "Café Headset" is `43 61 66 c3 a9 20 48 65 61 64 73 65 74` in UTF-8.
const SAMPLE_TEXT: &str = "cupertino-bluetooth-keys 1
device 11:22:33:44:55:01 bredr
name 436166c3a92048656164736574
link 08 04 30313233343536373839616263646566
device 11:22:33:44:55:02 le-random
ltk 03 1 16 beef 0102030405060708 a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5
ltk 03 0 7 beef 0102030405060708 a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5
";

#[test]
fn the_file_has_the_documented_text_form() {
    assert_eq!(sample().render(), SAMPLE_TEXT);
    assert_eq!(KeySet::parse(SAMPLE_TEXT).unwrap(), sample());
}

#[test]
fn saved_keys_load_back_unchanged_with_owner_only_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let store = KeyStore::new(dir.path().join("bluetooth"));
    store.save(ADAPTER, &sample()).unwrap();

    assert_eq!(store.load(ADAPTER).unwrap(), sample());
    let file = dir.path().join("bluetooth/AA-BB-CC-DD-EE-01.keys");
    assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(fs::metadata(dir.path().join("bluetooth")).unwrap().permissions().mode() & 0o777, 0o700);
    // The temporary file does not stay behind.
    assert_eq!(fs::read_dir(dir.path().join("bluetooth")).unwrap().count(), 1);

    // Saving again replaces the file and keeps the mode.
    let mut fewer = sample();
    assert!(fewer.remove(&id(2, AddressType::LeRandom)));
    store.save(ADAPTER, &fewer).unwrap();
    assert_eq!(store.load(ADAPTER).unwrap(), fewer);
    assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn an_adapter_without_a_file_has_no_bonds() {
    let dir = tempfile::tempdir().unwrap();
    assert!(KeyStore::new(dir.path().to_owned()).load(ADAPTER).unwrap().is_empty());
}

#[test]
fn a_corrupt_file_is_refused_and_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let store = KeyStore::new(dir.path().to_owned());
    let file = dir.path().join("AA-BB-CC-DD-EE-01.keys");
    let corrupt = SAMPLE_TEXT.replace("link 08 04", "link 08 xx");
    fs::write(&file, &corrupt).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();

    let error = store.load(ADAPTER).unwrap_err();
    let KeyStoreError::Corrupt { source, .. } = &error else { panic!("expected Corrupt, got {error}") };
    assert_eq!(source, &KeyFileError { line: 4, kind: KeyFileErrorKind::Field("PIN length") });
    assert_eq!(error.to_string(), format!("{}: line 4: invalid PIN length; the file was left as it is", file.display()));
    assert_eq!(fs::read_to_string(&file).unwrap(), corrupt);
}

#[test]
fn a_file_others_can_read_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = KeyStore::new(dir.path().to_owned());
    store.save(ADAPTER, &sample()).unwrap();
    let file = dir.path().join("AA-BB-CC-DD-EE-01.keys");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
    let error = store.load(ADAPTER).unwrap_err();
    assert_eq!(error.to_string(), format!("{}: mode 0640 lets others read the keys; fix it to 0600", file.display()));
}

#[test]
fn every_deviation_names_its_line() {
    let device = "device 11:22:33:44:55:01 bredr\n";
    let le = "device 11:22:33:44:55:02 le-public\n";
    let value = "00112233445566778899aabbccddeeff";
    let cases: Vec<(String, KeyFileError)> = vec![
        (String::new(), KeyFileError { line: 1, kind: KeyFileErrorKind::MissingHeader }),
        ("cupertino-bluetooth-keys 2\n".to_owned(), KeyFileError { line: 1, kind: KeyFileErrorKind::UnsupportedVersion("cupertino-bluetooth-keys 2".to_owned()) }),
        ("cupertino-bluetooth-keys 1\ndevice 11:22:33:44:55:01 bredr".to_owned(), KeyFileError { line: 2, kind: KeyFileErrorKind::MissingNewline }),
        ("cupertino-bluetooth-keys 1\nname 41\n".to_owned(), KeyFileError { line: 2, kind: KeyFileErrorKind::OutsideDevice }),
        (format!("cupertino-bluetooth-keys 1\n{device}{device}"), KeyFileError { line: 3, kind: KeyFileErrorKind::DuplicateDevice }),
        (format!("cupertino-bluetooth-keys 1\n{device}link 05 04 {value}\nlink 05 04 {value}\n"), KeyFileError { line: 4, kind: KeyFileErrorKind::Repeated("link") }),
        (format!("cupertino-bluetooth-keys 1\n{le}link 05 04 {value}\n"), KeyFileError { line: 3, kind: KeyFileErrorKind::WrongTransport("link") }),
        (format!("cupertino-bluetooth-keys 1\n{device}ltk 03 1 16 beef 0102030405060708 {value}\n"), KeyFileError { line: 3, kind: KeyFileErrorKind::WrongTransport("ltk") }),
        (format!("cupertino-bluetooth-keys 1\n{le}ltk 03 1 16 beef 0102030405060708 {value}\nltk 03 1 16 beef 0102030405060708 {value}\n"), KeyFileError { line: 4, kind: KeyFileErrorKind::Repeated("ltk") }),
        (format!("cupertino-bluetooth-keys 1\n{device}link 05 04 {}\n", &value[..30]), KeyFileError { line: 3, kind: KeyFileErrorKind::Field("value") }),
        (format!("cupertino-bluetooth-keys 1\n{device}name zz\n"), KeyFileError { line: 3, kind: KeyFileErrorKind::Field("name") }),
        (format!("cupertino-bluetooth-keys 1\n{device}name ff\n"), KeyFileError { line: 3, kind: KeyFileErrorKind::Field("name") }),
        ("cupertino-bluetooth-keys 1\ndevice 11:22:33:44:55 bredr\n".to_owned(), KeyFileError { line: 2, kind: KeyFileErrorKind::Field("address") }),
        ("cupertino-bluetooth-keys 1\ndevice 11:22:33:44:55:01 wifi\n".to_owned(), KeyFileError { line: 2, kind: KeyFileErrorKind::Field("address type") }),
        (format!("cupertino-bluetooth-keys 1\n{le}ltk 03 2 16 beef 0102030405060708 {value}\n"), KeyFileError { line: 3, kind: KeyFileErrorKind::Field("central") }),
        (format!("cupertino-bluetooth-keys 1\n{device}frobnicate\n"), KeyFileError { line: 3, kind: KeyFileErrorKind::UnknownLine("frobnicate".to_owned()) }),
    ];
    for (text, expected) in cases {
        assert_eq!(KeySet::parse(&text), Err(expected), "{text:?}");
    }
}
