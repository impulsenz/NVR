//! Locks that survive a panic elsewhere.
//!
//! A `std::sync::Mutex` is "poisoned" when a thread panics while holding it,
//! and every later `.lock().unwrap()` then panics too, so one fault can bring
//! down unrelated parts (recording included). GAZE's shared state is plain
//! data that stays usable after a panic, so these helpers take the data back
//! instead of propagating the poison.

use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

pub trait MutexExt<T> {
    /// Lock, ignoring poisoning.
    fn locked(&self) -> MutexGuard<'_, T>;
}

impl<T> MutexExt<T> for Mutex<T> {
    fn locked(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub trait RwLockExt<T> {
    /// Read-lock, ignoring poisoning.
    fn reader(&self) -> RwLockReadGuard<'_, T>;
    /// Write-lock, ignoring poisoning.
    fn writer(&self) -> RwLockWriteGuard<'_, T>;
}

impl<T> RwLockExt<T> for RwLock<T> {
    fn reader(&self) -> RwLockReadGuard<'_, T> {
        self.read().unwrap_or_else(|e| e.into_inner())
    }
    fn writer(&self) -> RwLockWriteGuard<'_, T> {
        self.write().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_panic_while_locked_does_not_poison_later_callers() {
        let m = Arc::new(Mutex::new(1));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let _g = m2.locked();
            panic!("boom");
        })
        .join();
        assert!(m.lock().is_err(), "the std mutex is poisoned");
        *m.locked() += 1;
        assert_eq!(*m.locked(), 2);

        let r = Arc::new(RwLock::new(1));
        let r2 = r.clone();
        let _ = std::thread::spawn(move || {
            let _g = r2.writer();
            panic!("boom");
        })
        .join();
        *r.writer() += 1;
        assert_eq!(*r.reader(), 2);
    }
}
