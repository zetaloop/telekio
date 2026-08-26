use std::{
    ffi::c_void,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    task::{Context, Poll as RustPoll},
};

use telekio::{
    CallResult, IoError, OperationPoll, OwnedBytes, Poll, SignalKind, SignalRequest, SignalResult,
    Status, Waker,
};

use super::HandleContext;

trait Receiver: Send {
    fn poll_recv(&mut self, context: &mut Context<'_>) -> RustPoll<()>;
}

struct Signal {
    receiver: Box<dyn Receiver>,
}

#[cfg(unix)]
impl Receiver for tokio::signal::unix::Signal {
    fn poll_recv(&mut self, context: &mut Context<'_>) -> RustPoll<()> {
        self.poll_recv(context).map(|_| ())
    }
}

#[cfg(windows)]
macro_rules! windows_receivers {
    ($($receiver:ty),+ $(,)?) => {
        $(
            impl Receiver for $receiver {
                fn poll_recv(&mut self, context: &mut Context<'_>) -> RustPoll<()> {
                    self.poll_recv(context).map(|_| ())
                }
            }
        )+
    };
}

#[cfg(windows)]
windows_receivers!(
    tokio::signal::windows::CtrlC,
    tokio::signal::windows::CtrlBreak,
    tokio::signal::windows::CtrlClose,
    tokio::signal::windows::CtrlLogoff,
    tokio::signal::windows::CtrlShutdown,
);

pub(super) unsafe extern "C" fn signal(
    context: *const c_void,
    request: SignalRequest,
) -> SignalResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| create(context, request))) {
        Ok(Ok(receiver)) => SignalResult {
            call: call_ok(),
            error: IoError::none(),
            signal: unsafe {
                telekio::Signal::from_raw(
                    Box::into_raw(Box::new(Signal { receiver })).cast(),
                    poll,
                    release,
                )
            },
        },
        Ok(Err(error)) => SignalResult {
            error: IoError::from_error(&error),
            call: call_error(error),
            signal: telekio::Signal::empty(),
        },
        Err(payload) => SignalResult {
            call: super::host_panic(&*payload),
            error: IoError::none(),
            signal: telekio::Signal::empty(),
        },
    }
}

#[cfg(unix)]
fn create(context: &HandleContext, request: SignalRequest) -> io::Result<Box<dyn Receiver>> {
    let _guard = context.handle.enter();
    if request.kind == SignalKind::Unix {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::from_raw(request.number))
            .map(|receiver| Box::new(receiver) as Box<dyn Receiver>)
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows console signals are unavailable on Unix",
        ))
    }
}

#[cfg(windows)]
fn create(context: &HandleContext, request: SignalRequest) -> io::Result<Box<dyn Receiver>> {
    let _guard = context.handle.enter();
    match request.kind {
        SignalKind::CtrlC => {
            tokio::signal::windows::ctrl_c().map(|receiver| Box::new(receiver) as Box<dyn Receiver>)
        }
        SignalKind::CtrlBreak => tokio::signal::windows::ctrl_break()
            .map(|receiver| Box::new(receiver) as Box<dyn Receiver>),
        SignalKind::CtrlClose => tokio::signal::windows::ctrl_close()
            .map(|receiver| Box::new(receiver) as Box<dyn Receiver>),
        SignalKind::CtrlLogoff => tokio::signal::windows::ctrl_logoff()
            .map(|receiver| Box::new(receiver) as Box<dyn Receiver>),
        SignalKind::CtrlShutdown => tokio::signal::windows::ctrl_shutdown()
            .map(|receiver| Box::new(receiver) as Box<dyn Receiver>),
        SignalKind::Unix => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Unix signals are unavailable on Windows",
        )),
    }
}

#[cfg(not(any(unix, windows)))]
fn create(_: &HandleContext, _: SignalRequest) -> io::Result<Box<dyn Receiver>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "signals are unavailable on this platform",
    ))
}

unsafe extern "C" fn poll(data: *mut c_void, waker: *const Waker) -> OperationPoll {
    let signal = unsafe { &mut *data.cast::<Signal>() };
    let waker = unsafe { (*waker).clone_rust_waker() };
    let mut context = Context::from_waker(&waker);
    OperationPoll {
        state: match signal.receiver.poll_recv(&mut context) {
            RustPoll::Pending => Poll::Pending,
            RustPoll::Ready(()) => Poll::Ready,
        },
        call: call_ok(),
    }
}

unsafe extern "C" fn release(data: *mut c_void) {
    drop(unsafe { Box::from_raw(data.cast::<Signal>()) });
}

fn call_ok() -> CallResult {
    CallResult {
        status: Status::Ok,
        payload: OwnedBytes::empty(),
    }
}

fn call_error(error: io::Error) -> CallResult {
    CallResult {
        status: Status::Error,
        payload: OwnedBytes::from_string(error.to_string()),
    }
}
