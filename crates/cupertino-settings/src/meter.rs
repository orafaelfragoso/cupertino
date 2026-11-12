//! The input level meter of the Sound pane: while the pane shows the input, `pw-record` listens to the default input at
//! a low rate and this reads how loud it is. Nothing is kept or written; stopping the meter ends the recording.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// The sample rate the meter listens at, in samples per second.
const RATE: u32 = 8000;
/// How many samples make one reading (50 ms).
const WINDOW: usize = (RATE / 20) as usize;
/// The quietest level the meter shows, in decibels below full scale; anything below reads as silence.
const FLOOR_DB: f64 = 50.0;
/// The size of the header the recording starts with, in bytes.
const HEADER: usize = 24;

/// The meter's level in 0 to 1 for the loudness of the signal `samples` (16-bit, as numbers).
pub fn level_of(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mean_square =
        samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len() as f64;
    let db = 10.0 * (mean_square / 32768.0_f64.powi(2)).max(1e-12).log10();
    ((db + FLOOR_DB) / FLOOR_DB).clamp(0.0, 1.0)
}

/// A recording that measures the input.
pub struct Meter {
    child: Child,
    level: Arc<AtomicU32>,
}

impl Meter {
    /// Start listening to the default input; `None` when `pw-record` cannot be started.
    pub fn start() -> Option<Meter> {
        let mut child = Command::new("pw-record")
            .args([
                "--rate",
                &RATE.to_string(),
                "--channels",
                "1",
                "--format",
                "s16",
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut out = child.stdout.take()?;
        let level = Arc::new(AtomicU32::new(0));
        let shared = level.clone();
        std::thread::spawn(move || {
            let mut header = [0u8; HEADER];
            if out.read_exact(&mut header).is_err() {
                return;
            }
            // the header says where the samples start; skip anything beyond the 24 bytes read
            let start = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
            let mut skip = vec![0u8; start.saturating_sub(HEADER).min(4096)];
            if out.read_exact(&mut skip).is_err() {
                return;
            }
            let mut bytes = vec![0u8; WINDOW * 2];
            while out.read_exact(&mut bytes).is_ok() {
                let samples: Vec<i16> = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| i16::from_be_bytes([b[0], b[1]]))
                    .collect();
                shared.store((level_of(&samples) as f32).to_bits(), Ordering::Relaxed);
            }
        });
        Some(Meter { child, level })
    }

    /// The loudness of the last window, 0 to 1.
    pub fn level(&self) -> f64 {
        f64::from(f32::from_bits(self.level.load(Ordering::Relaxed)))
    }
}

impl Drop for Meter {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
