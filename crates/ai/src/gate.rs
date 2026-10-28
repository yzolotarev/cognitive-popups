//! Who reaches the model first. A request the reader is waiting for never waits;
//! background requests run one at a time and do not start while the reader waits.
//! A background request already in flight is not interrupted.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    /// The reader pressed a key and is looking at a window.
    Foreground,
    /// Precomputation, preparation, end-of-lesson analysis.
    Background,
}

/// How long background work yields to waiting foreground requests before going
/// ahead anyway, so a stuck foreground request cannot stall it forever.
const YIELD_LIMIT: Duration = Duration::from_secs(10);

#[derive(Default)]
pub struct Gate {
    foreground: AtomicUsize,
    background: Mutex<()>,
}

pub enum Turn<'a> {
    Foreground(&'a Gate),
    Background(#[allow(dead_code)] MutexGuard<'a, ()>),
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        if let Turn::Foreground(gate) = self {
            gate.foreground.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl Gate {
    pub fn enter(&self, priority: Priority) -> Turn<'_> {
        match priority {
            Priority::Foreground => {
                self.foreground.fetch_add(1, Ordering::SeqCst);
                Turn::Foreground(self)
            }
            Priority::Background => {
                let guard = self
                    .background
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let started = Instant::now();
                while self.foreground_waiting() > 0 && started.elapsed() < YIELD_LIMIT {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Turn::Background(guard)
            }
        }
    }

    pub fn foreground_waiting(&self) -> usize {
        self.foreground.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn foreground_does_not_wait_for_background() {
        let gate = Arc::new(Gate::default());
        let _background = gate.enter(Priority::Background);
        let started = Instant::now();
        let _foreground = gate.enter(Priority::Foreground);
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn background_yields_while_the_reader_waits() {
        let gate = Arc::new(Gate::default());
        let foreground = gate.enter(Priority::Foreground);
        let other = Arc::clone(&gate);
        let handle = std::thread::spawn(move || {
            let started = Instant::now();
            let _turn = other.enter(Priority::Background);
            started.elapsed()
        });
        std::thread::sleep(Duration::from_millis(150));
        drop(foreground);
        let waited = handle.join().unwrap();
        assert!(waited >= Duration::from_millis(100), "{waited:?}");
    }

    #[test]
    fn foreground_count_returns_to_zero() {
        let gate = Gate::default();
        {
            let _a = gate.enter(Priority::Foreground);
            let _b = gate.enter(Priority::Foreground);
            assert_eq!(gate.foreground_waiting(), 2);
        }
        assert_eq!(gate.foreground_waiting(), 0);
    }
}
