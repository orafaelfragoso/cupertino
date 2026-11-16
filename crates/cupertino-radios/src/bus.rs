//! Calls to the radios' services that give up after a bounded wait, whatever the connection they are made on: a
//! service that is hung, stopped or slow to start answers nothing, and a reading must not wait for it for good.

use std::collections::HashMap;
use std::future::{poll_fn, Future};
use std::pin::pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::Poll;
use std::time::Duration;

use zbus::blocking::{Connection, Proxy};
use zbus::export::serde::Serialize;
use zbus::zvariant::{DynamicDeserialize, DynamicType, OwnedValue, Value};

/// How long a reading or a switch waits for a service's answer.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a call waits that the service answers only once a radio link is up, down or removed.
pub const LINK_TIMEOUT: Duration = Duration::from_secs(90);

const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// The wait of a reading or a switch, in milliseconds; 0 is [`CALL_TIMEOUT`].
static CALL_WAIT_MS: AtomicU64 = AtomicU64::new(0);

/// Make every reading and switch of this process wait `wait` for its answer instead of [`CALL_TIMEOUT`].
pub fn set_call_timeout(wait: Duration) {
    let ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX).max(1);
    CALL_WAIT_MS.store(ms, Ordering::Relaxed);
}

/// How long a reading or a switch waits for its answer.
pub fn call_timeout() -> Duration {
    match CALL_WAIT_MS.load(Ordering::Relaxed) {
        0 => CALL_TIMEOUT,
        ms => Duration::from_millis(ms),
    }
}

/// What a call that was not answered in time fails with.
fn unanswered_error() -> zbus::Error {
    zbus::fdo::Error::NoReply("The service did not answer in time.".into()).into()
}

/// Whether `error` says the service did not answer in time, here or as the bus daemon tells it.
pub fn unanswered(error: &zbus::Error) -> bool {
    match error {
        zbus::Error::FDO(e) => matches!(
            e.as_ref(),
            zbus::fdo::Error::NoReply(_)
                | zbus::fdo::Error::Timeout(_)
                | zbus::fdo::Error::TimedOut(_)
        ),
        zbus::Error::MethodError(name, _, _) => matches!(
            name.as_str(),
            "org.freedesktop.DBus.Error.NoReply"
                | "org.freedesktop.DBus.Error.Timeout"
                | "org.freedesktop.DBus.Error.TimedOut"
        ),
        zbus::Error::InputOutput(e) => e.kind() == std::io::ErrorKind::TimedOut,
        _ => false,
    }
}

/// Wait for `call` for `wait` at most.
fn within<T>(wait: Duration, call: impl Future<Output = zbus::Result<T>>) -> zbus::Result<T> {
    zbus::block_on(async {
        let mut call = pin!(call);
        let mut timer = pin!(async_io::Timer::after(wait));
        poll_fn(|cx| {
            if let Poll::Ready(answer) = call.as_mut().poll(cx) {
                return Poll::Ready(answer);
            }
            timer.as_mut().poll(cx).map(|_| Err(unanswered_error()))
        })
        .await
    })
}

/// A service's object asked with a bounded wait.
pub(crate) trait Bounded {
    /// Call `method` and wait for its answer for [`call_timeout`] at most.
    fn ask<B, R>(&self, method: &str, body: &B) -> zbus::Result<R>
    where
        B: Serialize + DynamicType,
        R: for<'d> DynamicDeserialize<'d>;

    /// Call `method`, which answers once a link is up, down or removed, and wait for [`LINK_TIMEOUT`] at most.
    fn ask_link<B, R>(&self, method: &str, body: &B) -> zbus::Result<R>
    where
        B: Serialize + DynamicType,
        R: for<'d> DynamicDeserialize<'d>;

    /// Read one property from the service.
    fn read<T>(&self, name: &str) -> zbus::Result<T>
    where
        T: TryFrom<OwnedValue>,
        T::Error: Into<zbus::Error>;

    /// Set one property of the service.
    fn write<'t, T>(&self, name: &str, value: T) -> zbus::Result<()>
    where
        T: 't + Into<Value<'t>>;
}

impl Bounded for Proxy<'_> {
    fn ask<B, R>(&self, method: &str, body: &B) -> zbus::Result<R>
    where
        B: Serialize + DynamicType,
        R: for<'d> DynamicDeserialize<'d>,
    {
        within(call_timeout(), self.inner().call(method, body))
    }

    fn ask_link<B, R>(&self, method: &str, body: &B) -> zbus::Result<R>
    where
        B: Serialize + DynamicType,
        R: for<'d> DynamicDeserialize<'d>,
    {
        within(LINK_TIMEOUT, self.inner().call(method, body))
    }

    fn read<T>(&self, name: &str) -> zbus::Result<T>
    where
        T: TryFrom<OwnedValue>,
        T::Error: Into<zbus::Error>,
    {
        within(call_timeout(), self.inner().get_property(name))
    }

    fn write<'t, T>(&self, name: &str, value: T) -> zbus::Result<()>
    where
        T: 't + Into<Value<'t>>,
    {
        within(call_timeout(), async {
            self.inner()
                .set_property(name, value)
                .await
                .map_err(zbus::Error::from)
        })
    }
}

/// Every property of `interface` on the object `path` of `name`, in one call.
pub(crate) fn get_all(
    connection: &Connection,
    name: &str,
    path: &str,
    interface: &str,
) -> zbus::Result<HashMap<String, OwnedValue>> {
    within(call_timeout(), async {
        connection
            .inner()
            .call_method(Some(name), path, Some(PROPERTIES), "GetAll", &(interface,))
            .await?
            .body()
            .deserialize()
    })
}

/// The property `key` of a [`get_all`] answer.
pub(crate) fn property<T>(properties: &HashMap<String, OwnedValue>, key: &str) -> zbus::Result<T>
where
    T: TryFrom<OwnedValue>,
    T::Error: Into<zbus::Error>,
{
    let value = properties
        .get(key)
        .ok_or_else(|| zbus::Error::Failure(format!("the service's object has no {key}")))?;
    T::try_from(value.try_clone()?).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_that_is_not_answered_fails_as_unanswered_after_its_wait() {
        let never = std::future::pending::<zbus::Result<()>>();
        let error = within(Duration::from_millis(5), never).unwrap_err();
        assert!(unanswered(&error), "{error}");
        assert!(within(Duration::from_secs(60), async { Ok(7) }).is_ok_and(|n| n == 7));
        assert!(!unanswered(&zbus::Error::Failure("refused".into())));
        assert!(unanswered(&zbus::Error::from(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "timed out"
        ))));
    }

    #[test]
    fn the_wait_of_a_call_can_be_set() {
        assert_eq!(call_timeout(), CALL_TIMEOUT);
        set_call_timeout(Duration::from_millis(40));
        assert_eq!(call_timeout(), Duration::from_millis(40));
    }
}
