//! Connecting to a PipeWire that is not there.

use cupertino_portal::screencast::Screencast;

#[test]
fn connecting_to_a_socket_nobody_listens_on_fails_with_the_reason() {
    // The only test of this binary, so no other thread reads the environment while it changes.
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("screencast-no-pipewire");
    std::env::set_var("PIPEWIRE_REMOTE", dir.join("pipewire-missing"));
    let error = match Screencast::connect(|_| {}) {
        Ok(_) => panic!("a missing socket cannot be connected to"),
        Err(error) => error,
    };
    assert!(
        error.to_string().starts_with("connecting to PipeWire"),
        "{error}"
    );
}
