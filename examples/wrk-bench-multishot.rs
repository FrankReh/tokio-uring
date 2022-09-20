use std::io;
use std::rc::Rc;
use tokio::task::JoinHandle;

use tokio::pin;
use tokio_stream::StreamExt;

use std::collections::VecDeque;
use std::sync::Mutex;

use tokio_uring::net::TcpListener;

pub const RESPONSE: &[u8] =
    b"HTTP/1.1 200 OK\nContent-Type: text/plain\nContent-Length: 12\n\nHello world!";

pub const ADDRESS: &str = "127.0.0.1:8080";
pub const CLOSE_ADDRESS: &str = "127.0.0.1:8081";

fn main() -> io::Result<()> {
    tokio_uring::start(async {
        let tasks = Rc::new(Mutex::new(VecDeque::with_capacity(16)));
        let listener = Rc::new(TcpListener::bind(ADDRESS.parse().unwrap())?);
        let close_listener = TcpListener::bind(CLOSE_ADDRESS.parse().unwrap())?;

        let listener = listener.clone();

        let tasks2 = tasks.clone();
        // TODO what's the difference between these two spawns? What does spawn_local do in a
        // current-thread runtime?
        // let task: JoinHandle<io::Result<()>> = tokio::task::spawn_local(async move {}

        let task: JoinHandle<io::Result<()>> = tokio_uring::spawn(async move {
            // TODO fix need for mut cancel
            let (stream, mut cancel) = listener.accept_multishot().unwrap();
            pin!(stream);

            let closer: JoinHandle<io::Result<()>> = tokio_uring::spawn(async move {
                let (c_stream, mut c_cancel) = close_listener.accept_multishot().unwrap();
                pin!(c_stream);
                c_stream.next().await;
                cancel.async_cancel().await;
                c_cancel.async_cancel().await;
                Ok(())
            });

            while let Some(next) = stream.next().await {
                match next {
                    Err(e) => {
                        // Just print the error and continue.
                        // Interesting: when the async_cancel code was first added above,
                        // this returned an an os error 25, in io::Error terms: Operation canceled.
                        println!("accept_multishot stream.next returned {}", e);
                    }
                    Ok((tcp_stream, _)) => {
                        let task2: JoinHandle<io::Result<()>> = tokio_uring::spawn(async move {
                            let (result, _) = tcp_stream.write(RESPONSE).await;

                            if let Err(err) = result {
                                eprintln!("Client connection failed: {}", err);
                            }
                            Ok(())
                        });
                        tasks2.lock().unwrap().push_back(task2);
                    }
                }
            }

            closer.await.unwrap()?;

            Ok(())
        });
        tasks.lock().unwrap().push_back(task);

        loop {
            let front = tasks.lock().unwrap().pop_front();
            match front {
                Some(t) => {
                    t.await.unwrap()?;
                }
                None => {
                    break;
                }
            }
        }

        Ok(())
    })
}
