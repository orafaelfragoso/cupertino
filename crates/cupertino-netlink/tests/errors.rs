//! What the errors say and what they chain to.

use cupertino_netlink::{Error, ParseError};
use rustix::io::Errno;
use std::error::Error as _;
use std::path::PathBuf;

#[test]
fn parse_errors_say_what_ran_out() {
    assert_eq!(
        ParseError::Truncated {
            what: "netlink header",
            needed: 16,
            have: 3
        }
        .to_string(),
        "netlink header needs 16 bytes, 3 left"
    );
    assert_eq!(
        ParseError::MissingAttribute {
            message: "link",
            attribute: "IFLA_IFNAME"
        }
        .to_string(),
        "link without IFLA_IFNAME"
    );
    assert_eq!(
        ParseError::RfkillSize { actual: 3 }.to_string(),
        "rfkill event of 3 bytes, expected 8"
    );
    assert_eq!(
        ParseError::AttributeSize {
            attribute: 4,
            expected: 4,
            actual: 2
        }
        .to_string(),
        "attribute 4 has 2 bytes, expected 4"
    );
}

#[test]
fn a_decode_failure_names_its_origin_and_chains_to_the_parse_error() {
    let error = Error::Parse {
        origin: "RTM_GETLINK",
        source: ParseError::AttributeNotUtf8 { attribute: 3 },
    };
    assert_eq!(
        error.to_string(),
        "RTM_GETLINK: attribute 3 is not valid UTF-8"
    );
    assert!(error.source().is_some());
}

#[test]
fn system_call_failures_name_the_call_and_chain_to_the_errno() {
    let error = Error::Syscall {
        call: "bind",
        source: Errno::PERM,
    };
    assert!(error.to_string().starts_with("bind: "));
    assert!(error.source().is_some());
    let kernel = Error::Kernel {
        request: "RTM_NEWLINK",
        errno: Errno::NODEV,
    };
    assert!(kernel
        .to_string()
        .starts_with("the kernel refused RTM_NEWLINK: "));
    assert!(kernel.source().is_some());
}

#[test]
fn a_device_failure_names_the_call_and_the_path() {
    let error = Error::Device {
        path: PathBuf::from("/dev/rfkill"),
        call: "open",
        source: Errno::ACCESS,
    };
    assert!(error.to_string().starts_with("open /dev/rfkill: "));
    assert!(error.source().is_some());
}

#[test]
fn protocol_failures_carry_no_underlying_error() {
    let sequence = Error::UnexpectedSequence {
        expected: 4,
        actual: 9,
    };
    assert_eq!(sequence.to_string(), "reply with sequence 9, expected 4");
    assert!(sequence.source().is_none());
    let group = Error::GroupOutOfRange {
        name: "scan".to_owned(),
        id: 40,
    };
    assert!(group.to_string().contains("scan"));
    assert!(group.source().is_none());
    let missing = Error::MissingGroup {
        family: "nl80211",
        group: "mlme",
    };
    assert_eq!(
        missing.to_string(),
        "the nl80211 family has no mlme multicast group"
    );
    assert!(missing.source().is_none());
}
