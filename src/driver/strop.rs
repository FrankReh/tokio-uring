use std::cell::RefCell;
use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use tokio_stream::Stream;

use io_uring::squeue;

use crate::driver;

/// In-flight operation
pub(crate) struct StrOp<T: 'static + Clone> {
    // Driver running the operation
    pub(super) driver: Rc<RefCell<driver::Inner>>,

    // Operation index in the slab
    pub(super) index: usize,

    // Per-operation data
    data: Option<T>,
}

/// Operation completion. Returns stored state with the result of the operation.
#[derive(Debug)]
pub(crate) struct Completion<T> {
    pub(crate) data: T,
    pub(crate) result: io::Result<u32>,
    // the field is currently only read in tests
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) flags: u32,
}

/// The ReadyFifo tracks what has been read out of the cq but not yet served through the stream.
/// That is, this queue tracks the data that has been returned by the kernel but which is still
/// waiting for the stream to be polled. The stream's poll_next will pick out one result at a time
/// from thie queue. When the stream's poll_next finds this queue empty, the Lifecycle is
/// transitioned to Waiting.
///
/// The ReadyFifo is carried across into the Waiting Lifecycle, even though the invariant
/// guarantees it is empty at that time, to avoid reallocations by the VecDeque structure for cases
/// where polling sometimes doesn't keep up with data being received. The memory allocated by the
/// VecDeque will be freed when the multishot operation is completely done.
///
/// Also of note, the growth of the VecDeque is unbounded in this module. io_uring multishot
/// operations currently rely on a fixed size file descriptor table (for the multishot accept
/// operation) or on a fixed size set of buffers (for the multishot recv operations) and the kernel
/// will build up its own queue of stalled operations if the fd table or the buffers are exhausted.
/// So backpressure is expected to be enforced through the sizing of the fd table and the provided
/// buffers. A future improvement could check the VecDeque capacity when transitioning from
/// Submitted to Waiting and drop the empty queue in favor or a new one with zero capacity to start
/// if memory reclamation was deemed important.
type ReadyFifo = VecDeque<(io::Result<u32>, u32)>;

pub(crate) enum Lifecycle {
    /// The operation has been submitted to uring and is currently in-flight
    Submitted(ReadyFifo),

    /// The submitter is waiting for the completion of the operation
    Waiting(ReadyFifo, Waker),

    /// The submitter no longer has interest in the operation result. The state
    /// must be passed to the driver and held until the operation completes.
    Ignored(Box<dyn std::any::Any>),

    /// The operation has produced results, either 'more' results or the final result.
    Completed(ReadyFifo),
}

impl<T: Clone> StrOp<T> {
    /// Create a new operation
    fn new(data: T, inner: &mut driver::Inner, inner_rc: &Rc<RefCell<driver::Inner>>) -> StrOp<T> {
        StrOp {
            driver: inner_rc.clone(),
            index: inner.ops.insert_multishot(),
            data: Some(data),
        }
    }

    /// Submit an operation to uring.
    ///
    /// `state` is stored during the operation tracking any state submitted to
    /// the kernel.
    pub(super) fn submit_with<F>(data: T, f: F) -> io::Result<StrOp<T>> // TODO Could get rid of this io::Result
    where
        F: FnOnce(&mut T) -> squeue::Entry,
    {
        driver::CURRENT.with(|inner_rc| {
            let mut inner_ref = inner_rc.borrow_mut();
            let inner = &mut *inner_ref;

            // If the submission queue is full, flush it to the kernel
            if inner.uring.submission().is_full() {
                if let Err(e) = inner.submit() {
                    panic!("while submission was found full, inner.submit returned {}", e);
                }
            }

            // Create the operation
            let mut op = StrOp::new(data, inner, inner_rc);

            // Configure the SQE
            let sqe = f(op.data.as_mut().unwrap()).user_data(op.index as _);

            {
                let mut sq = inner.uring.submission();

                // Push the new operation
                if unsafe { sq.push(&sqe).is_err() } {
                    unimplemented!("when is this hit?");
                }
            }

            Ok(op)
        })
    }
}

impl<T: Clone> Stream for StrOp<T>
where
    T: Unpin + 'static,
{
    type Item = Completion<T>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        use std::mem;

        let me = &mut *self;
        let mut inner = me.driver.borrow_mut();
        let lifecycle = inner.ops.get_mut(me.index).expect("invalid internal state");

        let lifecycle = match lifecycle {
            driver::Split::Single(_) => panic!("expected multishot op, got single shot op"),
            driver::Split::Multi(lifecycle) => lifecycle,
        };

        match mem::replace(lifecycle, Lifecycle::Submitted(Default::default())) {
            Lifecycle::Submitted(mut ready) => {
                // If ready is empty, transition lifecycle to waiting with a waker,
                // otherwise return front as next stream item and keep lifecycle at Submitted.
                match ready.pop_front() {
                    None => {
                        *lifecycle = Lifecycle::Waiting(ready, cx.waker().clone());
                        Poll::Pending
                    }
                    Some(front) => {
                        *lifecycle = Lifecycle::Submitted(ready);
                        Poll::Ready(Some(Completion {
                            data: me
                                .data
                                .as_mut()
                                .cloned()
                                .take()
                                .expect("unexpected operation state"),
                            result: front.0,
                            flags: front.1,
                        }))
                    }
                }
            }
            Lifecycle::Waiting(ready, waker) if !waker.will_wake(cx.waker()) => {
                *lifecycle = Lifecycle::Waiting(ready, cx.waker().clone());
                Poll::Pending
            }
            Lifecycle::Waiting(ready, waker) => {
                *lifecycle = Lifecycle::Waiting(ready, waker);
                Poll::Pending
            }
            Lifecycle::Ignored(..) => unreachable!(),
            Lifecycle::Completed(mut ready) => {
                // If ready is empty, cleanup index and indicate to stream we are done,
                // otherwise return front as next stream item and keep lifecycle at Completed.
                match ready.pop_front() {
                    None => {
                        inner.ops.remove(me.index);
                        me.index = usize::MAX;
                        Poll::Ready(None)
                    }
                    Some(front) => {
                        *lifecycle = Lifecycle::Completed(ready);
                        Poll::Ready(Some(Completion {
                            data: me
                                .data
                                .as_mut()
                                .cloned()
                                .take()
                                .expect("unexpected operation state"),
                            result: front.0,
                            flags: front.1,
                        }))
                    }
                }
            }
        }
    }
}

impl<T: Clone> Drop for StrOp<T> {
    fn drop(&mut self) {
        let mut inner = self.driver.borrow_mut();
        let lifecycle = match inner.ops.get_mut(self.index) {
            Some(lifecycle) => lifecycle,
            None => return,
        };

        let lifecycle = match lifecycle {
            driver::Split::Single(_) => panic!("expected multishot op, got single shot op"),
            driver::Split::Multi(lifecycle) => lifecycle,
        };

        match lifecycle {
            Lifecycle::Submitted(_) | Lifecycle::Waiting(_, _) => {
                *lifecycle = Lifecycle::Ignored(Box::new(self.data.take()));
            }
            Lifecycle::Completed(_) => {
                inner.ops.remove(self.index);
            }
            Lifecycle::Ignored(..) => unreachable!(),
        }
    }
}

impl Lifecycle {
    // complete is called when a completion queue entry has been read.
    // Returns true, indicating the lifecycle was at ignored state.
    pub(super) fn complete(&mut self, result: io::Result<u32>, flags: u32) -> bool {
        // Check flags to see if there is more.
        const MORE: u32 = io_uring::sys::IORING_CQE_F_MORE;
        if (flags & MORE) != 0 {
            self.result_more(result, flags & !MORE)
        } else {
            self.result_final(result, flags)
        }
    }
    fn result_more(&mut self, result: io::Result<u32>, flags: u32) -> bool {
        // The MORE flag was found, so no state transition to Completed.
        use std::mem;

        match mem::replace(self, Lifecycle::Submitted(Default::default())) {
            Lifecycle::Submitted(mut ready) => {
                ready.push_back((result, flags));
                *self = Lifecycle::Submitted(ready);
                false
            }
            Lifecycle::Waiting(mut ready, waker) => {
                // TODO for debug, could assert ready is empty.
                ready.push_back((result, flags));
                *self = Lifecycle::Submitted(ready);
                waker.wake();
                false
            }
            Lifecycle::Ignored(..) => true,
            Lifecycle::Completed(..) => unreachable!("invalid operation state"), // 'more' shouldn't be possible once completed
        }
    }
    fn result_final(&mut self, result: io::Result<u32>, flags: u32) -> bool {
        // The MORE flag was not found, so all transitions are to Completed.
        use std::mem;

        match mem::replace(self, Lifecycle::Submitted(Default::default())) {
            Lifecycle::Submitted(mut ready) => {
                ready.push_back((result, flags));
                *self = Lifecycle::Completed(ready);
                false
            }
            Lifecycle::Waiting(mut ready, waker) => {
                // TODO for debug, could assert ready is empty.
                ready.push_back((result, flags));
                *self = Lifecycle::Completed(ready);
                waker.wake();
                false
            }
            Lifecycle::Ignored(..) => true,
            Lifecycle::Completed(..) => unreachable!("invalid operation state"), // double completed should not be possible
        }
    }
}

#[cfg(test)]
mod test {
    use std::rc::Rc;

    use tokio_test::{assert_pending, assert_ready, task};

    use super::*;

    #[test]
    fn op_stays_in_slab_on_drop() {
        let (op, driver, data) = init();
        drop(op);

        assert_eq!(2, Rc::strong_count(&data));

        assert_eq!(1, driver.num_operations());
        release(driver);
    }

    #[test]
    fn poll_op_once() {
        let (op, driver, data) = init();
        let mut op = task::spawn(op);
        assert_pending!(op.poll_next());
        assert_eq!(2, Rc::strong_count(&data));

        complete(&op, Ok(1));
        assert_eq!(1, driver.num_operations());
        assert_eq!(2, Rc::strong_count(&data));

        assert!(op.is_woken());
        let Completion {
            result,
            flags,
            data: d,
        } = assert_ready!(op.poll_next()).unwrap();
        assert_eq!(2, Rc::strong_count(&data));
        assert_eq!(1, result.unwrap());
        assert_eq!(0, flags);

        drop(d);
        assert_eq!(1, Rc::strong_count(&data));

        drop(op);
        assert_eq!(0, driver.num_operations());

        release(driver);
    }

    #[test]
    fn poll_op_twice() {
        let (op, driver, ..) = init();
        let mut op = task::spawn(op);
        assert_pending!(op.poll_next());
        assert_pending!(op.poll_next());

        complete(&op, Ok(1));

        assert!(op.is_woken());
        let Completion { result, flags, .. } = assert_ready!(op.poll_next()).unwrap();
        assert_eq!(1, result.unwrap());
        assert_eq!(0, flags);

        release(driver);
    }

    #[test]
    fn poll_change_task() {
        let (op, driver, ..) = init();
        let mut op = task::spawn(op);
        assert_pending!(op.poll_next());

        let op = op.into_inner();
        let mut op = task::spawn(op);
        assert_pending!(op.poll_next());

        complete(&op, Ok(1));

        assert!(op.is_woken());
        let Completion { result, flags, .. } = assert_ready!(op.poll_next()).unwrap();
        assert_eq!(1, result.unwrap());
        assert_eq!(0, flags);

        release(driver);
    }

    #[test]
    fn complete_before_poll_next() {
        let (op, driver, data) = init();
        let mut op = task::spawn(op);
        complete(&op, Ok(1));
        assert_eq!(1, driver.num_operations());
        assert_eq!(2, Rc::strong_count(&data));

        let Completion { result, flags, .. } = assert_ready!(op.poll_next()).unwrap();
        assert_eq!(1, result.unwrap());
        assert_eq!(0, flags);

        drop(op);
        assert_eq!(0, driver.num_operations());

        release(driver);
    }

    #[test]
    fn complete_after_drop() {
        let (op, driver, data) = init();
        let index = op.index;
        drop(op);

        assert_eq!(2, Rc::strong_count(&data));

        assert_eq!(1, driver.num_operations());
        driver.inner.borrow_mut().ops.complete(index, Ok(1), 0);
        assert_eq!(1, Rc::strong_count(&data));
        assert_eq!(0, driver.num_operations());
        release(driver);
    }

    fn init() -> (StrOp<Rc<()>>, crate::driver::Driver, Rc<()>) {
        use crate::driver::Driver;

        let driver = Driver::new().unwrap();
        let handle = driver.inner.clone();
        let data = Rc::new(());

        let op = {
            let mut inner = handle.borrow_mut();
            StrOp::new(data.clone(), &mut inner, &handle)
        };

        (op, driver, data)
    }

    fn complete(op: &StrOp<Rc<()>>, result: io::Result<u32>) {
        op.driver.borrow_mut().ops.complete(op.index, result, 0);
    }

    fn release(driver: crate::driver::Driver) {
        // Clear ops, we aren't really doing any I/O
        driver.inner.borrow_mut().ops.0.clear();
    }
}
