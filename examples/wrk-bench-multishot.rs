use std::io;
use std::rc::Rc;
use tokio::task::JoinHandle;

use tokio::pin;
use tokio_stream::StreamExt;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

pub const RESPONSE: &[u8] =
    b"HTTP/1.1 200 OK\nContent-Type: text/plain\nContent-Length: 12\n\nHello world!";

pub const ADDRESS: &str = "127.0.0.1:8080";

fn main() -> io::Result<()> {
    tokio_uring::start(async {
        let tasks = Arc::new(Mutex::new(VecDeque::with_capacity(16)));
        let listener = Rc::new(tokio_uring::net::TcpListener::bind(
            ADDRESS.parse().unwrap(),
        )?);

        // TODO cleanup once working
        //for _ in 0..16 {}
        for _ in 0..1 {
            let listener = listener.clone();
            let tasks2 = tasks.clone();
            // TODO what's the difference between these two spawns? What does spawn_local do in a
            // current-thread runtime?
            // let task: JoinHandle<io::Result<()>> = tokio::task::spawn_local(async move {}
            let task: JoinHandle<io::Result<()>> = tokio_uring::spawn(async move {
                // accept_multishot isn't possible now. What to write about it?
                let (stream, _cancelid) = listener.accept_multishot().unwrap();
                pin!(stream);
                while let Some(next) = stream.next().await {
                    match next {
                        Err(e) => {
                            // Just print the error and continue.
                            println!("accept_multishot stream.next returned {}", e);
                        },
                        Ok((tcp_stream, _)) => {
                            let task2: JoinHandle<io::Result<()>> = tokio_uring::spawn(async move {
                                let (result, _) = tcp_stream.write(RESPONSE).await;

                                if let Err(err) = result {
                                    eprintln!("Client connection failed: {}", err);
                                }
                                Ok(())
                            });
                            tasks2.lock().unwrap().push_back(task2);
                        },
                    }
                }
                Ok(())
            });
            tasks.lock().unwrap().push_back(task);
        }

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
