use std::{
    any::Any,
    ffi::c_void,
    future::Future as RustFuture,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    task::{Context as TaskContext, Poll as RustPoll},
};

use telekio::{BlockOnResult, Future, OwnedBytes, Poll, RuntimeApi, Status, Waker};

pub struct Runtime {
    context: Box<RuntimeContext>,
}

struct RuntimeContext {
    runtime: tokio::runtime::Runtime,
}

static RUNTIME_API: RuntimeApi = RuntimeApi { block_on };

impl Runtime {
    pub fn new() -> io::Result<Self> {
        tokio::runtime::Runtime::new().map(Self::from_tokio)
    }

    pub fn from_tokio(runtime: tokio::runtime::Runtime) -> Self {
        Self {
            context: Box::new(RuntimeContext { runtime }),
        }
    }

    pub fn runtime(&self) -> telekio::Runtime {
        unsafe {
            telekio::Runtime::from_raw(
                (&raw const *self.context).cast::<c_void>(),
                &raw const RUNTIME_API,
            )
        }
    }
}

unsafe extern "C" fn block_on(context: *const c_void, future: Future) -> BlockOnResult {
    let context = unsafe { &*context.cast::<RuntimeContext>() };
    match catch_unwind(AssertUnwindSafe(|| {
        context.runtime.block_on(GuestFuture(future))
    })) {
        Ok(status) => BlockOnResult {
            status,
            payload: OwnedBytes::empty(),
        },
        Err(payload) => BlockOnResult {
            status: Status::HostPanicked,
            payload: OwnedBytes::from_string(panic_message(&*payload)),
        },
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&'static str>()
                .map(|message| (*message).to_owned())
        })
        .unwrap_or_else(|| "Box<dyn Any>".to_owned())
}

struct GuestFuture(Future);

impl RustFuture for GuestFuture {
    type Output = Status;

    fn poll(self: Pin<&mut Self>, context: &mut TaskContext<'_>) -> RustPoll<Self::Output> {
        let waker = Waker::from_ref(context.waker());
        match unsafe { (self.0.poll)(self.0.data, &raw const waker) } {
            Poll::Pending => RustPoll::Pending,
            Poll::Ready => RustPoll::Ready(Status::Ok),
            Poll::Panicked => RustPoll::Ready(Status::Panicked),
        }
    }
}
