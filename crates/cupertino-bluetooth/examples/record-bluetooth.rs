//! Records management traffic from this machine, read-only, as a fixture.
//!
//! ```sh
//! cargo run -p cupertino-bluetooth --example record-bluetooth -- [--listen SECONDS] > capture.txt
//! ```
//!
//! It sends only Read Management Version Information, Read Controller Index List and Read
//! Controller Information (one per controller), then optionally keeps listening for events. All of
//! these are allowed on an untrusted socket, so no privilege is needed. It writes one packet per
//! line, `>` for a command sent and `<` for a packet received, followed by the packet in hex.
//!
//! Before anything is written the controller's address and name are replaced by fixed fake values
//! (`FAKE_ADDRESS`, `FAKE_NAME`). A packet that could carry a device address or a key (anything
//! beyond controller-level events) is not written: the example stops with an error instead.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use cupertino_bluetooth::reply::parse_index_list;
use cupertino_bluetooth::{decode_packet, Command, ControlSocket, ControllerIndex, Event, Opcode};
use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::io::Errno;

/// Written in place of the controller address, in wire order (AA:BB:CC:DD:EE:01).
const FAKE_ADDRESS: [u8; 6] = [0x01, 0xEE, 0xDD, 0xCC, 0xBB, 0xAA];
const FAKE_NAME: &[u8] = b"cupertino-test";

/// How long to wait for the reply to one command.
const REPLY_WAIT: Duration = Duration::from_secs(2);

/// Header (6), Command Complete's opcode and status (3), then Read Controller Information's reply:
/// address 6, version 1, manufacturer 2, supported settings 4, current settings 4, class 3, name
/// 249, short name 11.
const INFO_ADDRESS: usize = 9;
const INFO_NAME: usize = INFO_ADDRESS + 6 + 1 + 2 + 4 + 4 + 3;
const INFO_SHORT_NAME: usize = INFO_NAME + 249;
const INFO_END: usize = INFO_SHORT_NAME + 11;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Waits until the socket is readable or `timeout` passes. Returns whether it is readable.
fn readable(socket: &ControlSocket, timeout: Duration) -> Result<bool, String> {
    let timeout = Timespec {
        tv_sec: timeout.as_secs() as i64,
        tv_nsec: i64::from(timeout.subsec_nanos()),
    };
    loop {
        let mut fds = [PollFd::new(socket, PollFlags::IN)];
        match poll(&mut fds, Some(&timeout)) {
            Ok(ready) => return Ok(ready > 0),
            Err(Errno::INTR) => continue,
            Err(errno) => return Err(format!("poll: {errno}")),
        }
    }
}

/// Replaces the controller's address and names in a Read Controller Information reply.
fn anonymise_info(packet: &mut [u8]) -> Result<(), String> {
    if packet.len() != INFO_END {
        return Err(format!(
            "a controller information reply has {} bytes, expected {INFO_END}",
            packet.len()
        ));
    }
    packet[INFO_ADDRESS..INFO_ADDRESS + 6].copy_from_slice(&FAKE_ADDRESS);
    let name = &mut packet[INFO_NAME..INFO_NAME + 249];
    name.fill(0);
    name[..FAKE_NAME.len()].copy_from_slice(FAKE_NAME);
    packet[INFO_SHORT_NAME..INFO_END].fill(0);
    Ok(())
}

/// Anonymises `packet` according to what it is, or refuses it.
fn sanitise(packet: &mut [u8]) -> Result<(), String> {
    let decoded = decode_packet(packet).map_err(|e| format!("a packet does not decode: {e}"))?;
    match decoded.event {
        Event::CommandComplete {
            opcode: Opcode::READ_CONTROLLER_INFO,
            ..
        } => anonymise_info(packet),
        Event::CommandComplete {
            opcode: Opcode::READ_VERSION | Opcode::READ_INDEX_LIST,
            ..
        }
        | Event::CommandStatus { .. }
        | Event::IndexAdded
        | Event::IndexRemoved
        | Event::NewSettings(_)
        | Event::ClassOfDeviceChanged(_) => Ok(()),
        Event::LocalNameChanged { .. } => {
            // Header 6, then name 249 and short name 11.
            packet[6..].fill(0);
            packet[6..6 + FAKE_NAME.len()].copy_from_slice(FAKE_NAME);
            Ok(())
        }
        other => Err(format!(
            "refusing to record {other:?}: it may carry a device address or a key"
        )),
    }
}

fn run(listen: Duration) -> Result<(), String> {
    let socket = ControlSocket::open().map_err(|e| e.to_string())?;
    println!("# Recorded read-only. Controller address and name replaced by fixed fake values.");
    println!("# '>' command sent, '<' packet received, hex of the whole packet.");

    let mut indexes = Vec::new();
    let mut queue = vec![
        (ControllerIndex::NONE, Command::ReadVersion),
        (ControllerIndex::NONE, Command::ReadIndexList),
    ];
    while !queue.is_empty() {
        let (index, command) = queue.remove(0);
        socket
            .send(index, &command)
            .map_err(|e| format!("sending {command:?}: {e}"))?;
        println!(
            "> {}",
            hex(&command.encode(index).map_err(|e| e.to_string())?)
        );
        if !readable(&socket, REPLY_WAIT)? {
            return Err(format!("no reply to {command:?} within {REPLY_WAIT:?}"));
        }
        for mut packet in socket.read_raw().map_err(|e| e.to_string())? {
            sanitise(&mut packet)?;
            println!("< {}", hex(&packet));
            if let Event::CommandComplete {
                opcode: Opcode::READ_INDEX_LIST,
                data,
                ..
            } = decode_packet(&packet).map_err(|e| e.to_string())?.event
            {
                indexes = parse_index_list(&data).map_err(|e| e.to_string())?;
                queue.extend(
                    indexes
                        .iter()
                        .map(|&index| (index, Command::ReadControllerInfo)),
                );
            }
        }
    }
    eprintln!("record: {} controller(s)", indexes.len());

    let deadline = Instant::now() + listen;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if !readable(&socket, left)? {
            break;
        }
        for mut packet in socket.read_raw().map_err(|e| e.to_string())? {
            sanitise(&mut packet)?;
            println!("< {}", hex(&packet));
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let listen = match (args.next().as_deref(), args.next()) {
        (None, _) => Duration::ZERO,
        (Some("--listen"), Some(seconds)) => match seconds.parse() {
            Ok(seconds) => Duration::from_secs(seconds),
            Err(_) => {
                eprintln!("record: --listen needs a number of seconds");
                return ExitCode::FAILURE;
            }
        },
        _ => {
            eprintln!("usage: record-bluetooth [--listen SECONDS]");
            return ExitCode::FAILURE;
        }
    };
    match run(listen) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("record: {message}");
            ExitCode::FAILURE
        }
    }
}
