// Internal data structures shared between thread-local and thread-safe
// fixed buffer collections.

mod pool;
pub(super) use pool::Pool;
pub(super) use pool::Waiter;

mod registry;
pub(super) use registry::Registry;
