//! The limit on open files. Every client connection, buffer fence and device the server holds is a descriptor, and the
//! soft limit of a login session is often 1024.

use rustix::process::{getrlimit, setrlimit, Resource, Rlimit};

/// What to set when the soft limit is below the hard one: the soft limit moves up to the hard limit.
fn raised(limit: Rlimit) -> Option<Rlimit> {
    (limit.current != limit.maximum).then_some(Rlimit {
        current: limit.maximum,
        maximum: limit.maximum,
    })
}

fn show(value: Option<u64>) -> String {
    value.map_or_else(|| "unlimited".into(), |v| v.to_string())
}

/// Raises the soft limit on open files to the hard limit and logs both values.
pub fn raise() {
    let before = getrlimit(Resource::Nofile);
    let Some(wanted) = raised(before) else {
        tracing::info!(limit = show(before.current), "open file limit");
        return;
    };
    match setrlimit(Resource::Nofile, wanted) {
        Ok(()) => tracing::info!(
            soft = show(before.current),
            hard = show(before.maximum),
            "raised the soft open file limit to the hard limit"
        ),
        Err(e) => tracing::warn!(
            soft = show(before.current),
            hard = show(before.maximum),
            error = %e,
            "could not raise the soft open file limit"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn limit(current: Option<u64>, maximum: Option<u64>) -> Rlimit {
        Rlimit { current, maximum }
    }

    #[test]
    fn the_soft_limit_moves_up_to_the_hard_limit() {
        assert_eq!(
            raised(limit(Some(1024), Some(524_288))),
            Some(limit(Some(524_288), Some(524_288)))
        );
        assert_eq!(raised(limit(Some(1024), None)), Some(limit(None, None)));
    }

    #[test]
    fn a_soft_limit_at_the_hard_limit_is_left_alone() {
        assert_eq!(raised(limit(Some(4096), Some(4096))), None);
        assert_eq!(raised(limit(None, None)), None);
    }
}
