//! Provides a slot through which values may be exchanged asynchronously.
//!
//! A slot is similar to a channel with capacity 1, except that sending something will replace the
//! value currently in the channel. This behavior is useful when the receiver is interested only in
//! the latest value.
//!
//! Slots consists of a [`SlotWriter`] that can update the value in the slot, and one or more
//! [`SlotReader`]s that can read the slot's value, check if a new value has been written, and block
//! (synchronously or asynchronously) until a new value is available. A connected pair of
//! [`SlotWriter`] and [`SlotReader`] can be created by calling [`slot()`].
//!
//! Slots are meant to facilitate the communication between an asynchronous task and a non-async
//! thread: an async task can call [`SlotWriter::update`] anytime without blocking, and the
//! non-async thread can check or block for a new value using the [`SlotReader`]'s methods.
//! Communication in the other direction is enabled by the async [`SlotReader::wait`] method.

use std::sync::{Arc, Condvar, Mutex};

use async_std::task;

/// The writing end of a slot.
///
/// See [`SlotWriter::update`].
pub struct SlotWriter<T>(Arc<Slot<T>>);

/// The reading end of a slot.
///
/// [`SlotReader`]s can be cloned, and each clone will track the read status separately. This means
/// that every reader can wait for a new message without affecting other readers.
pub struct SlotReader<T> {
    slot: Arc<Slot<T>>,
    /// Last read generation. Initially `!0`. Copied from the last written generation on every read.
    read_gen: u64,
}

impl<T> Clone for SlotReader<T> {
    fn clone(&self) -> Self {
        Self {
            slot: self.slot.clone(),
            read_gen: self.read_gen,
        }
    }
}

/// Creates a pair of [`SlotWriter`] and [`SlotReader`] accessing the same slot.
///
/// The [`SlotReader`] can be cloned to allow several tasks or threads to access the same slot.
pub fn slot<T>() -> (SlotWriter<T>, SlotReader<T>) {
    let slot = Arc::new(Slot::default());
    (SlotWriter(slot.clone()), SlotReader { slot, read_gen: !0 })
}

struct Slot<T> {
    data: Mutex<SlotData<T>>,
    condvar: Condvar,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            data: Mutex::new(SlotData {
                value: None,
                write_gen: 0,
                disconnected: false,
            }),
            condvar: Condvar::new(),
        }
    }
}

struct SlotData<T> {
    value: Option<T>,
    /// Last written generation.
    write_gen: u64,
    disconnected: bool,
}

impl<T> SlotWriter<T> {
    /// Updates the [`Slot`]'s value.
    ///
    /// This notifies all [`SlotReader`]s of this slot that are currently waiting on a new value.
    ///
    /// This operation does not block (but it may acquire a mutex internally). If the previous value
    /// has not been read yet, it will be overwritten with the new value. It is safe to call this
    /// method from an async task.
    pub fn update(&mut self, value: T) {
        let mut guard = self.0.data.lock().unwrap();
        guard.value = Some(value);
        guard.write_gen += 1;
        drop(guard);
        self.0.condvar.notify_all();
    }
}

impl<T> Drop for SlotWriter<T> {
    fn drop(&mut self) {
        self.0.data.lock().unwrap().disconnected = true;
        self.0.condvar.notify_all();
    }
}

impl<T: Clone> SlotReader<T> {
    /// Retrieves the most recent value written to the slot.
    ///
    /// Returns [`None`] if no value has ever been written to the slot.
    pub fn get(&mut self) -> Option<T> {
        let guard = self.slot.data.lock().unwrap();
        match &guard.value {
            Some(value) => {
                self.read_gen = guard.write_gen;
                Some(value.clone())
            }
            None => None,
        }
    }

    /// Retrieves the next value written to the slot.
    ///
    /// If no value was written since the last time a value was retrieved from this [`SlotReader`],
    /// this function returns [`None`]. If you want to access the last value regardless, call
    /// [`SlotReader::get`] instead.
    pub fn next(&mut self) -> Option<T> {
        let guard = self.slot.data.lock().unwrap();
        match &guard.value {
            Some(value) if guard.write_gen != self.read_gen => {
                self.read_gen = guard.write_gen;
                Some(value.clone())
            }
            _ => None,
        }
    }

    /// Blocks the calling thread until a new value is available, and returns that value.
    ///
    /// If the connected [`SlotWriter`] has been dropped, or is dropped while blocking, a
    /// [`Disconnected`] error is returned.
    pub fn block(&mut self) -> Result<T, Disconnected> {
        let mut guard = self.slot.data.lock().unwrap();
        loop {
            if guard.disconnected {
                return Err(Disconnected);
            }
            match &guard.value {
                Some(value) if guard.write_gen != self.read_gen => {
                    self.read_gen = guard.write_gen;
                    return Ok(value.clone());
                }
                _ => {}
            }
            guard = self.slot.condvar.wait(guard).unwrap();
        }
    }

    /// Asynchronously waits until a new value is available, and returns that value.
    pub async fn wait(&mut self) -> Result<T, Disconnected>
    where
        T: Send + 'static,
    {
        // Clone to work around lack of `block_in_place` in async-std. Copy the `read_gen` back to
        // `self` to make the clone unobservable.
        let mut this = self.clone();
        let (result, read_gen) = task::spawn_blocking(move || (this.block(), this.read_gen)).await;
        self.read_gen = read_gen;
        result
    }

    /// Returns a [`bool`] indicating whether the corresponding [`SlotWriter`] has been dropped.
    pub fn is_disconnected(&self) -> bool {
        self.slot.data.lock().unwrap().disconnected
    }
}

/// An error that indicates that the [`SlotWriter`] connected to a [`SlotReader`] has been dropped.
///
/// This type deliberately does not implement the [`std::error::Error`] trait. It cannot convey any
/// useful information about *why* the [`SlotWriter`] was dropped, so the caller has to determine a
/// root cause or recover from the error.
#[derive(Debug, PartialEq, Eq)]
pub struct Disconnected;

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    #[test]
    fn slot_exchange() {
        let (mut w, mut r) = slot();
        assert!(!r.is_disconnected());
        assert!(r.get().is_none());
        assert!(r.next().is_none());
        w.update(123);
        assert_eq!(r.next(), Some(123));
        assert!(r.next().is_none());
        assert_eq!(r.get(), Some(123));
        assert!(r.next().is_none());
        assert!(!r.is_disconnected());
        drop(w);
        assert!(r.is_disconnected());
        assert_eq!(r.block(), Err(Disconnected));
        assert_eq!(r.block(), Err(Disconnected));
    }

    #[test]
    fn block() {
        let (mut w, mut r) = slot();
        let handle = thread::spawn(move || {
            w.update(456);
            thread::park();
        });
        assert_eq!(r.block(), Ok(456));
        assert!(!r.is_disconnected());
        handle.thread().unpark();
        assert_eq!(r.block(), Err(Disconnected));
        assert!(r.is_disconnected());
        assert_eq!(r.block(), Err(Disconnected));
        assert!(r.is_disconnected());
    }

    #[test]
    fn async_wait() {
        let (mut w, mut r) = slot();
        let handle = thread::spawn(move || {
            w.update(456);
            thread::park();
        });
        assert_eq!(task::block_on(r.wait()), Ok(456));
        handle.thread().unpark();
        assert_eq!(task::block_on(r.wait()), Err(Disconnected));
        assert_eq!(r.block(), Err(Disconnected));
    }

    #[test]
    fn clone() {
        let (mut w, mut r) = slot();
        w.update(123);
        let mut r2 = r.clone();
        assert_eq!(r.block(), Ok(123));
        assert_eq!(r2.block(), Ok(123));
    }
}
