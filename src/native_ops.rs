//! Deterministic seams shared by real AX operations and scripted regressions.
use crate::model::*;

pub fn prepared_request<T>(
    current: impl Fn() -> bool,
    prepare: impl FnOnce() -> Result<()>,
    request: impl FnOnce() -> Result<T>,
) -> Result<T> {
    if !current() {
        return Err(BackendError::cancelled());
    }
    prepare()?;
    if !current() {
        return Err(BackendError::cancelled());
    }
    let result = request();
    if !current() {
        return Err(BackendError::cancelled());
    }
    result
}

/// Optional metadata may be absent, but interruptions must survive fallback paths.
pub fn optional<T>(result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if matches!(e.kind, ErrorKind::Cancelled | ErrorKind::Permission) => Err(e),
        Err(_) => Ok(None),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Membership {
    Present(usize),
    Closed,
}
/// Only complete, successful enumeration establishes absence. Cached state/title
/// reads never participate. Ambiguity retains recovery instead of guessing.
pub fn membership(
    exact: Option<usize>,
    identifier: Option<&str>,
    owners: usize,
    candidates: impl IntoIterator<Item = Result<Option<String>>>,
) -> Result<Membership> {
    if let Some(index) = exact {
        return Ok(Membership::Present(index));
    }
    let Some(identifier) = identifier else {
        return Ok(Membership::Closed);
    };
    let mut matches = vec![];
    for (index, candidate) in candidates.into_iter().enumerate() {
        if candidate?.as_deref() == Some(identifier) {
            matches.push(index);
        }
    }
    match matches.as_slice() {
        [] => Ok(Membership::Closed),
        [index] if owners == 1 => Ok(Membership::Present(*index)),
        _ => Err(BackendError::new(
            ErrorKind::Communication,
            "Replacement window identity is ambiguous",
        )),
    }
}

/// A barrier only cancels queued commands. Editing can start during any native
/// operation, so check again before every subsequent focus operation/readback.
pub fn focus_sequence<T>(
    suspended: impl Fn() -> bool,
    mut operation: impl FnMut(usize) -> Result<()>,
    mut poll: impl FnMut() -> Result<Option<T>>,
    mut wait: impl FnMut(),
) -> Result<Option<T>> {
    for step in 0..3 {
        if suspended() {
            return Err(BackendError::cancelled());
        }
        operation(step)?;
    }
    for _ in 0..30 {
        if suspended() {
            return Err(BackendError::cancelled());
        }
        if let Some(result) = poll()? {
            if suspended() {
                return Err(BackendError::cancelled());
            }
            return Ok(Some(result));
        }
        wait();
    }
    Err("Target window did not receive keyboard focus".into())
}

/// Four 40 ms polling intervals preserve the existing 160 ms sampling budget.
pub fn settle_frame(
    mut read: impl FnMut() -> Result<Rect>,
    mut wait: impl FnMut(),
) -> Result<Rect> {
    let mut actual = read()?;
    let mut stable = 0;
    for _ in 0..4 {
        wait();
        let next = read()?;
        stable = if next.near(actual) { stable + 1 } else { 0 };
        actual = next;
        if stable >= 2 && actual.valid() {
            return Ok(actual);
        }
    }
    Err(BackendError::new(
        ErrorKind::UnsettledGeometry,
        format!(
            "Window geometry did not settle within four polling intervals; last frame: {actual:?}"
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    #[test]
    fn a5_every_request_prepares_its_object_before_reading() {
        let calls = RefCell::new(vec![]);
        for object in ["app", "window", "replacement"] {
            prepared_request(
                || true,
                || {
                    calls.borrow_mut().push((object, "timeout"));
                    Ok(())
                },
                || {
                    calls.borrow_mut().push((object, "read"));
                    Ok(())
                },
            )
            .unwrap();
        }
        assert_eq!(
            *calls.borrow(),
            vec![
                ("app", "timeout"),
                ("app", "read"),
                ("window", "timeout"),
                ("window", "read"),
                ("replacement", "timeout"),
                ("replacement", "read")
            ]
        );
    }
    #[test]
    fn a5_cancellation_during_enumeration_stops_before_next_request() {
        for cancel_at in 0..4 {
            let calls = Cell::new(0);
            let current = Cell::new(true);
            for _ in 0..8 {
                let result = optional(prepared_request(
                    || current.get(),
                    || Ok(()),
                    || {
                        let n = calls.get();
                        calls.set(n + 1);
                        if n == cancel_at {
                            current.set(false);
                        }
                        Ok(())
                    },
                ));
                if let Err(e) = result {
                    assert_eq!(e.kind, ErrorKind::Cancelled);
                    break;
                }
            }
            assert_eq!(calls.get(), cancel_at + 1);
        }
    }
    #[test]
    fn a1_membership_ignores_cached_attributes_and_rebinds_only_unique_identity() {
        assert_eq!(membership(None, None, 1, []).unwrap(), Membership::Closed);
        assert_eq!(
            membership(None, Some("unique"), 1, [Ok(Some("unique".into()))]).unwrap(),
            Membership::Present(0)
        );
        assert!(membership(None, Some("unique"), 2, [Ok(Some("unique".into()))]).is_err());
        assert!(
            membership(
                None,
                Some("unique"),
                1,
                [Err(BackendError::new(ErrorKind::Communication, "timeout"))]
            )
            .is_err()
        );
        assert!(
            membership(
                None,
                Some("unique"),
                1,
                [Err(BackendError::new(ErrorKind::Permission, "denied"))]
            )
            .is_err()
        );
        assert_eq!(
            membership(None, Some("gone"), 1, [Ok(None)]).unwrap(),
            Membership::Closed
        );
    }
    #[test]
    fn d4_editing_between_native_operations_and_during_polling_stops_focus() {
        for suspend_after in 0..5 {
            let calls = RefCell::new(vec![]);
            let suspended = Cell::new(suspend_after == 0);
            let result = focus_sequence(
                || suspended.get(),
                |step| {
                    calls.borrow_mut().push(step);
                    if calls.borrow().len() == suspend_after {
                        suspended.set(true);
                    }
                    Ok(())
                },
                || {
                    calls.borrow_mut().push(3);
                    if calls.borrow().len() == suspend_after {
                        suspended.set(true);
                    }
                    Ok(None::<()>)
                },
                || {},
            );
            assert_eq!(result.unwrap_err().kind, ErrorKind::Cancelled);
            assert_eq!(calls.borrow().len(), suspend_after);
        }
    }
    #[test]
    fn d3_delayed_geometry_must_not_succeed_on_first_unchanged_poll() {
        let old = Rect::default();
        let changed = Rect { x: 600., ..old };
        let mut values = [old, old, changed, changed, changed].into_iter();
        assert_eq!(
            settle_frame(|| Ok(values.next().unwrap()), || {}).unwrap(),
            changed
        );
    }
    #[test]
    fn d3_oscillation_reports_unsettled_geometry() {
        let a = Rect::default();
        let b = Rect { x: 600., ..a };
        let mut values = [a, b, a, b, a].into_iter();
        assert_eq!(
            settle_frame(|| Ok(values.next().unwrap()), || {})
                .unwrap_err()
                .kind,
            ErrorKind::UnsettledGeometry
        );
    }

    #[test]
    fn d3_clamped_frame_is_accepted_after_two_stable_intervals() {
        let clamped = Rect {
            width: 1500.,
            ..Rect::default()
        };
        let polls = Cell::new(0);
        assert_eq!(
            settle_frame(|| Ok(clamped), || polls.set(polls.get() + 1)).unwrap(),
            clamped
        );
        assert_eq!(polls.get(), 2);
    }
    #[test]
    fn d3_restoration_can_retry_after_unsettled_geometry() {
        let a = Rect::default();
        let b = Rect { x: 600., ..a };
        let mut values = [a, b, a, b, a].into_iter();
        assert!(settle_frame(|| Ok(values.next().unwrap()), || {}).is_err());
        assert_eq!(settle_frame(|| Ok(a), || {}).unwrap(), a);
    }
}
