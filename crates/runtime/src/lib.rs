use std::time::{Duration, Instant};

use smol::channel::{Receiver, Sender};

#[derive(Debug)]
pub struct TimerCanceled;

impl std::fmt::Display for TimerCanceled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("timer canceled")
    }
}

impl std::error::Error for TimerCanceled {}

#[derive(Debug)]
pub struct DeadlineOverflow;

impl std::fmt::Display for DeadlineOverflow {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("deadline exceeds the monotonic clock")
    }
}

impl std::error::Error for DeadlineOverflow {}

#[derive(Debug)]
enum DeadlineSource {
    System { timer: smol::Timer, origin: Instant },
    Controlled(Receiver<Duration>),
}

#[derive(Debug)]
#[must_use]
pub struct Deadline(DeadlineSource);

#[derive(Debug)]
pub struct DeadlineTrigger(Sender<Duration>);

impl Deadline {
    pub fn after(delay: Duration) -> Result<Self, DeadlineOverflow> {
        let origin = Instant::now();
        let at = origin.checked_add(delay).ok_or(DeadlineOverflow)?;
        Ok(Self(DeadlineSource::System { timer: smol::Timer::at(at), origin }))
    }

    pub fn controlled() -> (DeadlineTrigger, Self) {
        let (sender, receiver) = smol::channel::bounded(1);
        (DeadlineTrigger(sender), Self(DeadlineSource::Controlled(receiver)))
    }

    pub async fn wait(self) -> Result<Duration, TimerCanceled> {
        match self.0 {
            DeadlineSource::System { timer, origin } => {
                let instant = timer.await;
                Ok(instant.saturating_duration_since(origin))
            }
            DeadlineSource::Controlled(receiver) => receiver.recv().await.map_err(|_| TimerCanceled),
        }
    }
}

impl DeadlineTrigger {
    pub fn fire(self, now: Duration) -> Result<(), TimerCanceled> {
        self.0.send_blocking(now).map_err(|_| TimerCanceled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_deadline_uses_the_requested_delay() {
        let start = Instant::now();
        let delay = Duration::from_millis(5);
        let elapsed = smol::block_on(Deadline::after(delay).unwrap().wait()).unwrap();
        assert_eq!(elapsed, delay);
        assert!(start.elapsed() >= delay);
    }

    #[test]
    fn controlled_deadline_propagates_cancellation_and_mock_time() {
        let (trigger, deadline) = Deadline::controlled();
        drop(trigger);
        assert!(smol::block_on(deadline.wait()).is_err());
        let (trigger, deadline) = Deadline::controlled();
        trigger.fire(Duration::from_millis(37)).unwrap();
        assert_eq!(smol::block_on(deadline.wait()).unwrap(), Duration::from_millis(37));
    }

    #[test]
    fn unrepresentable_deadline_is_rejected() {
        assert!(Deadline::after(Duration::MAX).is_err());
    }

    #[test]
    fn dropping_a_task_cancels_its_system_deadline_callback() {
        let executor = smol::LocalExecutor::new();
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let outcome = ran.clone();
        let task = executor.spawn(async move {
            Deadline::after(Duration::from_millis(5)).unwrap().wait().await.unwrap();
            outcome.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        assert!(executor.try_tick());
        drop(task);
        smol::block_on(smol::Timer::after(Duration::from_millis(10)));
        while executor.try_tick() {}
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
    }
}
