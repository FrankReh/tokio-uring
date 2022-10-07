use std::io;
use std::rc::Rc;
use tokio::time::Duration;

use tokio::pin;
use tokio_stream::StreamExt;

use tokio::task::JoinHandle;

use tokio_uring::net::{TcpListener, TcpStream};

const RESPONSE: &[u8] =
    b"HTTP/1.1 200 OK\nContent-Type: text/plain\nContent-Length: 12\n\nHello world!";

const ADDRESS: &str = "0.0.0.0:8080";
const CLOSE_ADDRESS: &str = "127.0.0.1:8081";

fn main() -> io::Result<()> {
    //console_subscriber::init();

    /*
    console_subscriber::ConsoleLayer::builder()
        // set how long the console will retain data from completed tasks
        .retention(Duration::from_secs(10))
        // ... other configurations ...
        .init();
    */

    // From the liburing manpage io_uring_queue_init_params(3)
    //
    // By default, the CQ ring will have twice the number of entries as
    // specified by entries for the SQ ring. This is adequate for regular
    // file or storage workloads, but may be too small networked workloads.
    // The SQ ring entries do not impose a limit on the number of in-flight
    // requests that the ring can support, it merely limits the number that
    // can be submitted to the kernel in one go (batch). if the CQ ring
    // overflows, e.g. more entries are generated than fits in the ring
    // before the application can reap them, then the ring enters a CQ ring
    // overflow state. This is indicated by IORING_SQ_CQ_OVERFLOW being set
    // in the SQ ring flags. Unless the kernel runs out of available memory,
    // entries are not dropped, but it is a much slower completion path and
    // will slow down request processing. For that reason it should be
    // avoided and the CQ ring sized appropriately for the workload.
    //
    // So we make the CQ large.
    // tokio_uring::start(async {
    tokio_uring::builder()
        // even without overflow the stall after 2000 from upcloud2, then 2000 from upcloud3,
        // then 2000 from upcloud2 again, causes a stall.
        .entries(512)
        .uring_builder(tokio_uring::uring_builder().setup_cqsize(4096))
        .start(async {
            let listener = Rc::new(TcpListener::bind(ADDRESS.parse().unwrap())?);
            let close_listener = TcpListener::bind(CLOSE_ADDRESS.parse().unwrap())?;

            let listener = listener.clone();

            let accept_task: JoinHandle<io::Result<()>> = tokio::task::spawn_local(async move {
                let ctx = new_ctx();

                // task that dumps to stdout or returns to client.
                let _dumper = tokio_uring::spawn(dump_ctx(ctx.clone()));

                // TODO maybe fix need for mut cancel
                let (accept_stream, mut cancel) = listener.accept_multishot().unwrap();
                pin!(accept_stream);

                let closer: JoinHandle<io::Result<()>> = tokio_uring::spawn(async move {
                    // The functionality of this benchmark/test server would be perfectly handled
                    // with a single-shot accept here, but for now, we use the multishot version
                    // because it is the function under test.
                    let (c_stream, mut c_cancel) = close_listener.accept_multishot().unwrap();
                    pin!(c_stream);
                    c_stream.next().await;
                    cancel.async_cancel().await;
                    c_cancel.async_cancel().await;

                    // dropping c_stream would get it canceled too so the above
                    // c_cancel.async_cancel().await isn't strictly necessary.
                    Ok(())
                });

                // If this is run on a kernel prior to 5.19, this will loop will fire one time with
                // the error of an 'Invalid argument', os error 22 having originated in the
                // kernel's io_uring interface and then the stream will be closed. Other kernel
                // errors for a particular accept won't necessarily close the accept stream.
                while let Some(next) = accept_stream.next().await {
                    match next {
                        Err(e) => {
                            // Just print the error and continue.
                            // Development note: when the async_cancel code was first added above,
                            // this returned an os error 25, in io::Error terms: Operation canceled.
                            // Now that canceled condition is absorbed by the low level stream
                            // itself and the stream is closed, without an error being returned
                            // that it was canceled.
                            println!("accept_multishot accept_stream.next returned {}", e);
                        }
                        Ok(tcp_stream) => {
                            // println!("tcp peer_addr {}", tcp_stream.peer_addr().unwrap());
                            let _task2: JoinHandle<io::Result<()>> =
                                tokio_uring::spawn(foo(ctx.clone(), tcp_stream));
                        }
                    }
                }

                closer.await.unwrap()?;

                Ok(())
            });

            // Now wait for the accept task to be completed which involves
            // the accept operation being canceled first.
            accept_task.await.unwrap()?;

            tokio::time::sleep(Duration::from_millis(100)).await;

            Ok(())
        })
}

// foo is the task that reads, writes, reads some more, and then closes the stream.
// There will be a separate foo task for each TCP connection accepted.
async fn foo(ctx: Ctx, tcp_stream: TcpStream) -> Result<(), std::io::Error> {
    // Continuous read from the TCP stream until 0 bytes are read.
    let mut buf = vec![1u8; 256];

    ctx.borrow_mut().read1 += 1;
    // First read

    let (result, nbuf) = tcp_stream.read(buf).await;
    buf = nbuf;
    _ = result?;

    ctx.borrow_mut().write1 += 1;
    let (result, _) = tcp_stream.write_all(RESPONSE).await;

    if let Err(e) = result {
        eprintln!("Client connection failed: {}", e);
        return Err(e);
    }

    ctx.borrow_mut().read2 += 1;

    // Second read, to avoid closing the connection before the client has.
    let (result, _nbuf) = tcp_stream.read(buf).await;
    _ = result?;

    ctx.borrow_mut().shutdown += 1;

    // Explicit shutdown from the server side avoids the lost TCP connections on
    // the server side that would end up in CLOSE_WAIT state when the client goes away
    // in the manor of the rbench-client non-conn tests.
    // rbench-client -h $addr -d 5s -r 1 -c 4000 --batch 16
    // let _ = tcp_stream.shutdown(std::net::Shutdown::Both); // this works
    // let _ = tcp_stream.shutdown(std::net::Shutdown::Read); // this works too
    // let _ = tcp_stream.shutdown(std::net::Shutdown::Write); // this does not work, the host is stuck with many of CLOSE_WAIT TCP connections anyway

    // Shutdown the reading side.
    // This was shown to work and should let the client receive any data that was sent but not yet
    // consumed by the client.
    // But leaves client with TIME_WAIT connections, closing Write side too doesn't help -
    // the client still has TIME_WAIT connections that have to timeout on their own. Seems the
    // client needs to also shutdown its connection cleanly - something the rbench-client isn't
    // doing yet when in original http mode where it uses hyper, but does do cleanly when in the
    // new --tcp mode.
    _ = tcp_stream.shutdown(std::net::Shutdown::Read);

    ctx.borrow_mut().done += 1;

    Ok(())
}

// This context is used for printing some counters every few seconds. Was useful when questioning
// how far a task had progressed before being blocked.

use core::cell::RefCell;

type Ctx = Rc<RefCell<Context>>;
fn new_ctx() -> Ctx {
    let context: Context = Default::default();
    Rc::new(RefCell::new(context))
}

#[derive(Debug, Default)]
struct Context {
    read1: usize,
    write1: usize,
    read2: usize,
    shutdown: usize,
    done: usize,
}
async fn dump_ctx(_ctx: Ctx) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;
        /* kept around for future debugging
        let c = ctx.borrow();
        println!("ctx: {:?}", c);
        */
    }
}
