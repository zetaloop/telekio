use std::{
    error::Error,
    future::Future,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    pin::Pin,
    sync::{Arc, mpsc},
    task::{Context, Poll},
    thread::JoinHandle,
    time::Duration,
};

use libloading::Library;
use telekio_host::{Attach, Attachment, Runtime};

#[path = "../../api.rs"]
mod api;
use api::PluginFuture;

struct Request {
    future: PluginFuture,
    attachment: Arc<Attachment>,
}

// The plugin returns an owned, pinned Send future; this adapter serializes its polls.
unsafe impl Send for Request {}

impl Future for Request {
    type Output = (u64, u64);

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let waker = unsafe { telekio_abi::Waker::from_ref(context.waker()) };
        let result = self
            .attachment
            .enter(|| unsafe { (self.future.poll)(self.future.data, &raw const waker) });
        match result.state {
            telekio_abi::Poll::Pending => Poll::Pending,
            telekio_abi::Poll::Ready => Poll::Ready((result.value, result.task_id)),
            telekio_abi::Poll::Panicked => panic!("plugin request panicked"),
        }
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        self.attachment
            .enter(|| unsafe { (self.future.release)(self.future.data) })
            .into_io_result()
            .unwrap();
    }
}

fn serve(response: bool) -> (String, mpsc::Receiver<()>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (accepted, waiting) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut reader = BufReader::new(&mut stream);
        loop {
            let mut line = String::new();
            assert_ne!(reader.read_line(&mut line).unwrap(), 0);
            if line == "\r\n" {
                break;
            }
        }
        accepted.send(()).unwrap();
        if response {
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n35")
                .unwrap();
        } else {
            loop {
                match stream.read(&mut [0; 64]) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => break,
                    Err(error) => panic!("cancelled request kept its connection open: {error}"),
                }
            }
        }
    });
    (format!("http://{address}"), waiting, server)
}

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args_os().nth(1).expect("expected plugin path");
    let runtime = Runtime::new()?;
    for _ in 0..10 {
        let library = unsafe { Library::new(&path)? };
        let attach = unsafe { *library.get::<Attach>(b"telekio_attach")? };
        let request = unsafe {
            *library.get::<unsafe extern "C" fn(*const u8, usize) -> PluginFuture>(b"request")?
        };
        let released = unsafe { *library.get::<unsafe extern "C" fn() -> usize>(b"released")? };
        let owner = runtime.owner();
        let attachment = Arc::new(unsafe { owner.attach(attach)? });
        let releases = attachment.enter(|| unsafe { released() });

        for response in [true, false] {
            let (url, accepted, server) = serve(response);
            let future = attachment.enter(|| unsafe { request(url.as_ptr(), url.len()) });
            let task = runtime.tokio().spawn(Request {
                future,
                attachment: Arc::clone(&attachment),
            });
            let id = task.id().to_string().parse::<u64>()?;
            accepted.recv_timeout(Duration::from_secs(10))?;
            if response {
                assert_eq!(runtime.tokio().block_on(task)?, (42, id));
            } else {
                task.abort();
                assert!(runtime.tokio().block_on(task).unwrap_err().is_cancelled());
            }
            server.join().unwrap();
        }
        assert_eq!(attachment.enter(|| unsafe { released() }), releases + 2);
        let mut attachment = Arc::try_unwrap(attachment).unwrap_or_else(|_| {
            panic!("plugin requests still hold their attachment");
        });
        runtime.tokio().block_on(owner.detach(&mut attachment))?;
        library.close()?;
    }
    println!("10 plugin call, cancellation, detach, and unload cycles passed");
    Ok(())
}
