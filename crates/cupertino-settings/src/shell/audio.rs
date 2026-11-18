//! The Sound pane's Audio1 requests. One future at a time preserves the order of a device
//! selection and a subsequent volume change. Adjacent slider writes coalesce; reads never
//! overwrite a newer change. Kit polls the futures without blocking the window.

use std::collections::VecDeque;
use std::future::{poll_fn, Future};
use std::task::Poll;
use std::time::Duration;

use cupertino_kit::core::Command;
use zbus::export::serde::Serialize;
use zbus::zvariant::{DynamicDeserialize, DynamicType};

use super::Message;
use crate::live::{Output, Reading, Volume};
use crate::panes::{AudioChange, ControlId};
use crate::services::{failure, Bus};

const NAME: &str = "org.cupertino.Audio1";
const PATH: &str = "/org/cupertino/Audio1";
/// A whole pane request (including connection and activation); the individual backend commands
/// have their own one-second timeout.
const PATIENCE: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub input: bool,
    pub devices: Vec<Output>,
    pub volume: Reading<Volume>,
    pub balance: Option<f64>,
}

#[derive(Default)]
pub(super) struct Audio {
    reading: Option<bool>,
    writes: VecDeque<(ControlId, AudioChange)>,
    busy: bool,
}

impl Audio {
    pub fn read(&mut self, input: bool) {
        self.reading = Some(input);
    }

    pub fn write(&mut self, id: ControlId, change: AudioChange) {
        if let Some((previous, value)) = self.writes.back_mut() {
            if *previous == id {
                *value = change;
                return;
            }
        }
        self.writes.push_back((id, change));
    }

    pub fn done(&mut self) {
        self.busy = false;
    }

    pub fn has_writes(&self) -> bool {
        !self.writes.is_empty()
    }

    pub fn command(&mut self, bus: &Bus) -> Option<Command<Message>> {
        if self.busy {
            return None;
        }
        let write = self.writes.pop_front();
        let input = if write.is_none() {
            self.reading.take()?
        } else {
            false
        };
        self.busy = true;
        let connection = bus.audio_connection();
        Some(Command::Perform(Box::pin(async move {
            if let Some((id, change)) = write {
                let result = within(async { apply(&connection.await?, &change).await }).await;
                Message::AudioWritten(id, result.map_err(|e| failure(&e)))
            } else {
                let result = within(async { Ok(read(&connection.await?, input).await) }).await;
                Message::AudioRead(result.unwrap_or_else(|e| Snapshot {
                    input,
                    devices: Vec::new(),
                    volume: Reading::Unavailable(failure(&e)),
                    balance: None,
                }))
            }
        })))
    }
}

async fn within<T>(work: impl Future<Output = zbus::Result<T>>) -> zbus::Result<T> {
    let mut work = std::pin::pin!(work);
    let mut timer = std::pin::pin!(async_io::Timer::after(PATIENCE));
    poll_fn(|cx| match work.as_mut().poll(cx) {
        Poll::Ready(r) => Poll::Ready(r),
        Poll::Pending => timer
            .as_mut()
            .poll(cx)
            .map(|_| Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into())),
    })
    .await
}

async fn call<B: Serialize + DynamicType, R: for<'d> DynamicDeserialize<'d>>(
    conn: &zbus::Connection,
    method: &str,
    body: &B,
) -> zbus::Result<R> {
    conn.call_method(Some(NAME), PATH, Some(NAME), method, body)
        .await?
        .body()
        .deserialize()
}

async fn read(conn: &zbus::Connection, input: bool) -> Snapshot {
    let direction = if input { "input" } else { "output" };
    let devices = call::<_, Vec<(u32, String, bool)>>(conn, "ListDevices", &direction)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(id, name, default)| Output { id, name, default })
        .collect();
    let volume = match call(conn, "GetVolume", &direction).await {
        Ok((level, muted)) => Reading::Ready(Volume { level, muted }),
        Err(e) => Reading::Unavailable(failure(&e)),
    };
    let balance = if input {
        None
    } else {
        call(conn, "GetBalance", &()).await.ok()
    };
    Snapshot {
        input,
        devices,
        volume,
        balance,
    }
}

async fn apply(conn: &zbus::Connection, change: &AudioChange) -> zbus::Result<()> {
    match *change {
        AudioChange::SetInput(id) => call(conn, "SetDefault", &("input", id)).await,
        AudioChange::SetOutput(id) => call(conn, "SetDefault", &("output", id)).await,
        AudioChange::SetBalance(value) => call(conn, "SetBalance", &value).await,
        AudioChange::SetVolume { level, muted } | AudioChange::SetInputVolume { level, muted } => {
            let direction = if matches!(change, AudioChange::SetInputVolume { .. }) {
                "input"
            } else {
                "output"
            };
            call::<_, (f64, bool)>(conn, "SetVolume", &(direction, level, muted))
                .await
                .map(drop)
        }
    }
}
