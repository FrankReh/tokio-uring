use crate::driver::{Driver, CURRENT};
use std::cell::RefCell;

use std::future::Future;
use std::io;
use tokio::io::unix::AsyncFd;
use tokio::task::LocalSet;

pub(crate) struct Runtime {
    /// io-uring driver
    driver: AsyncFd<Driver>,

    /// LocalSet for !Send tasks
    local: LocalSet,

    /// Tokio runtime, always current-thread
    rt: tokio::runtime::Runtime,
}

/// Spawns a new asynchronous task, returning a [`JoinHandle`] for it.
///
/// Spawning a task enables the task to execute concurrently to other tasks.
/// There is no guarantee that a spawned task will execute to completion. When a
/// runtime is shutdown, all outstanding tasks are dropped, regardless of the
/// lifecycle of that task.
///
/// This function must be called from the context of a `tokio-uring` runtime.
///
/// [`JoinHandle`]: tokio::task::JoinHandle
///
/// # Examples
///
/// In this example, a server is started and `spawn` is used to start a new task
/// that processes each received connection.
///
/// ```no_run
/// tokio_uring::start(async {
///     let handle = tokio_uring::spawn(async {
///         println!("hello from a background task");
///     });
///
///     // Let the task complete
///     handle.await.unwrap();
/// });
/// ```
pub fn spawn<T: std::future::Future + 'static>(task: T) -> tokio::task::JoinHandle<T::Output> {
    tokio::task::spawn_local(task)
}

/* TODO
use std::io::Write; // bring Write into scope for flush.
*/

impl Runtime {
    pub(crate) fn new(b: &crate::Builder) -> io::Result<Runtime> {
        let rt = tokio::runtime::Builder::new_current_thread()
            /* TODO
            .on_thread_unpark(|| {
                print!("<");
                std::io::stdout().flush().unwrap();
            })
             */
            .on_thread_park(|| {
                CURRENT.with(|x| {
                    if let Err(e) = RefCell::borrow_mut(x).submit2() {
                        panic!(
                            //"within the on_thread_park callback, uring.submit returned {}",
                            "within the on_thread_park callback, submit2 returned {}",
                            e
                        );
                    }
                    /* TODO
                    print!(">");
                    std::io::stdout().flush().unwrap();
                     */
                });
            })
            .enable_all()
            .build()?;

        let local = LocalSet::new();

        let driver = {
            let _guard = rt.enter();
            AsyncFd::new(Driver::new(b)?)?
        };

        Ok(Runtime { driver, local, rt })
    }

    pub(crate) fn block_on<F>(&mut self, future: F) -> F::Output
    where
        F: Future,
    {
        self.driver.get_ref().with(|| {
            let drive = async {
                // TODO pp("A");
                loop {
                    // TODO pp("B");
                    // Wait for read-readiness
                    let mut guard = self.driver.readable().await.unwrap();
                    // TODO pp("C");
                    self.driver.get_ref().tick();
                    // TODO pp("D");
                    guard.clear_ready();
                    // TODO pp("E");
                }
            };

            tokio::pin!(drive);
            tokio::pin!(future);

            self.rt
                .block_on(self.local.run_until(crate::future::poll_fn(|cx| {
                    // TODO pp("x");
                    assert!(drive.as_mut().poll(cx).is_pending());
                    // TODO pp("y");
                    future.as_mut().poll(cx)
                })))
        })
    }
}
/* TODO
fn pp(s: &str) {
    print!("{}", s);
    std::io::stdout().flush().unwrap();
}
*/
