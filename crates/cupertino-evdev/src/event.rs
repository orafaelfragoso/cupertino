//! The `struct input_event` record.

#[cfg(not(target_pointer_width = "64"))]
compile_error!("cupertino-evdev implements the 64-bit `struct input_event` layout only");

/// Size in bytes of one `struct input_event` on a 64-bit kernel: `timeval` (two `i64`), type, code, value.
pub const EVENT_SIZE: usize = 24;

/// One kernel input event, as read from `/dev/input/eventN`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputEvent {
    /// Timestamp seconds, on the clock selected with `EVIOCSCLOCKID`.
    pub sec: i64,
    /// Timestamp microseconds.
    pub usec: i64,
    /// Event type (`EV_*`).
    pub kind: u16,
    /// Event code (`KEY_*`, `REL_*`, `ABS_*`, ...).
    pub code: u16,
    /// Event value.
    pub value: i32,
}

impl InputEvent {
    /// Decodes one native-endian record.
    pub fn from_bytes(bytes: &[u8; EVENT_SIZE]) -> Self {
        let word = |at: usize| -> [u8; 8] { bytes[at..at + 8].try_into().expect("8-byte slice") };
        Self {
            sec: i64::from_ne_bytes(word(0)),
            usec: i64::from_ne_bytes(word(8)),
            kind: u16::from_ne_bytes([bytes[16], bytes[17]]),
            code: u16::from_ne_bytes([bytes[18], bytes[19]]),
            value: i32::from_ne_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
        }
    }

    /// Encodes the record in the kernel layout.
    pub fn to_bytes(&self) -> [u8; EVENT_SIZE] {
        let mut bytes = [0u8; EVENT_SIZE];
        bytes[0..8].copy_from_slice(&self.sec.to_ne_bytes());
        bytes[8..16].copy_from_slice(&self.usec.to_ne_bytes());
        bytes[16..18].copy_from_slice(&self.kind.to_ne_bytes());
        bytes[18..20].copy_from_slice(&self.code.to_ne_bytes());
        bytes[20..24].copy_from_slice(&self.value.to_ne_bytes());
        bytes
    }

    /// The timestamp in microseconds.
    pub fn time_us(&self) -> i64 {
        self.sec * 1_000_000 + self.usec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_the_kernel_layout() {
        let event = InputEvent { sec: 1_700_000_000, usec: 123_456, kind: 3, code: 0x35, value: -7 };
        let bytes = event.to_bytes();
        assert_eq!(&bytes[16..18], &3u16.to_ne_bytes());
        assert_eq!(InputEvent::from_bytes(&bytes), event);
        assert_eq!(event.time_us(), 1_700_000_000_123_456);
    }
}
