//! `kit::account`: the entries of the password database.

use cupertino_kit::account::{self, Account};
use std::path::PathBuf;

#[test]
fn the_superuser_is_root_with_its_home() {
    let root = account::by_uid(0)
        .unwrap()
        .expect("uid 0 is in the database");
    assert_eq!(root.uid, 0);
    assert_eq!(root.login, "root");
    assert_eq!(root.home, PathBuf::from("/root"));
}

#[test]
fn the_process_runs_as_the_account_of_its_user_id() {
    let current = account::current()
        .unwrap()
        .expect("the test runs as a known user");
    assert_eq!(current.uid, account::uid());
    assert!(!current.login.is_empty());
    assert_eq!(account::by_uid(account::uid()).unwrap(), Some(current));
}

#[test]
fn a_user_id_with_no_entry_has_no_account() {
    // the last id below `(uid_t) -1`, which no distribution hands out
    assert_eq!(account::by_uid(u32::MAX - 1).unwrap(), None);
}

#[test]
fn the_full_name_is_the_first_field_of_the_comment() {
    let with = |gecos: &str| Account {
        uid: 1000,
        login: "ada".into(),
        gecos: gecos.into(),
        home: PathBuf::from("/home/ada"),
    };
    assert_eq!(with("Ada Lovelace,Room 1,,").full_name(), "Ada Lovelace");
    assert_eq!(with(" Ada ").full_name(), " Ada ");
    assert_eq!(with(",Room 1").full_name(), "");
    assert_eq!(with("").full_name(), "");
}
