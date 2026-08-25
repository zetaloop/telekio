use std::{
    ffi::c_void,
    future::Future,
    pin::Pin,
    task::{Context, Poll as RustPoll},
};

use crate::{CallResult, Handle, Poll, Waker};

#[doc(hidden)]
pub const IO_DRIVER_DISABLED_ERROR: &str = "A Tokio 1.x context was found, but IO is disabled. Call `enable_io` on the runtime builder to enable IO.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum IoKind {
    Fd,
    Socket,
    Handle,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct IoResource {
    pub kind: IoKind,
    pub raw: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct IoInterest(u8);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct IoReady(u8);

#[repr(C)]
pub struct IoRegistration {
    data: *mut c_void,
    poll: unsafe extern "C" fn(*mut c_void, IoInterest, *const Waker) -> IoPoll,
    ready: unsafe extern "C" fn(*mut c_void, IoInterest) -> IoOperation,
    try_ready: unsafe extern "C" fn(*mut c_void, IoInterest) -> IoPoll,
    clear: unsafe extern "C" fn(*mut c_void, IoReady),
    release: unsafe extern "C" fn(*mut c_void),
}

#[repr(C)]
pub struct IoOperation {
    data: *mut c_void,
    poll: unsafe extern "C" fn(*mut c_void, *const Waker) -> IoPoll,
    release: unsafe extern "C" fn(*mut c_void),
}

#[repr(C)]
pub struct IoResult {
    pub call: CallResult,
    pub registration: IoRegistration,
}

#[repr(C)]
pub struct IoPoll {
    pub state: Poll,
    pub call: CallResult,
    pub ready: IoReady,
}

unsafe impl Send for IoRegistration {}
unsafe impl Sync for IoRegistration {}
unsafe impl Send for IoOperation {}

impl IoResource {
    pub const fn fd(raw: i32) -> Self {
        Self {
            kind: IoKind::Fd,
            raw: raw as u32 as u64,
        }
    }

    pub const fn socket(raw: u64) -> Self {
        Self {
            kind: IoKind::Socket,
            raw,
        }
    }

    pub const fn handle(raw: u64) -> Self {
        Self {
            kind: IoKind::Handle,
            raw,
        }
    }
}

impl IoInterest {
    pub const READABLE: Self = Self(1 << 0);
    pub const WRITABLE: Self = Self(1 << 1);
    pub const PRIORITY: Self = Self(1 << 2);
    pub const ERROR: Self = Self(1 << 3);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn contains(self, interest: Self) -> bool {
        self.0 & interest.0 != 0
    }
}

impl std::ops::BitOrAssign for IoInterest {
    fn bitor_assign(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

impl IoReady {
    pub const READABLE: Self = Self(1 << 0);
    pub const WRITABLE: Self = Self(1 << 1);
    pub const READ_CLOSED: Self = Self(1 << 2);
    pub const WRITE_CLOSED: Self = Self(1 << 3);
    pub const PRIORITY: Self = Self(1 << 4);
    pub const ERROR: Self = Self(1 << 5);
    pub const SHUTDOWN: Self = Self(1 << 6);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, ready: Self) -> bool {
        self.0 & ready.0 != 0
    }
}

impl std::ops::BitOrAssign for IoReady {
    fn bitor_assign(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

impl IoRegistration {
    pub fn empty() -> Self {
        Self {
            data: std::ptr::null_mut(),
            poll: poll_empty,
            ready: ready_empty,
            try_ready: try_ready_empty,
            clear: clear_empty,
            release: release_empty,
        }
    }

    /// # Safety
    ///
    /// `data` and the callbacks must describe one owned host registration.
    pub const unsafe fn from_raw(
        data: *mut c_void,
        poll: unsafe extern "C" fn(*mut c_void, IoInterest, *const Waker) -> IoPoll,
        ready: unsafe extern "C" fn(*mut c_void, IoInterest) -> IoOperation,
        try_ready: unsafe extern "C" fn(*mut c_void, IoInterest) -> IoPoll,
        clear: unsafe extern "C" fn(*mut c_void, IoReady),
        release: unsafe extern "C" fn(*mut c_void),
    ) -> Self {
        Self {
            data,
            poll,
            ready,
            try_ready,
            clear,
            release,
        }
    }

    #[track_caller]
    pub fn from_result(result: IoResult) -> std::io::Result<Self> {
        if result.call.status == crate::Status::Error {
            let message = unsafe { result.call.payload.into_string() };
            if message == IO_DRIVER_DISABLED_ERROR {
                panic!("{message}");
            }
            return Err(std::io::Error::other(message));
        }
        result.call.into_io_result()?;
        Ok(result.registration)
    }

    pub fn poll(&self, interest: IoInterest, waker: &Waker) -> IoPoll {
        unsafe { (self.poll)(self.data, interest, waker) }
    }

    pub fn ready(&self, interest: IoInterest) -> IoOperation {
        unsafe { (self.ready)(self.data, interest) }
    }

    pub fn try_ready(&self, interest: IoInterest) -> IoPoll {
        unsafe { (self.try_ready)(self.data, interest) }
    }

    pub fn clear(&self, ready: IoReady) {
        unsafe { (self.clear)(self.data, ready) };
    }

    pub fn close(&mut self) {
        if !self.data.is_null() {
            unsafe { (self.release)(self.data) };
            self.data = std::ptr::null_mut();
        }
    }
}

impl Drop for IoRegistration {
    fn drop(&mut self) {
        self.close();
    }
}

impl std::fmt::Debug for IoRegistration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IoRegistration")
            .field("data", &self.data)
            .finish_non_exhaustive()
    }
}

impl IoOperation {
    pub fn empty() -> Self {
        Self {
            data: std::ptr::null_mut(),
            poll: poll_operation_empty,
            release: release_empty,
        }
    }

    /// # Safety
    ///
    /// `data` and both callbacks must describe one owned host operation.
    pub const unsafe fn from_raw(
        data: *mut c_void,
        poll: unsafe extern "C" fn(*mut c_void, *const Waker) -> IoPoll,
        release: unsafe extern "C" fn(*mut c_void),
    ) -> Self {
        Self {
            data,
            poll,
            release,
        }
    }
}

impl Future for IoOperation {
    type Output = std::io::Result<IoReady>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> RustPoll<Self::Output> {
        let waker = Waker::from_ref(context.waker());
        let result = unsafe { (self.poll)(self.data, &raw const waker) };
        match result.state {
            Poll::Pending => {
                unsafe { result.call.payload.release() };
                RustPoll::Pending
            }
            Poll::Ready => match result.call.into_io_result() {
                Ok(()) => RustPoll::Ready(Ok(result.ready)),
                Err(error) => RustPoll::Ready(Err(error)),
            },
            Poll::Panicked => {
                result.call.into_io_result().unwrap();
                unreachable!()
            }
        }
    }
}

impl Drop for IoOperation {
    fn drop(&mut self) {
        unsafe { (self.release)(self.data) };
    }
}

impl Handle {
    #[doc(hidden)]
    #[track_caller]
    pub fn register_io(
        &self,
        resource: IoResource,
        interest: IoInterest,
    ) -> std::io::Result<IoRegistration> {
        IoRegistration::from_result(unsafe {
            ((*self.raw.api).register_io)(self.raw.context, resource, interest)
        })
    }
}

unsafe extern "C" fn poll_empty(_: *mut c_void, _: IoInterest, _: *const Waker) -> IoPoll {
    empty_poll()
}

unsafe extern "C" fn ready_empty(_: *mut c_void, _: IoInterest) -> IoOperation {
    IoOperation {
        data: std::ptr::null_mut(),
        poll: poll_operation_empty,
        release: release_empty,
    }
}

unsafe extern "C" fn poll_operation_empty(_: *mut c_void, _: *const Waker) -> IoPoll {
    empty_poll()
}

fn empty_poll() -> IoPoll {
    IoPoll {
        state: Poll::Ready,
        call: crate::CallResult {
            status: crate::Status::Ok,
            payload: crate::OwnedBytes::empty(),
        },
        ready: IoReady::SHUTDOWN,
    }
}

unsafe extern "C" fn try_ready_empty(_: *mut c_void, _: IoInterest) -> IoPoll {
    empty_poll()
}

unsafe extern "C" fn clear_empty(_: *mut c_void, _: IoReady) {}
unsafe extern "C" fn release_empty(_: *mut c_void) {}
