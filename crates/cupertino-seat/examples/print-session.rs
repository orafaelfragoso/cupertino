//! Connects to the system bus, finds this process's session and prints it. Takes no control of
//! the session, so it is safe to run from a desktop session.
//!
//! `cargo run -p cupertino-seat --example print-session`

use cupertino_seat::{connect, inspect, locate, Error, SessionLookup};

fn main() -> Result<(), Error> {
    let lookup = SessionLookup::from_environment()?;
    println!("lookup:  {lookup:?}");
    let conn = connect()?;
    let session = locate(&conn, &lookup)?;
    let info = inspect(&conn, &session)?;
    println!("path:    {}", info.path);
    println!("id:      {}", info.id);
    println!("type:    {}", info.session_type);
    println!("active:  {}", info.active);
    println!("vt:      {}", info.vt);
    println!("seat:    {}", info.seat);
    Ok(())
}
