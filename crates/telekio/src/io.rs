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
    kind: IoKind,
    raw: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct IoInterest(u8);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct IoReady(u8);

macro_rules! io_error_kinds {
    ($($kind:ident),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(u8)]
        enum IoErrorKind {
            $($kind,)+
            Other,
        }

        impl From<std::io::ErrorKind> for IoErrorKind {
            fn from(kind: std::io::ErrorKind) -> Self {
                match kind {
                    $(std::io::ErrorKind::$kind => Self::$kind,)+
                    _ => Self::Other,
                }
            }
        }

        impl From<IoErrorKind> for std::io::ErrorKind {
            fn from(kind: IoErrorKind) -> Self {
                match kind {
                    $(IoErrorKind::$kind => Self::$kind,)+
                    IoErrorKind::Other => Self::Other,
                }
            }
        }
    };
}

io_error_kinds!(
    NotFound,
    PermissionDenied,
    ConnectionRefused,
    ConnectionReset,
    HostUnreachable,
    NetworkUnreachable,
    ConnectionAborted,
    NotConnected,
    AddrInUse,
    AddrNotAvailable,
    NetworkDown,
    BrokenPipe,
    AlreadyExists,
    WouldBlock,
    NotADirectory,
    IsADirectory,
    DirectoryNotEmpty,
    ReadOnlyFilesystem,
    StaleNetworkFileHandle,
    InvalidInput,
    InvalidData,
    TimedOut,
    WriteZero,
    StorageFull,
    NotSeekable,
    QuotaExceeded,
    FileTooLarge,
    ResourceBusy,
    ExecutableFileBusy,
    Deadlock,
    CrossesDevices,
    TooManyLinks,
    InvalidFilename,
    ArgumentListTooLong,
    Interrupted,
    Unsupported,
    UnexpectedEof,
    OutOfMemory,
);

#[derive(Clone, Copy)]
#[repr(C)]
pub struct IoError {
    kind: IoErrorKind,
    raw: i32,
    has_raw: u8,
}

#[repr(C)]
pub struct IoRegistration {
    data: *mut c_void,
    poll: unsafe extern "C" fn(*mut c_void, IoInterest, *const Waker) -> IoPoll,
    ready: unsafe extern "C" fn(*mut c_void, IoInterest) -> IoOperationResult,
    try_operate: unsafe extern "C" fn(*mut c_void, IoRequest) -> IoPoll,
    try_ready: unsafe extern "C" fn(*mut c_void, IoInterest) -> IoPoll,
    clear: unsafe extern "C" fn(*mut c_void, u8, IoReady) -> IoCallResult,
    release: unsafe extern "C" fn(*mut c_void) -> CallResult,
}

#[repr(C)]
pub struct IoDriverRegistration {
    data: *mut c_void,
    release: unsafe extern "C" fn(*mut c_void) -> CallResult,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum IoOperationKind {
    Connect,
    Read,
    Write,
    Disconnect,
}

#[repr(C)]
pub struct IoRequest {
    kind: IoOperationKind,
    data: *mut u8,
    len: usize,
}

#[repr(C)]
pub struct IoOperation {
    data: *mut c_void,
    poll: unsafe extern "C" fn(*mut c_void, *const Waker) -> IoPoll,
    release: unsafe extern "C" fn(*mut c_void) -> CallResult,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct IoEvent {
    pub ready: IoReady,
    pub tick: u8,
}

#[repr(C)]
pub struct IoResult {
    pub call: CallResult,
    pub error: IoError,
    pub registration: IoRegistration,
}

#[repr(C)]
pub struct IoDriverResult {
    pub call: CallResult,
    pub error: IoError,
    pub registration: IoDriverRegistration,
}

#[repr(C)]
pub struct IoOperationResult {
    pub call: CallResult,
    pub operation: IoOperation,
}

#[repr(C)]
pub struct IoCallResult {
    pub call: CallResult,
    pub error: IoError,
}

#[repr(C)]
pub struct IoPoll {
    pub state: Poll,
    pub call: CallResult,
    pub error: IoError,
    pub ready: IoReady,
    pub tick: u8,
    pub value: usize,
}

unsafe impl Send for IoRegistration {}
unsafe impl Sync for IoRegistration {}
unsafe impl Send for IoDriverRegistration {}
unsafe impl Sync for IoDriverRegistration {}
unsafe impl Send for IoOperation {}
unsafe impl Sync for IoOperation {}

impl IoResource {
    /// # Safety
    ///
    /// `raw` must remain a valid file descriptor until the resulting
    /// registration is dropped.
    #[doc(hidden)]
    pub const unsafe fn fd(raw: i32) -> Self {
        Self {
            kind: IoKind::Fd,
            raw: raw as u32 as u64,
        }
    }

    /// # Safety
    ///
    /// `raw` must remain a valid socket until the resulting registration is dropped.
    #[doc(hidden)]
    pub const unsafe fn socket(raw: u64) -> Self {
        Self {
            kind: IoKind::Socket,
            raw,
        }
    }

    /// # Safety
    ///
    /// `raw` must remain a valid handle until the registration call returns.
    #[doc(hidden)]
    pub const unsafe fn handle(raw: u64) -> Self {
        Self {
            kind: IoKind::Handle,
            raw,
        }
    }

    #[doc(hidden)]
    pub const fn kind(self) -> IoKind {
        self.kind
    }

    #[doc(hidden)]
    pub const fn raw(self) -> u64 {
        self.raw
    }
}

impl IoRequest {
    /// # Safety
    ///
    /// `data..data + len` must remain valid for the requested synchronous I/O
    /// operation and permit access matching `kind`.
    #[doc(hidden)]
    pub const unsafe fn from_raw(kind: IoOperationKind, data: *mut u8, len: usize) -> Self {
        Self { kind, data, len }
    }

    #[doc(hidden)]
    pub const fn kind(&self) -> IoOperationKind {
        self.kind
    }

    #[doc(hidden)]
    pub const fn buffer(&self) -> *mut u8 {
        self.data
    }

    #[doc(hidden)]
    pub const fn buffer_len(&self) -> usize {
        self.len
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

impl IoError {
    pub const fn none() -> Self {
        Self {
            kind: IoErrorKind::Other,
            raw: 0,
            has_raw: 0,
        }
    }

    pub fn from_error(error: &std::io::Error) -> Self {
        Self {
            kind: error.kind().into(),
            raw: error.raw_os_error().unwrap_or_default(),
            has_raw: error.raw_os_error().is_some().into(),
        }
    }

    #[doc(hidden)]
    pub fn into_io_result(self, call: CallResult) -> std::io::Result<()> {
        if call.status != crate::Status::Error {
            return call.into_io_result();
        }
        if self.has_raw != 0 {
            unsafe { call.payload.release() };
            Err(std::io::Error::from_raw_os_error(self.raw))
        } else {
            Err(std::io::Error::new(self.kind.into(), unsafe {
                call.payload.into_string()
            }))
        }
    }
}

impl IoCallResult {
    fn into_io_result(self) -> std::io::Result<()> {
        self.error.into_io_result(self.call)
    }
}

impl IoRegistration {
    pub fn empty() -> Self {
        Self {
            data: std::ptr::null_mut(),
            poll: poll_empty,
            ready: ready_empty,
            try_operate: try_operate_empty,
            try_ready: try_ready_empty,
            clear: clear_empty,
            release: release_empty,
        }
    }

    /// # Safety
    ///
    /// `data` and the callbacks must describe one owned host registration whose
    /// state synchronizes concurrent access across threads.
    pub const unsafe fn from_raw(
        data: *mut c_void,
        poll: unsafe extern "C" fn(*mut c_void, IoInterest, *const Waker) -> IoPoll,
        ready: unsafe extern "C" fn(*mut c_void, IoInterest) -> IoOperationResult,
        try_operate: unsafe extern "C" fn(*mut c_void, IoRequest) -> IoPoll,
        try_ready: unsafe extern "C" fn(*mut c_void, IoInterest) -> IoPoll,
        clear: unsafe extern "C" fn(*mut c_void, u8, IoReady) -> IoCallResult,
        release: unsafe extern "C" fn(*mut c_void) -> CallResult,
    ) -> Self {
        Self {
            data,
            poll,
            ready,
            try_operate,
            try_ready,
            clear,
            release,
        }
    }

    #[track_caller]
    pub fn from_result(result: IoResult) -> std::io::Result<Self> {
        if let Err(error) = result.error.into_io_result(result.call) {
            if error.to_string() == IO_DRIVER_DISABLED_ERROR {
                panic!("{error}");
            }
            return Err(error);
        }
        Ok(result.registration)
    }

    pub fn poll(&self, interest: IoInterest, waker: &Waker) -> IoPoll {
        unsafe { (self.poll)(self.data, interest, waker) }
    }

    pub fn ready(&self, interest: IoInterest) -> IoOperation {
        let result = unsafe { (self.ready)(self.data, interest) };
        result.call.resume("failed to create Tokio I/O operation");
        result.operation
    }

    pub fn try_operate(&self, request: IoRequest) -> IoPoll {
        unsafe { (self.try_operate)(self.data, request) }
    }

    pub fn try_ready(&self, interest: IoInterest) -> IoPoll {
        unsafe { (self.try_ready)(self.data, interest) }
    }

    pub fn clear(&self, tick: u8, ready: IoReady) -> std::io::Result<()> {
        unsafe { (self.clear)(self.data, tick, ready) }.into_io_result()
    }

    pub fn close(&mut self) {
        if !self.data.is_null() {
            unsafe { (self.release)(self.data) }.resume("failed to release Tokio I/O registration");
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

impl IoDriverRegistration {
    pub fn empty() -> Self {
        Self {
            data: std::ptr::null_mut(),
            release: release_empty,
        }
    }

    /// # Safety
    ///
    /// `data` and `release` must describe one owned host driver registration.
    pub const unsafe fn from_raw(
        data: *mut c_void,
        release: unsafe extern "C" fn(*mut c_void) -> CallResult,
    ) -> Self {
        Self { data, release }
    }

    pub fn from_result(result: IoDriverResult) -> std::io::Result<Self> {
        result.error.into_io_result(result.call)?;
        Ok(result.registration)
    }

    pub fn close(&mut self) {
        if !self.data.is_null() {
            unsafe { (self.release)(self.data) }
                .resume("failed to release Tokio I/O driver registration");
            self.data = std::ptr::null_mut();
        }
    }
}

impl Drop for IoDriverRegistration {
    fn drop(&mut self) {
        self.close();
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
    /// `data` and both callbacks must describe one owned host operation whose
    /// state is safe to move and access through shared references across
    /// threads.
    pub const unsafe fn from_raw(
        data: *mut c_void,
        poll: unsafe extern "C" fn(*mut c_void, *const Waker) -> IoPoll,
        release: unsafe extern "C" fn(*mut c_void) -> CallResult,
    ) -> Self {
        Self {
            data,
            poll,
            release,
        }
    }
}

impl Future for IoOperation {
    type Output = std::io::Result<IoEvent>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> RustPoll<Self::Output> {
        let waker = unsafe { Waker::from_ref(context.waker()) };
        let result = unsafe { (self.poll)(self.data, &raw const waker) };
        match result.state {
            Poll::Pending => {
                unsafe { result.call.payload.release() };
                RustPoll::Pending
            }
            Poll::Ready => match result.error.into_io_result(result.call) {
                Ok(()) => RustPoll::Ready(Ok(IoEvent {
                    ready: result.ready,
                    tick: result.tick,
                })),
                Err(error) => RustPoll::Ready(Err(error)),
            },
            Poll::Panicked => {
                result.error.into_io_result(result.call).unwrap();
                unreachable!()
            }
        }
    }
}

impl Drop for IoOperation {
    fn drop(&mut self) {
        unsafe { (self.release)(self.data) }.resume("failed to release Tokio I/O operation");
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

unsafe extern "C" fn try_operate_empty(_: *mut c_void, _: IoRequest) -> IoPoll {
    empty_poll()
}

unsafe extern "C" fn ready_empty(_: *mut c_void, _: IoInterest) -> IoOperationResult {
    IoOperationResult {
        call: CallResult::ok(),
        operation: IoOperation {
            data: std::ptr::null_mut(),
            poll: poll_operation_empty,
            release: release_empty,
        },
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
        error: IoError::none(),
        ready: IoReady::SHUTDOWN,
        tick: 0,
        value: 0,
    }
}

unsafe extern "C" fn try_ready_empty(_: *mut c_void, _: IoInterest) -> IoPoll {
    empty_poll()
}

unsafe extern "C" fn clear_empty(_: *mut c_void, _: u8, _: IoReady) -> IoCallResult {
    IoCallResult {
        call: CallResult::ok(),
        error: IoError::none(),
    }
}
unsafe extern "C" fn release_empty(_: *mut c_void) -> CallResult {
    CallResult::ok()
}
