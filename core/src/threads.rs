//! The thread budget of one [`crate::normalize_workbook`] call ([`crate::NormalizeOptions::threads`]),
//! read by every inner fan-out (totals beside the layout, the model's first passes and stream,
//! column typing) without changing their public signatures. A thread-local set for the duration
//! of the call; threads spawned through [`spawn`] inherit it. Outside such a call (None) every
//! fan-out works as before. The results never depend on it, only the number of threads.

use std::cell::Cell;

thread_local! {
    static LIMIT: Cell<Option<usize>> = const { Cell::new(None) };
    static SPAWNED: Cell<usize> = const { Cell::new(0) };
}

/// Runs `f` with the thread budget `limit` (None: no limit), restoring the previous one after
/// (also on a panic).
pub(crate) fn with<R>(limit: Option<usize>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<usize>);
    impl Drop for Restore {
        fn drop(&mut self) {
            LIMIT.with(|l| l.set(self.0));
        }
    }
    let _restore = Restore(LIMIT.with(|l| l.replace(limit)));
    f()
}

/// At most `wanted` threads, within the current budget (at least 1).
pub(crate) fn cap(wanted: usize) -> usize {
    LIMIT
        .with(Cell::get)
        .map_or(wanted, |n| wanted.min(n))
        .max(1)
}

/// Whether a fan-out may run work beside the current thread (the budget is not 1).
pub(crate) fn parallel() -> bool {
    cap(2) > 1
}

/// [`std::thread::Scope::spawn`] that passes the current budget on to the new thread.
pub(crate) fn spawn<'scope, T: Send + 'scope>(
    s: &'scope std::thread::Scope<'scope, '_>,
    f: impl FnOnce() -> T + Send + 'scope,
) -> std::thread::ScopedJoinHandle<'scope, T> {
    let limit = LIMIT.with(Cell::get);
    SPAWNED.with(|c| c.set(c.get() + 1));
    s.spawn(move || with(limit, f))
}

/// Threads the fan-outs have spawned from the calling thread so far (a test hook: a
/// `threads: Some(1)` normalization spawns none).
#[doc(hidden)]
#[must_use]
pub fn spawned() -> usize {
    SPAWNED.with(Cell::get)
}
