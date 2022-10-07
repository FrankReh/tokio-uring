/* TODO
 * refer to reason the Closing variant was removed below.
use crate::driver::{Close, Op};
 */
use crate::future::poll_fn;

use std::cell::RefCell;
use std::os::unix::io::{FromRawFd, RawFd};
use std::rc::Rc;
use std::task::Waker;

// Tracks in-flight operations on a file descriptor. Ensures all in-flight
// operations complete before submitting the close.
#[derive(Clone)]
pub(crate) struct SharedFd {
    inner: Rc<Inner>,
}

struct Inner {
    // Open file descriptor
    fd: RawFd,

    // Waker to notify when the close operation completes.
    state: RefCell<State>,
}

enum State {
    /// Initial state
    Init,

    /// Waiting for all in-flight operation to complete.
    Waiting(Option<Waker>),

    /* TODO
     * Decide if this variant gets used again if we find a way to issue the Close while on the same
     * stack as the driver's tick call - both attempt a borrow_mut.
    /// The FD is closing
    Closing(Op<Close>),
    */
    /// The FD is fully closed
    Closed,
}

impl SharedFd {
    pub(crate) fn new(fd: RawFd) -> SharedFd {
        SharedFd {
            inner: Rc::new(Inner {
                fd,
                state: RefCell::new(State::Init),
            }),
        }
    }

    /// Returns the RawFd
    pub(crate) fn raw_fd(&self) -> RawFd {
        self.inner.fd
    }

    /// An FD cannot be closed until all in-flight operation have completed.
    /// This prevents bugs where in-flight reads could operate on the incorrect
    /// file descriptor.
    ///
    /// TO model this, if there are no in-flight operations, then
    pub(crate) async fn close(mut self) {
        // Get a mutable reference to Inner, indicating there are no
        // in-flight operations on the FD.
        if let Some(inner) = Rc::get_mut(&mut self.inner) {
            // Submit the close operation
            inner.submit_close_op();
        }

        self.inner.closed().await;
    }
}

impl Inner {
    /// If there are no in-flight operations, submit the operation.
    fn submit_close_op(&mut self) {
        // Close the FD
        let state = RefCell::get_mut(&mut self.state);

        // TODO calling Op::close seems to be problematic because the ring driver becoming readable
        // triggers a borrow_mut call on the driver's handle so it can call tick(),
        // but tick reads the cq and calls complete on any cqe found, and a complete can result in
        // an ignored boxed SharedFd being dropped, and the drop calls submit_close_op and that
        // wants to create an Op with try_submit_with which only verifies that CURRENT is set
        // but doesn't see the mutable borrow already taking place so it calls Op::submit_with
        // and the first thing that does is call borrow_mut on the CURRENT driver.
        //
        // Dropping a close instruction into the sq ring is certainly faster than making the close
        // system call, but the close system call can be done without trying to borrow the ring
        // driver. Seems to be a rub between calling tick having mutably borrowed the driver
        // and wanting to submit operations to the driver by first mutably borrowing the driver.

        *state = State::Closed;
        let _ = unsafe { std::fs::File::from_raw_fd(self.fd) };
        /*

        // Submit a close operation
        *state = match Op::close(self.fd) {
            Ok(op) => State::Closing(op),
            Err(_) => {
                // Submitting the operation failed, we fall back on a
                // synchronous `close`. This is safe as, at this point, we
                // guarantee all in-flight operations have completed. The most
                // common cause for an error is attempting to close the FD while
                // off runtime.
                //
                // This is done by initializing a `File` with the FD and
                // dropping it.
                //
                // TODO: Should we warn?
                let _ = unsafe { std::fs::File::from_raw_fd(self.fd) };

                State::Closed
            }
        };
         */
    }

    /// Completes when the FD has been closed.
    async fn closed(&self) {
        /* TODO
         * refer to reason the Closing variant was removed above.
        use std::future::Future;
        use std::pin::Pin;
         */
        use std::task::Poll;

        poll_fn(|cx| {
            let mut state = self.state.borrow_mut();

            match &mut *state {
                State::Init => {
                    *state = State::Waiting(Some(cx.waker().clone()));
                    Poll::Pending
                }
                State::Waiting(Some(waker)) => {
                    if !waker.will_wake(cx.waker()) {
                        *waker = cx.waker().clone();
                    }

                    Poll::Pending
                }
                State::Waiting(None) => {
                    *state = State::Waiting(Some(cx.waker().clone()));
                    Poll::Pending
                }
                /* TODO
                 * refer to reason this variant was removed above.
                State::Closing(op) => {
                    // Nothing to do if the close opeation failed.
                    let _ = ready!(Pin::new(op).poll(cx));
                    *state = State::Closed;
                    Poll::Ready(())
                }
                */
                State::Closed => Poll::Ready(()),
            }
        })
        .await;
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Submit the close operation, if needed
        match RefCell::get_mut(&mut self.state) {
            State::Init | State::Waiting(..) => {
                self.submit_close_op();
            }
            _ => {}
        }
    }
}
