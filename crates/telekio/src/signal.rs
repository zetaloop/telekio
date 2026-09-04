use std::{
    ffi::c_void,
    task::{Context, Poll as RustPoll},
};

use crate::{CallResult, Handle, IoError, OperationPoll, Poll, Status, Waker};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum SignalKind {
    Unix,
    CtrlC,
    CtrlBreak,
    CtrlClose,
    CtrlLogoff,
    CtrlShutdown,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct SignalRequest {
    pub kind: SignalKind,
    pub number: i32,
}

#[repr(C)]
pub struct Signal {
    resource: crate::abi::Resource,
    poll: unsafe extern "C" fn(*mut c_void, *const Waker) -> OperationPoll,
}

#[repr(C)]
pub struct SignalResult {
    pub call: CallResult,
    pub error: IoError,
    pub signal: Signal,
}

impl SignalRequest {
    pub const fn unix(number: i32) -> Self {
        Self {
            kind: SignalKind::Unix,
            number,
        }
    }

    pub const fn console(kind: SignalKind) -> Self {
        Self { kind, number: 0 }
    }
}

impl Signal {
    pub fn empty() -> Self {
        Self {
            resource: crate::abi::Resource::empty(),
            poll: poll_empty,
        }
    }

    /// # Safety
    ///
    /// `data` and both callbacks must describe one owned host signal receiver
    /// whose state is safe to move and access through shared references across
    /// threads.
    pub const unsafe fn from_raw(
        data: *mut c_void,
        poll: unsafe extern "C" fn(*mut c_void, *const Waker) -> OperationPoll,
        release: unsafe extern "C" fn(*mut c_void) -> CallResult,
    ) -> Self {
        Self {
            resource: unsafe { crate::abi::Resource::from_raw(data, release) },
            poll,
        }
    }

    pub fn from_result(result: SignalResult) -> std::io::Result<Self> {
        result.error.into_io_result(result.call)?;
        Ok(result.signal)
    }

    pub async fn recv(&mut self) {
        std::future::poll_fn(|context| self.poll_recv(context)).await
    }

    pub fn poll_recv(&mut self, context: &mut Context<'_>) -> RustPoll<()> {
        let waker = unsafe { Waker::from_ref(context.waker()) };
        let result = unsafe { (self.poll)(self.resource.data(), &raw const waker) };
        match result.state {
            Poll::Pending => {
                unsafe { result.call.payload.release() };
                RustPoll::Pending
            }
            Poll::Ready => {
                result.call.into_io_result().expect("host signal failed");
                RustPoll::Ready(())
            }
            Poll::Panicked => {
                result.call.into_io_result().unwrap();
                unreachable!()
            }
        }
    }
}

impl std::fmt::Debug for Signal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Signal")
            .field("data", &self.resource.data())
            .finish_non_exhaustive()
    }
}

impl Handle {
    #[doc(hidden)]
    pub fn signal(&self, request: SignalRequest) -> std::io::Result<Signal> {
        Signal::from_result(unsafe { ((*self.raw.api).signal)(self.raw.context, request) })
    }
}

unsafe extern "C" fn poll_empty(_: *mut c_void, _: *const Waker) -> OperationPoll {
    OperationPoll {
        state: Poll::Ready,
        call: CallResult {
            status: Status::Ok,
            payload: crate::OwnedBytes::empty(),
        },
    }
}
