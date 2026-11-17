//! The error type of the shell protocols: what each case says, what it wraps and how a transport error converts.

use cupertino_shell_client::Error;
use std::error::Error as StdError;

#[test]
fn each_case_says_what_went_wrong() {
    assert_eq!(Error::UnknownOutput.to_string(), "unknown output");
    assert_eq!(
        Error::InvalidShape.to_string(),
        "rectangle is not finite or index is out of range"
    );
    assert_eq!(
        Error::Unsized.to_string(),
        "configure without a size for a surface that requested none"
    );
}

#[test]
fn a_transport_error_is_shown_as_the_transport_shows_it() {
    let inner = cupertino_wayland_client::Error::UnknownSurface;
    let shown = inner.to_string();
    let error = Error::Transport(inner);
    assert_eq!(error.to_string(), shown);
}

#[test]
fn a_transport_error_converts_and_is_the_source() {
    let error: Error = cupertino_wayland_client::Error::Cancelled.into();
    assert!(matches!(
        error,
        Error::Transport(cupertino_wayland_client::Error::Cancelled)
    ));
    let source = error.source().expect("the transport error is the source");
    assert_eq!(
        source.to_string(),
        cupertino_wayland_client::Error::Cancelled.to_string()
    );
}

#[test]
fn the_other_cases_have_no_source() {
    assert!(Error::UnknownOutput.source().is_none());
    assert!(Error::InvalidShape.source().is_none());
    assert!(Error::Unsized.source().is_none());
}

#[test]
fn the_error_is_usable_as_a_boxed_error() {
    let boxed: Box<dyn StdError + Send + Sync> = Box::new(Error::Unsized);
    assert!(!boxed.to_string().is_empty());
    assert_eq!(format!("{:?}", Error::UnknownOutput), "UnknownOutput");
}
