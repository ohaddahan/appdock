use std::time::Duration;
const PUBLISH: Duration = Duration::from_millis(2100);
const DISCOVER: Duration = Duration::from_millis(10500);

/// The caller supplies monotonic elapsed time; tests use a fake clock. Missed
/// deadlines run once and schedule from now, with no catch-up bursts.
#[derive(Default)]
pub struct RefreshSchedule {
    publish: Duration,
    discover: Duration,
}
impl RefreshSchedule {
    pub fn due(&mut self, now: Duration, pointer_down: bool) -> (bool, bool) {
        let publish = now >= self.publish;
        let discover = now >= self.discover && !pointer_down;
        if publish {
            self.publish = now + PUBLISH;
        }
        if discover {
            self.discover = now + DISCOVER;
        }
        (publish, discover)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn d1_initial_boundaries_delays_and_pointer_resumption() {
        let mut clock = RefreshSchedule::default();
        assert_eq!(clock.due(Duration::ZERO, false), (true, true));
        assert_eq!(
            clock.due(Duration::from_millis(2099), false),
            (false, false)
        );
        assert_eq!(clock.due(PUBLISH, false), (true, false));
        assert_eq!(clock.due(DISCOVER, true), (true, false));
        assert_eq!(
            clock.due(DISCOVER + Duration::from_millis(1), false),
            (false, true)
        );
        let delayed = Duration::from_secs(200);
        assert_eq!(clock.due(delayed, false), (true, true));
        assert_eq!(clock.due(delayed, false), (false, false));
        assert_eq!(clock.due(delayed + PUBLISH, false), (true, false));
        assert_eq!(clock.due(delayed + DISCOVER, false), (true, true));
    }
}
