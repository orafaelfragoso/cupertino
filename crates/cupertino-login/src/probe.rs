//! Keeping `Capabilities` current: logind's answers and the session manager's presence are read on a thread of their own,
//! whenever the menu asks (`RefreshCapabilities`) and when the manager's name gets an owner, so no question to logind (a bus
//! round trip that can wait for a polkit decision) holds the agent's main loop. Many requests in a row are one read.

use std::sync::mpsc::Receiver;

use crate::capabilities::{Capabilities, Inputs};

/// For every request on `requests`, read `read` and hand the result to `publish`, until the sender is dropped. What is off
/// is logged with its reason when the inputs differ from the last read.
pub fn run(
    requests: Receiver<()>,
    mut read: impl FnMut() -> Inputs,
    mut publish: impl FnMut(Capabilities),
) {
    let mut last: Option<Inputs> = None;
    while requests.recv().is_ok() {
        while requests.try_recv().is_ok() {}
        let inputs = read();
        if last != Some(inputs) {
            for reason in Capabilities::reasons(&inputs) {
                eprintln!("cupertino-login: menu item off: {reason}");
            }
            last = Some(inputs);
        }
        publish(Capabilities::new(&inputs));
    }
}
