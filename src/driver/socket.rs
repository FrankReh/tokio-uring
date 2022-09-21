use crate::driver::cancellable;
use crate::{
    buf::{IoBuf, IoBufMut},
    driver::{Op, SharedFd, StrOp},
};
use futures_core::stream::Stream;
use std::{
    io,
    net::SocketAddr,
    os::unix::io::{AsRawFd, IntoRawFd, RawFd},
    path::Path,
};

#[derive(Clone)]
pub(crate) struct Socket {
    /// Open file descriptor
    fd: SharedFd,
}

pub(crate) fn get_domain(socket_addr: SocketAddr) -> libc::c_int {
    match socket_addr {
        SocketAddr::V4(_) => libc::AF_INET,
        SocketAddr::V6(_) => libc::AF_INET6,
    }
}

impl Socket {
    pub(crate) fn new(socket_addr: SocketAddr, socket_type: libc::c_int) -> io::Result<Socket> {
        let socket_type = socket_type | libc::SOCK_CLOEXEC;
        let domain = get_domain(socket_addr);
        let fd = socket2::Socket::new(domain.into(), socket_type.into(), None)?.into_raw_fd();
        let fd = SharedFd::new(fd);
        Ok(Socket { fd })
    }

    pub(crate) fn new_unix(socket_type: libc::c_int) -> io::Result<Socket> {
        let socket_type = socket_type | libc::SOCK_CLOEXEC;
        let domain = libc::AF_UNIX;
        let fd = socket2::Socket::new(domain.into(), socket_type.into(), None)?.into_raw_fd();
        let fd = SharedFd::new(fd);
        Ok(Socket { fd })
    }

    pub(crate) async fn write<T: IoBuf>(&self, buf: T) -> crate::BufResult<usize, T> {
        let op = Op::write_at(&self.fd, buf, 0).unwrap();
        op.write().await
    }

    pub(crate) async fn send_to<T: IoBuf>(
        &self,
        buf: T,
        socket_addr: SocketAddr,
    ) -> crate::BufResult<usize, T> {
        let op = Op::send_to(&self.fd, buf, socket_addr).unwrap();
        op.send().await
    }

    pub(crate) async fn read<T: IoBufMut>(&self, buf: T) -> crate::BufResult<usize, T> {
        let op = Op::read_at(&self.fd, buf, 0).unwrap();
        op.read().await
    }

    pub(crate) async fn recv_from<T: IoBufMut>(
        &self,
        buf: T,
    ) -> crate::BufResult<(usize, SocketAddr), T> {
        let op = Op::recv_from(&self.fd, buf).unwrap();
        op.recv().await
    }

    pub(crate) async fn accept(&self) -> io::Result<(Socket, Option<SocketAddr>)> {
        let op = Op::accept(&self.fd)?;
        let completion = op.await;
        let fd = completion.result?;
        let fd = SharedFd::new(fd as i32);
        let data = completion.data;
        let socket = Socket { fd };
        let (_, addr) = unsafe {
            socket2::SockAddr::init(move |addr_storage, len| {
                *addr_storage = data.socketaddr.0.to_owned();
                *len = data.socketaddr.1;
                Ok(())
            })?
        };
        Ok((socket, addr.as_socket()))
    }

    // Create an async stream of sockets and optional addresses from the open file descriptor.
    // TODO
    pub(crate) fn accept_multishot(
        &self,
    ) -> io::Result<(
        impl Stream<Item = io::Result<(Socket, Option<SocketAddr>)>> + '_,
        cancellable::Handle,
    )> {
        use async_stream::try_stream;
        use tokio_stream::StreamExt;

        // TODO what are the semantics of async cancel for a stream?
        // If the StrOp is only created when the stream is awaited, then there is no index to use
        // for the cancel id beforehand. If a stream can only be canceled after its started its
        // 'next' loop, how to get at the cancel id?
        // Should it be cancellable even if the operation hasn't been sent yet?
        //let mut op = StrOp::accept_multishot(&self.fd)?; // TODO this step could have failed - the submission step could have failed. Wrong args or feature not supported.
        let mut op = StrOp::accept_multishot(&self.fd)?;
        let cancel = op.cancel_handle();
        Ok((
            try_stream! {
                while let Some(completion) = op.next().await {
                    // TODO this shouldn't quit the stream loop.
                    // Maybe this shouldn't be a try?
                    let fd = completion.result?;
                    let fd = SharedFd::new(fd as i32);
                    let data = completion.data;
                    let socket = Socket { fd };
                    let (_, addr) = unsafe {
                        socket2::SockAddr::init(move |addr_storage, len| {
                            *addr_storage = data.socketaddr.0.to_owned();
                            *len = data.socketaddr.1;
                            Ok(())
                        })?
                    };
                    yield (socket, addr.as_socket());
                }
            },
            cancel,
        ))
    }

    // Create an async stream of sockets and optional addresses from the open file descriptor. This
    // is the poor man's version, where the original, single, `accept` operation is used in a loop.
    // This allows support of a stream for kernels that don't support the `multishot` versions of
    // the operation yet.
    pub(crate) fn accept_multishot_slow(
        &self,
    ) -> impl Stream<Item = io::Result<(Socket, Option<SocketAddr>)>> + '_ {
        use async_stream::try_stream;

        try_stream! {
            loop {
                let op = Op::accept(&self.fd)?;
                let completion = op.await;
                let fd = completion.result?;
                let fd = SharedFd::new(fd as i32);
                let data = completion.data;
                let socket = Socket { fd };
                let (_, addr) = unsafe {
                    socket2::SockAddr::init(move |addr_storage, len| {
                        *addr_storage = data.socketaddr.0.to_owned();
                        *len = data.socketaddr.1;
                        Ok(())
                    })?
                };
                yield (socket, addr.as_socket());
            }
        }
    }

    pub(crate) async fn connect(&self, socket_addr: socket2::SockAddr) -> io::Result<()> {
        let op = Op::connect(&self.fd, socket_addr)?;
        let completion = op.await;
        completion.result?;
        Ok(())
    }

    pub(crate) fn bind(socket_addr: SocketAddr, socket_type: libc::c_int) -> io::Result<Socket> {
        Self::bind_internal(
            socket_addr.into(),
            get_domain(socket_addr).into(),
            socket_type.into(),
        )
    }

    pub(crate) fn bind_unix<P: AsRef<Path>>(
        path: P,
        socket_type: libc::c_int,
    ) -> io::Result<Socket> {
        let addr = socket2::SockAddr::unix(path.as_ref())?;
        Self::bind_internal(addr, libc::AF_UNIX.into(), socket_type.into())
    }

    pub(crate) fn from_std(socket: std::net::UdpSocket) -> Socket {
        let fd = SharedFd::new(socket.into_raw_fd());
        Self { fd }
    }

    fn bind_internal(
        socket_addr: socket2::SockAddr,
        domain: socket2::Domain,
        socket_type: socket2::Type,
    ) -> io::Result<Socket> {
        let sys_listener = socket2::Socket::new(domain, socket_type, None)?;

        sys_listener.set_reuse_port(true)?;
        sys_listener.set_reuse_address(true)?;

        // TODO: config for buffer sizes
        // sys_listener.set_send_buffer_size(send_buf_size)?;
        // sys_listener.set_recv_buffer_size(recv_buf_size)?;

        sys_listener.bind(&socket_addr)?;

        let fd = SharedFd::new(sys_listener.into_raw_fd());

        Ok(Self { fd })
    }

    pub(crate) fn listen(&self, backlog: libc::c_int) -> io::Result<()> {
        syscall!(listen(self.as_raw_fd(), backlog))?;
        Ok(())
    }
}

impl AsRawFd for Socket {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.raw_fd()
    }
}
