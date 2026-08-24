use std::{
    any::Any,
    ffi::c_void,
    fmt,
    future::Future as RustFuture,
    mem::MaybeUninit,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    pin::Pin,
    slice,
    task::{Context, Poll as RustPoll, RawWaker, RawWakerVTable, Waker as RustWaker},
};

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Runtime {
    context: *const c_void,
    api: *const RuntimeApi,
}

unsafe impl Send for Runtime {}
unsafe impl Sync for Runtime {}

#[repr(C)]
pub struct RuntimeApi {
    pub block_on: unsafe extern "C" fn(*const c_void, Future) -> BlockOnResult,
}

#[repr(C)]
pub struct BlockOnResult {
    pub status: Status,
    pub payload: OwnedBytes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Status {
    Ok,
    Panicked,
    HostPanicked,
}

#[repr(C)]
pub struct OwnedBytes {
    pub data: *mut u8,
    pub len: usize,
    pub release: unsafe extern "C" fn(*mut u8, usize),
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Future {
    pub data: *mut c_void,
    pub poll: unsafe extern "C" fn(*mut c_void, *const Waker) -> Poll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Poll {
    Pending,
    Ready,
    Panicked,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Waker {
    data: *const c_void,
    clone: unsafe extern "C" fn(*const c_void, *mut Waker),
    wake: unsafe extern "C" fn(*const c_void),
    wake_by_ref: unsafe extern "C" fn(*const c_void),
    release: unsafe extern "C" fn(*const c_void),
}

unsafe impl Send for Waker {}
unsafe impl Sync for Waker {}

struct FutureState<F: RustFuture> {
    future: Option<Pin<Box<F>>>,
    result: Option<Result<F::Output, Box<dyn Any + Send>>>,
}

impl Runtime {
    /// # Safety
    ///
    /// `context` and `api` must remain valid for every use of the returned runtime.
    pub const unsafe fn from_raw(context: *const c_void, api: *const RuntimeApi) -> Self {
        Self { context, api }
    }

    pub fn block_on<F: RustFuture>(&self, future: F) -> F::Output {
        let mut state = FutureState {
            future: Some(Box::pin(future)),
            result: None,
        };
        let future = Future {
            data: (&raw mut state).cast(),
            poll: poll_future::<F>,
        };
        let result = unsafe { ((*self.api).block_on)(self.context, future) };
        match (result.status, state.result) {
            (Status::Ok, Some(Ok(output))) => {
                unsafe { result.payload.release() };
                output
            }
            (Status::Panicked, Some(Err(payload))) => {
                unsafe { result.payload.release() };
                resume_unwind(payload)
            }
            (Status::HostPanicked, _) => {
                let message = unsafe { result.payload.into_string() };
                resume_unwind(Box::new(message))
            }
            _ => {
                unsafe { result.payload.release() };
                panic!("host Tokio runtime returned an invalid block_on result")
            }
        }
    }
}

impl OwnedBytes {
    pub fn empty() -> Self {
        Self {
            data: std::ptr::null_mut(),
            len: 0,
            release: release_bytes,
        }
    }

    pub fn from_string(value: String) -> Self {
        let bytes = value.into_bytes().into_boxed_slice();
        let len = bytes.len();
        if len == 0 {
            return Self::empty();
        }
        Self {
            data: Box::into_raw(bytes).cast(),
            len,
            release: release_bytes,
        }
    }

    unsafe fn release(self) {
        unsafe { (self.release)(self.data, self.len) };
    }

    unsafe fn into_string(self) -> String {
        let bytes = if self.len == 0 {
            Vec::new()
        } else {
            unsafe { slice::from_raw_parts(self.data, self.len) }.to_vec()
        };
        unsafe { self.release() };
        String::from_utf8(bytes).unwrap_or_else(|_| "host Tokio runtime panicked".to_owned())
    }
}

unsafe extern "C" fn release_bytes(data: *mut u8, len: usize) {
    if !data.is_null() {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(data, len)) });
    }
}

impl fmt::Debug for Runtime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Runtime")
            .field("context", &self.context)
            .finish_non_exhaustive()
    }
}

unsafe extern "C" fn poll_future<F: RustFuture>(data: *mut c_void, waker: *const Waker) -> Poll {
    let state = unsafe { &mut *data.cast::<FutureState<F>>() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let waker = unsafe { (*waker).clone_rust_waker() };
        let mut context = Context::from_waker(&waker);
        let poll = state
            .future
            .as_mut()
            .expect("completed future was polled")
            .as_mut()
            .poll(&mut context);
        if poll.is_ready() {
            state.future = None;
        }
        poll
    }));
    match result {
        Ok(RustPoll::Pending) => Poll::Pending,
        Ok(RustPoll::Ready(output)) => {
            state.result = Some(Ok(output));
            Poll::Ready
        }
        Err(payload) => {
            state.result = Some(Err(payload));
            Poll::Panicked
        }
    }
}

impl Waker {
    pub fn from_ref(waker: &RustWaker) -> Self {
        Self {
            data: (waker as *const RustWaker).cast(),
            clone: clone_borrowed,
            wake: wake_borrowed,
            wake_by_ref: wake_borrowed,
            release: release_borrowed,
        }
    }

    unsafe fn clone_rust_waker(&self) -> RustWaker {
        let mut owned = MaybeUninit::uninit();
        unsafe { (self.clone)(self.data, owned.as_mut_ptr()) };
        let owned = unsafe { owned.assume_init() };
        let raw = RawWaker::new(Box::into_raw(Box::new(owned)).cast(), &RAW_WAKER_VTABLE);
        unsafe { RustWaker::from_raw(raw) }
    }
}

unsafe extern "C" fn clone_borrowed(data: *const c_void, output: *mut Waker) {
    let waker = unsafe { &*data.cast::<RustWaker>() };
    unsafe { output.write(owned_waker(waker.clone())) };
}

unsafe extern "C" fn wake_borrowed(data: *const c_void) {
    unsafe { &*data.cast::<RustWaker>() }.wake_by_ref();
}

unsafe extern "C" fn release_borrowed(_: *const c_void) {}

fn owned_waker(waker: RustWaker) -> Waker {
    Waker {
        data: Box::into_raw(Box::new(waker)).cast(),
        clone: clone_owned,
        wake: wake_owned,
        wake_by_ref: wake_by_ref_owned,
        release: release_owned,
    }
}

unsafe extern "C" fn clone_owned(data: *const c_void, output: *mut Waker) {
    let waker = unsafe { &*data.cast::<RustWaker>() };
    unsafe { output.write(owned_waker(waker.clone())) };
}

unsafe extern "C" fn wake_owned(data: *const c_void) {
    unsafe { Box::from_raw(data.cast_mut().cast::<RustWaker>()) }.wake();
}

unsafe extern "C" fn wake_by_ref_owned(data: *const c_void) {
    unsafe { &*data.cast::<RustWaker>() }.wake_by_ref();
}

unsafe extern "C" fn release_owned(data: *const c_void) {
    drop(unsafe { Box::from_raw(data.cast_mut().cast::<RustWaker>()) });
}

static RAW_WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(raw_clone, raw_wake, raw_wake_by_ref, raw_drop);

unsafe fn raw_clone(data: *const ()) -> RawWaker {
    let waker = unsafe { &*data.cast::<Waker>() };
    let mut cloned = MaybeUninit::uninit();
    unsafe { (waker.clone)(waker.data, cloned.as_mut_ptr()) };
    let cloned = unsafe { cloned.assume_init() };
    RawWaker::new(Box::into_raw(Box::new(cloned)).cast(), &RAW_WAKER_VTABLE)
}

unsafe fn raw_wake(data: *const ()) {
    let waker = unsafe { Box::from_raw(data.cast_mut().cast::<Waker>()) };
    unsafe { (waker.wake)(waker.data) };
}

unsafe fn raw_wake_by_ref(data: *const ()) {
    let waker = unsafe { &*data.cast::<Waker>() };
    unsafe { (waker.wake_by_ref)(waker.data) };
}

unsafe fn raw_drop(data: *const ()) {
    let waker = unsafe { Box::from_raw(data.cast_mut().cast::<Waker>()) };
    unsafe { (waker.release)(waker.data) };
}
