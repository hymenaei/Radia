use std::time::{Duration, Instant};

const RECOVERY_DELAY: Duration = Duration::from_millis(500);
pub(super) const MAX_RECOVERIES: u32 = 3;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum RecoveryAttempt {
    Start,
    WaitUntil(Instant),
    Exhausted,
}

#[derive(Default)]
pub(super) struct PresentationRecovery {
    attempts: u32,
    retry_at: Option<Instant>,
}

impl PresentationRecovery {
    pub(super) fn attempts(&self) -> u32 {
        self.attempts
    }

    pub(super) fn begin_attempt(&mut self, now: Instant) -> RecoveryAttempt {
        if let Some(retry_at) = self.retry_at
            && now < retry_at
        {
            return RecoveryAttempt::WaitUntil(retry_at);
        }
        if self.attempts == MAX_RECOVERIES {
            return RecoveryAttempt::Exhausted;
        }
        self.attempts += 1;
        self.retry_at = None;
        RecoveryAttempt::Start
    }

    pub(super) fn schedule_retry(&mut self, now: Instant) -> Instant {
        let retry_at = now + RECOVERY_DELAY;
        self.retry_at = Some(retry_at);
        retry_at
    }

    pub(super) fn presented(&mut self) {
        self.attempts = 0;
        self.retry_at = None;
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{MAX_RECOVERIES, PresentationRecovery, RECOVERY_DELAY, RecoveryAttempt};

    #[test]
    fn presentation_resets_recovery_limit() {
        let mut recovery = PresentationRecovery::default();
        let mut now = Instant::now();
        for _ in 0..6 {
            assert_eq!(recovery.begin_attempt(now), RecoveryAttempt::Start);
            recovery.presented();
        }
        for _ in 0..MAX_RECOVERIES {
            assert_eq!(recovery.begin_attempt(now), RecoveryAttempt::Start);
            now = recovery.schedule_retry(now);
        }
        assert_eq!(recovery.begin_attempt(now), RecoveryAttempt::Exhausted, "three attempts without a presented frame exhaust recovery");
        recovery.presented();
        assert_eq!(recovery.begin_attempt(now), RecoveryAttempt::Start, "a presented frame makes recovery available again");
    }

    #[test]
    fn recovery_retries_after_delay() {
        let mut recovery = PresentationRecovery::default();
        let now = Instant::now();

        assert_eq!(recovery.begin_attempt(now), RecoveryAttempt::Start);
        let retry_at = recovery.schedule_retry(now);
        assert_eq!(retry_at, now + RECOVERY_DELAY);
        assert_eq!(recovery.begin_attempt(retry_at - Duration::from_nanos(1)), RecoveryAttempt::WaitUntil(retry_at));
        assert_eq!(recovery.begin_attempt(retry_at), RecoveryAttempt::Start);
    }
}
