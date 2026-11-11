use super::*;

#[test]
fn framing_is_bounded_and_requires_a_complete_line() {
    assert_eq!(read_line(&mut &b"{}\nrest"[..], 3).unwrap(), b"{}\n");
    assert!(read_line(&mut &b"{}\n"[..], 2).is_err());
    assert!(read_line(&mut &b"{}"[..], 3).is_err());
    assert!(read_line(&mut &b""[..], 3).unwrap().is_empty());
}
#[test]
fn a_private_socket_has_one_live_owner_and_cleans_up_only_its_inode() {
    let runtime = tempfile::tempdir().unwrap();
    std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let first = Listener::bind(runtime.path(), "wayland-99").unwrap();
    assert_eq!(
        std::fs::metadata(&first.path).unwrap().mode() & 0o777,
        0o600
    );
    assert!(Listener::bind(runtime.path(), "wayland-99").is_err());
    let path = first.path.clone();
    drop(first);
    assert!(!path.exists());
    let second = Listener::bind(runtime.path(), "wayland-99").unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, "replacement").unwrap();
    drop(second);
    assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
}
#[test]
fn path_traversal_symlinks_and_shared_directories_are_refused() {
    let runtime = tempfile::tempdir().unwrap();
    for name in ["", "../other", "a/b", "wayland 1"] {
        assert!(socket_path(runtime.path(), name).is_err());
    }
    let other = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(other.path(), runtime.path().join("cupertino")).unwrap();
    assert!(Listener::bind(runtime.path(), "wayland-1").is_err());
    std::fs::remove_file(runtime.path().join("cupertino")).unwrap();
    std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Listener::bind(runtime.path(), "wayland-1").is_err());
}
#[test]
fn the_peer_start_time_is_field_22_of_its_process() {
    assert_eq!(
        start_of(
            "4242 (a (b) c) S 1 4242 4242 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000"
        ),
        Some(987654)
    );
    assert_eq!(start_of("4242 (x) S 1 2"), None);
    assert!(start_time(std::process::id()).is_some_and(|start| start > 0));
}
