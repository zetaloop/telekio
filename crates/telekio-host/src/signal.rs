use std::{
    ffi::c_void,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
    task::{Context, Poll as RustPoll},
};

use telekio::{
    CallResult, IoError, OperationPoll, OwnedBytes, Poll, SignalKind, SignalRequest, SignalResult,
    Status, Waker,
};

use super::{HandleContext, HostResource};

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
        Ok(Ok(receiver)) => match HostResource::new(&context.owner, Signal { receiver }) {
            Ok(signal) => SignalResult {
                call: call_ok(),
                error: IoError::none(),
                signal: unsafe {
                    telekio::Signal::from_raw(
                        Arc::into_raw(signal).cast_mut().cast(),
                        poll,
                        release,
                    )
                },
            },
            Err(error) => SignalResult {
                call: CallResult {
                    status: Status::Error,
                    payload: OwnedBytes::from_string(error),
                },
                error: IoError::none(),
                signal: telekio::Signal::empty(),
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
    match catch_unwind(AssertUnwindSafe(|| {
        let signal = unsafe { &*data.cast::<HostResource<Signal>>() };
        signal.update_waker(unsafe { &*waker });
        let waker = unsafe { (*waker).clone_rust_waker() };
        let mut context = Context::from_waker(&waker);
        match signal.with_mut(|signal| signal.receiver.poll_recv(&mut context)) {
            Ok(state) => OperationPoll {
                state: match state {
                    RustPoll::Pending => Poll::Pending,
                    RustPoll::Ready(()) => Poll::Ready,
                },
                call: call_ok(),
            },
            Err(error) => OperationPoll {
                state: Poll::Ready,
                call: CallResult {
                    status: Status::Error,
                    payload: OwnedBytes::from_string(error),
                },
            },
        }
    })) {
        Ok(result) => result,
        Err(payload) => OperationPoll {
            state: Poll::Panicked,
            call: super::host_panic(&*payload),
        },
    }
}

unsafe extern "C" fn release(data: *mut c_void) -> CallResult {
    super::host_callback(|| unsafe { Arc::from_raw(data.cast::<HostResource<Signal>>()) }.release())
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
