//! Cancellable delayed action, used to postpone virtual-sink teardown so a
//! brief headset disconnect/reconnect (flaky USB, power glitch) doesn't
//! destroy and recreate the sinks.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub struct Deferred {
    generation: AtomicU64,
}

impl Deferred {
    pub const fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
        }
    }

    /// Run `action` after `delay` unless `cancel` or another `schedule` runs
    /// first. A zero delay runs the action inline.
    pub async fn schedule<F>(&'static self, delay: Duration, action: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mine = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if delay.is_zero() {
            action.await;
            return;
        }
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            if self.generation.load(Ordering::SeqCst) == mine {
                action.await;
            }
        });
    }

    /// Drop any pending action.
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    fn counter_action(c: &Arc<AtomicUsize>) -> impl Future<Output = ()> + Send + 'static {
        let c = Arc::clone(c);
        async move {
            c.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn runs_after_delay() {
        static D: Deferred = Deferred::new();
        let n = Arc::new(AtomicUsize::new(0));
        D.schedule(Duration::from_secs(5), counter_action(&n)).await;
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert_eq!(n.load(Ordering::SeqCst), 0);
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_prevents_run() {
        static D: Deferred = Deferred::new();
        let n = Arc::new(AtomicUsize::new(0));
        D.schedule(Duration::from_secs(5), counter_action(&n)).await;
        D.cancel();
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert_eq!(n.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn reschedule_supersedes_previous() {
        static D: Deferred = Deferred::new();
        let n = Arc::new(AtomicUsize::new(0));
        D.schedule(Duration::from_secs(5), counter_action(&n)).await;
        D.schedule(Duration::from_secs(5), counter_action(&n)).await;
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn zero_delay_runs_inline() {
        static D: Deferred = Deferred::new();
        let n = Arc::new(AtomicUsize::new(0));
        D.schedule(Duration::ZERO, counter_action(&n)).await;
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }
}
