//! Coalesced change notifications; never hold the backend lock while waiting.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Condvar, Mutex, OnceLock,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn next_revision() -> u64 {
    static COUNTER: OnceLock<AtomicU64> = OnceLock::new();
    COUNTER
        .get_or_init(|| {
            AtomicU64::new(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
            )
        })
        .fetch_add(1, Ordering::Relaxed)
}
#[derive(Clone, Copy)]
pub struct Stamp {
    pub revision: u64,
    pub connected: bool,
}
pub struct Events {
    state: Mutex<Stamp>,
    changed: Condvar,
}
impl Default for Events {
    fn default() -> Self {
        Self {
            state: Mutex::new(Stamp {
                revision: next_revision(),
                connected: false,
            }),
            changed: Condvar::new(),
        }
    }
}
impl Events {
    pub fn stamp(&self) -> Stamp {
        *self.state.lock().unwrap()
    }
    pub fn signal(&self, connected: bool) {
        *self.state.lock().unwrap() = Stamp {
            revision: next_revision(),
            connected,
        };
        self.changed.notify_all();
    }
    pub fn wait(&self, after: u64, timeout: Duration) -> Stamp {
        let state = self.state.lock().unwrap();
        let (state, _) = self
            .changed
            .wait_timeout_while(state, timeout, |s| s.connected && s.revision == after)
            .unwrap();
        *state
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn does_not_lose_events_before_wait_or_disconnects() {
        let events = Events::default();
        events.signal(true);
        let old = events.stamp();
        events.signal(true);
        assert_ne!(
            events.wait(old.revision, Duration::from_secs(1)).revision,
            old.revision
        );
        events.signal(false);
        assert!(
            !events
                .wait(events.stamp().revision, Duration::from_secs(1))
                .connected
        );
    }
    #[test]
    fn wakes_without_a_backend_lock() {
        let events = std::sync::Arc::new(Events::default());
        events.signal(true);
        let old = events.stamp();
        let sender = events.clone();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            sender.signal(true);
        });
        assert_ne!(
            events.wait(old.revision, Duration::from_secs(2)).revision,
            old.revision
        );
        worker.join().unwrap();
    }
}
