use crate::driver::Op;

use std::io;
use std::os::unix::io::RawFd;

#[allow(dead_code)]
pub(crate) struct Close {
    fd: RawFd,
}

#[allow(dead_code)]
impl Op<Close> {
    pub(crate) fn close(fd: RawFd) -> io::Result<Op<Close>> {
        use io_uring::{opcode, types};

        Op::try_submit_with(Close { fd }, |close| {
            opcode::Close::new(types::Fd(close.fd)).build()
        })
    }
}
