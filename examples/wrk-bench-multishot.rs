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
            // TODO cleanup let task: JoinHandle<io::Result<()>> = tokio::task::spawn_local(async move {}
            // TODO cleanup let task: JoinHandle<io::Result<()>> = tokio_uring::spawn(async move {}
            let task: JoinHandle<io::Result<()>> = tokio::task::spawn_local(async move {
                let s = listener.accept_multishot();
                pin!(s);
                while let Some((stream, _)) = s.try_next().await? {
                    let task2: JoinHandle<io::Result<()>> = tokio_uring::spawn(async move {
                        let (result, _) = stream.write(RESPONSE).await;

                        if let Err(err) = result {
                            eprintln!("Client connection failed: {}", err);
                        }
                        Ok(())
                    });
                    tasks2.lock().unwrap().push_back(task2);
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
