// Test the drop aspect of the StrOp. In particular, for the first such operation, the multishot
// accept operation for which a stream is created, test that it is canceled automatically when the
// stream is dropped.
//
// This test necessarily requires a 5.19 or greater kernel.
//
// Spawn a task that does the accept in a loop that will break after two connections
// are established. Breaking out of the loop will also get the StrOp to be dropped.
//
// From the main task, make two connections to the accept loop and assert that they succeed,
// then wait a couple of ms, then try a third and confirm that it does not succeed.
// That should show that the StrOp drop method got the operation canceled.

use std::{cell::Cell, io, net::SocketAddr, rc::Rc};

use tokio::pin;
use tokio::sync::oneshot;
use tokio::time::Duration;
use tokio_stream::StreamExt;

use tokio_uring::net::{TcpListener, TcpStream};
use tokio_uring::spawn;

const ADDRESS: &str = "127.0.0.1:0";

fn main() -> io::Result<()> {
    tokio_uring::builder()
        .entries(16)
        .uring_builder(tokio_uring::uring_builder().setup_cqsize(32))
        .start(test_dropped_accept_stream())
}

type NotSupportedFlag = Rc<Cell<bool>>;

async fn test_dropped_accept_stream() -> io::Result<()> {
    // Channel for getting the listener's address and to ensure the timing of the accept
    // operations having been sent to the kernel.

    let count: i32 = 2; // How many connections to accept before letting accept stream be dropped.
    let not_supported: NotSupportedFlag = Rc::new(Cell::new(false));

    let (tx_ch, rx_ch) = oneshot::channel();

    let accept_task = spawn(accept(tx_ch, count, not_supported.clone()));

    let addr = rx_ch
        .await
        .expect("The spawned task expected to send a SocketAddr");

    // Connect `count` times.

    for _i in 0..count {
        let s = TcpStream::connect(addr).await;

        // If this test is being run on a kernel before 5.19, break out of loop without generating
        // another error.
        if not_supported.get() {
            break;
        }
        let s = s.unwrap();

        _ = s.shutdown(std::net::Shutdown::Both);
        // println!("connect {} to {} succeeded", _i, addr);
    }

    // Wait for the accept task to be completed which involves the cancel of the accept
    // operation being asynchronously sent to the ring driver in the kernel.
    //
    // If this resulted in an error, let the function return immediately.
    accept_task.await.unwrap()?;

    // A bit of a hack for this test. Wait a moment to let the drop method start the cancel
    // operation, and for the kernel to take action.
    tokio::time::sleep(Duration::from_millis(2)).await;

    // Verify that another connection attempt would fail.
    {
        // println!("trying a third connection to {}, which should fail.", addr);

        match TcpStream::connect(addr).await {
            Ok(_stream) => {
                panic!("third connection attempt should have failed, but returned a TcpStream")
            }
            Err(_) => {
                // println!("The cancel operation, triggered by the stream being dropped, succeeded in stopping the accept operation.");
            }
        }
    }

    // Give ring operations a chance to be completed.
    tokio::time::sleep(Duration::from_millis(2)).await;

    println!("Pass.");
    Ok(())
}

async fn accept(
    tx_ch: oneshot::Sender<SocketAddr>,
    count: i32,
    not_supported: NotSupportedFlag,
) -> io::Result<()> {
    let listener = TcpListener::bind(ADDRESS.parse().unwrap())?;

    let (accept_stream, _) = listener.accept_multishot().unwrap();

    if let Err(_) = tx_ch.send(listener.local_addr().unwrap()) {
        panic!("The receiver dropped");
    }

    pin!(accept_stream);

    for _i in 0..count {
        let next = accept_stream.next().await.unwrap();
        match next {
            Err(e) => {
                // Multishot accept requires kernel 5.19 or greater.
                if let Some(raw_os_err) = e.raw_os_error() {
                    if raw_os_err == 22 {
                        not_supported.set(true);
                        eprintln!("Invalid argument normally indicates the io_uring operation is not supported by this kernel, the multishot accept requires 5.19+");
                    }
                }
                eprintln!("accept_multishot accept_stream.next returned {}", e);
                return Err(e);
            }
            Ok(tcp_stream) => {
                // For this test, as soon as we accept the TCP connection, we shut it down.
                _ = tcp_stream.shutdown(std::net::Shutdown::Both);
            }
        }
    }

    Ok(())
    // Drop the accept_stream, triggering the cancellation operation.
}
