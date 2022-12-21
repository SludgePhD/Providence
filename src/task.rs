use std::{
    any::Any,
    future::Future,
    panic::{self, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
};

use async_std::task::{self, JoinHandle};
use futures::FutureExt;

/// An owned, asynchronous task running in the background.
///
/// This type enforces structured concurrency: the asynchronous computation runs only as long as its
/// [`Task`] exists. When dropped, the computation will be canceled. Panics happening inside the
/// [`Task`] will be propagated to the thread or task holding the corresponding [`Task`] value
/// (either when calling [`Task::block`] or when dropping the value).
pub struct Task<T> {
    handle: Option<JoinHandle<Result<T, Box<dyn Any + Send>>>>,
    /// Set to `true` when the task exits (in *any* fashion, including cancellation and panicking).
    finished: Arc<AtomicBool>,
}

impl<T> Task<T> {
    /// Spawns a new background task that will poll `future`.
    pub fn spawn<F>(future: F) -> Self
    where
        F: Future<Output = T> + Send + 'static,
        F::Output: Send + 'static,
    {
        let finished = Arc::new(AtomicBool::new(false));
        let finished2 = finished.clone();
        Self {
            handle: Some(task::spawn(async move {
                let _setflag = zaru::drop::defer(|| finished2.store(true, Ordering::Relaxed));

                // Unwind safety: this is the code that makes it safe.
                AssertUnwindSafe(future).catch_unwind().await
            })),
            finished,
        }
    }

    /// Blocks until the task exits (either successfully or via panic) and returns the produced
    /// value.
    ///
    /// If the task panicked, this will propagate the panic to the caller.
    pub fn block(mut self) -> T {
        match task::block_on(self.handle.take().unwrap()) {
            Ok(value) => value,
            Err(payload) => {
                panic::resume_unwind(payload);
            }
        }
    }

    /// Returns a [`bool`] indicating whether the asynchronous computation has finished
    /// (successfully or unsuccessfully with a panic).
    ///
    /// If this returns `true`, calling [`Task::block`] will return immediately (or propagate the
    /// [`Task`]'s panic) instead of blocking.
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // If the task indicates that it has finished, we can block on it without cancellation.
            // Cancellation can discard the task's value if it happens between `finished` getting
            // set and the task actually fully returning.
            // This is mostly useful in tests. Otherwise there's a small window in which a panicking
            // task reports `is_finished` but still won't propagate the panic when dropped.
            if self.is_finished() {
                if let Err(payload) = task::block_on(handle) {
                    if !thread::panicking() {
                        panic::resume_unwind(payload);
                    }
                }
            } else {
                // This calls `block_on`, and may be executed in an async context.
                // However, this will only actually block until the canceled task stops running, and if
                // that task blocks, that's a separate issue.
                if let Some(Err(payload)) = task::block_on(handle.cancel()) {
                    if !thread::panicking() {
                        panic::resume_unwind(payload);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        panic::{catch_unwind, resume_unwind},
        thread,
    };

    use super::*;

    fn silent_panic(payload: String) {
        resume_unwind(Box::new(payload));
    }

    #[test]
    fn block() {
        let task = Task::spawn(futures::future::ready(123));
        assert_eq!(task.block(), 123);
    }

    #[test]
    fn sets_finished_flag() {
        let task = Task::spawn(futures::future::ready(456));
        while !task.is_finished() {
            thread::yield_now();
        }
        assert_eq!(task.block(), 456);
    }

    #[test]
    fn propagates_panic_on_block() {
        let task = Task::spawn(async {
            silent_panic("task panic 123".into());
        });
        let payload = catch_unwind(|| task.block()).unwrap_err();
        let msg = payload
            .downcast::<String>()
            .expect("panic payload should be a `String`");
        assert!(msg.contains("task panic 123"));
    }

    #[test]
    fn propagates_panic_on_drop() {
        let task = Task::spawn(async {
            silent_panic("task panic 456".into());
        });
        // Make sure the task has finished before we check for panic. If it is canceled before it
        // can panic, the test will (correctly) fail.
        while !task.is_finished() {
            thread::yield_now();
        }

        let payload = catch_unwind(|| drop(task)).unwrap_err();
        let msg = payload
            .downcast::<String>()
            .expect("panic payload should be a `String`");
        assert!(msg.contains("task panic 456"));
    }

    #[test]
    fn task_is_send_sync() {
        fn check<T: Send + Sync>() {}
        check::<Task<()>>();
    }
}
