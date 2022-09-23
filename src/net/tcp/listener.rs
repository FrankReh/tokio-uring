use super::TcpStream;
use crate::driver::Socket;
use std::{io, net::SocketAddr};

use crate::driver::cancellable;
use futures_core::Stream;

/// A TCP socket server, listening for connections.
///
/// You can accept a new connection by using the [`accept`](`TcpListener::accept`)
/// method.
///
/// # Examples
///
/// ```
/// use tokio_uring::net::TcpListener;
/// use tokio_uring::net::TcpStream;
///
/// let listener = TcpListener::bind("127.0.0.1:2345".parse().unwrap()).unwrap();
///
/// tokio_uring::start(async move {
///     let (tx_ch, rx_ch) = tokio::sync::oneshot::channel();
///
///     tokio_uring::spawn(async move {
///         let (rx, _) = listener.accept().await.unwrap();
///         if let Err(_) = tx_ch.send(rx) {
///             panic!("The receiver dropped");
///         }
///     });
///     tokio::task::yield_now().await; // Ensure the listener.accept().await has been kicked off.
///
///     let tx = TcpStream::connect("127.0.0.1:2345".parse().unwrap()).await.unwrap();
///     let rx = rx_ch.await.expect("The spawned task expected to send a TcpStream");
///
///     tx.write(b"test" as &'static [u8]).await.0.unwrap();
///
///     let (_, buf) = rx.read(vec![0; 4]).await;
///
///     assert_eq!(buf, b"test");
/// });
/// ```
pub struct TcpListener {
    inner: Socket,
}

impl TcpListener {
    /// Creates a new TcpListener, which will be bound to the specified address.
    ///
    /// The returned listener is ready for accepting connections.
    ///
    /// Binding with a port number of 0 will request that the OS assigns a port
    /// to this listener.
    ///
    /// In the future, the port allocated can be queried via a (blocking) `local_addr`
    /// method.
    pub fn bind(addr: SocketAddr) -> io::Result<Self> {
        let socket = Socket::bind(addr, libc::SOCK_STREAM)?;
        socket.listen(1024)?;
        Ok(TcpListener { inner: socket })
    }

    /// Accepts a new incoming connection from this listener.
    ///
    /// This function will yield once a new TCP connection is established. When
    /// established, the corresponding [`TcpStream`] and the remote peer's
    /// address will be returned.
    ///
    /// [`TcpStream`]: struct@crate::net::TcpStream
    pub async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let (socket, socket_addr) = self.inner.accept().await?;
        let stream = TcpStream { inner: socket };
        let socket_addr = socket_addr.ok_or_else(|| {
            io::Error::new(io::ErrorKind::Other, "Could not get socket IP address")
        })?;
        Ok((stream, socket_addr))
    }

    /// TODO missing doc
    /// This is the slow version of the accept_multishot operation, where the implementation is
    /// a sequence of single accept operations under the hood.
    pub fn accept_multishot(
        &self,
    ) -> io::Result<(
        impl Stream<Item = io::Result<(TcpStream, SocketAddr)>> + '_,
        cancellable::Handle,
    )> {
        use async_stream::try_stream;
        use tokio::pin;
        use tokio_stream::StreamExt;
        let (s, cancel) = self.inner.accept_multishot()?;
        Ok((
            try_stream! {
                pin!(s);
                while let Some((socket, socket_addr)) = s.try_next().await? {
                    let stream = TcpStream { inner: socket };
                    let socket_addr = socket_addr.ok_or_else(|| {
                        io::Error::new(io::ErrorKind::Other, "Could not get socket IP address")
                    })?;
                    yield (stream, socket_addr)
                }
            },
            cancel,
        ))
    }

    /// TODO missing doc
    /// This is the slow version of the accept_multishot operation, where the implementation is
    /// a sequence of single accept operations under the hood.
    pub fn accept_multishot_slow(
        &self,
    ) -> impl Stream<Item = io::Result<(TcpStream, SocketAddr)>> + '_ {
        use async_stream::try_stream;
        use tokio::pin;
        use tokio_stream::StreamExt;
        try_stream! {
            let s = self.inner.accept_multishot_slow();
            pin!(s);
            while let Some((socket, socket_addr)) = s.try_next().await? {
                let stream = TcpStream { inner: socket };
                let socket_addr = socket_addr.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::Other, "Could not get socket IP address")
                })?;
                yield (stream, socket_addr)
            }
        }
    }
}
