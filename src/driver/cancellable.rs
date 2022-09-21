use std::ops::Deref;
use std::rc::Rc;
use std::sync::Mutex;

/// TODO comment
pub(crate) enum Index {
    Base(usize),
    Latched(Handle),
    Taken,
}

/// TODO comment
impl Index {
    /// TODO comment
    pub(crate) fn new(index: usize) -> Self {
        Index::Base(index)
    }

    /// Get the index without changing anything about the type.
    pub(crate) fn index(&self) -> Option<usize> {
        match self {
            Index::Base(index) => Some(*index),
            Index::Latched(handle) => handle.index(),
            Index::Taken => None,
        }
    }

    pub(crate) fn was_canceled(&self) -> bool {
        match self {
            Index::Base(_) => false,
            Index::Latched(handle) => handle.was_canceled(),
            Index::Taken => false,
        }
    }

    pub(crate) fn clear_was_canceled(&self) {
        match self {
            Index::Base(_) => {}
            Index::Latched(handle) => handle.clear_was_canceled(),
            Index::Taken => {}
        }
    }

    /// Take the index with the intention of removing the item from the slab.
    pub(crate) fn take_index(&mut self) -> Option<usize> {
        match self {
            Index::Base(index) => {
                let index = *index;
                *self = Index::Taken;
                Some(index)
            }
            Index::Latched(handle) => handle.take_index(),
            Index::Taken => None,
        }
    }

    /// TODO comment
    pub(crate) fn cancel_handle(&mut self) -> Handle {
        match self {
            Index::Base(index) => {
                let handle = Handle::new_index(*index);
                *self = Index::Latched(handle.clone());
                handle
            }

            Index::Latched(handle) => {
                // When we already have a latched handle, simply return a clone of it.
                handle.clone()
            }
            Index::Taken => {
                // When the op is already done and only now a cancel_handle is requested, create
                // one but create it already in the done state and switch ourself to it to avoid
                // allocating more if cancel_handle is called again.
                let handle = Handle::new_done();
                *self = Index::Latched(handle.clone());
                handle
            }
        }
    }
}

/// TODO comment
#[derive(Clone, Debug)]
pub struct Handle(Rc<Mutex<Option<(usize, bool)>>>);

impl Handle {
    /// TODO comment
    fn new_index(index: usize) -> Self {
        Self(Rc::new(Mutex::new(Some((index, false)))))
    }

    /// TODO comment
    fn new_done() -> Self {
        Self(Rc::new(Mutex::new(None)))
    }

    /// TODO comment
    fn index(&self) -> Option<usize> {
        let guard = self.0.lock().unwrap();
        guard.deref().as_ref().map(|(index, _)| *index)
    }

    /// Called from the cancel flow. Provide the slab index of the operation to be canceled
    /// and mark this Handle has having been used to cancel the option. That mark can then
    /// be used by poll_next to filter out the ECANCELED error and return a successful result of 0
    /// ... Wait a minute - that should not be returned by the stream.
    ///
    /// TODO WIP
    fn index_to_cancel(&mut self) -> Option<usize> {
        let mut guard = self.0.lock().unwrap();
        match guard.deref() {
            Some((index, _)) => {
                let index = *index;
                *guard = Some((index, true)); // mark has having been canceled
                Some(index)
            }
            None => None,
        }
    }

    /// TODO comment
    fn was_canceled(&self) -> bool {
        let guard = self.0.lock().unwrap();
        match guard.deref() {
            Some((_, canceled)) => *canceled,
            None => false,
        }
    }

    /// TODO comment
    fn clear_was_canceled(&self) {
        let mut guard = self.0.lock().unwrap();
        match guard.deref() {
            Some((index, _)) => {
                *guard = Some((*index, false)); // mark as no longer having been canceled
            }
            None => {}
        }
    }

    /// TODO comment
    fn take_index(&mut self) -> Option<usize> {
        let mut guard = self.0.lock().unwrap();
        match guard.deref() {
            Some((index, _)) => {
                let index = *index;
                *guard = None;
                Some(index)
            }
            None => None,
        }
    }

    /// TODO comment
    pub async fn async_cancel(&mut self) {
        println!("async_cancel called on {:?}", self);
        match self.index_to_cancel() {
            Some(index) => {
                let op = Op::async_cancel(index).unwrap(); // TODO don't expect an error
                let completion = op.await;
                println!("async_cancel completion {:?}", completion);
                // Ignore completion.
            }
            None => (),
        }
    }
}

// TODO improve comment
// Stuff for the AsyncCancel operation, which for now, does not need to be made public because the
// slab index hasn't been made public. In the future, there could be an AsyncCancel operation
// defined for file descriptors and since file desciptors are made public, there could be a reason
// for supporting that publicly.

use crate::driver::Op;
use std::io;

#[derive(Debug)]
pub(crate) struct AsyncCancel {
    index: usize,
}

impl Op<AsyncCancel> {
    pub(crate) fn async_cancel(index: usize) -> io::Result<Op<AsyncCancel>> {
        use io_uring::opcode;

        Op::submit_with(AsyncCancel { index }, |ac| {
            let index = ac.index;

            opcode::AsyncCancel::new(index as _).build()
        })
    }
}
