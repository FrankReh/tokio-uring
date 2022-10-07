use std::io;
use std::rc::Rc;
use tokio::task::JoinHandle;

const RESPONSE: &'static [u8] =
    b"HTTP/1.1 200 OK\nContent-Type: text/plain\nContent-Length: 12\n\nHello world!";

const ADDRESS: &'static str = "0.0.0.0:8090";

fn main() -> io::Result<()> {
    tokio_uring::start(async {
        let mut tasks = Vec::with_capacity(16);
        let listener = Rc::new(tokio_uring::net::TcpListener::bind(
            ADDRESS.parse().unwrap(),
        )?);

        for _ in 0..16 {
            let listener = listener.clone();
            let task: JoinHandle<io::Result<()>> = tokio::task::spawn_local(async move {
                loop {
                    let (stream, _) = listener.accept().await?;

                    tokio_uring::spawn(async move {
                        let (result, _) = stream.write_all(RESPONSE).await;

                        if let Err(err) = result {
                            eprintln!("Client connection failed: {}", err);
                            return;
                        }

                        // Continuous read from the TCP stream until the other side has closed it.
                        let mut buf = vec![1u8; 256];
                        loop {
                            let (result, nbuf) = stream.read(buf).await;
                            match result {
                                Ok(0) => {
                                    //println!("stream.read returned 0");
                                    // Break when other side has closed.
                                    break;
                                }
                                Ok(_n) => {
                                    //println!("stream.read returned n: {}", _n);
                                }
                                Err(_err) => {
                                    //println!("stream.read returned error: {}", _err);
                                    // Break when other side has closed.
                                    break;
                                }
                            }
                            buf = nbuf;
                        }
                    });
                }
            });
            tasks.push(task);
        }

        for t in tasks {
            t.await.unwrap()?;
        }

        Ok(())
    })
}
