use std::{
    ffi::c_void,
    future::Future,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll},
    time::Duration,
};

#[path = "../../api.rs"]
mod api;
use api::{PluginFuture, PluginPoll};

telekio::plugin!();

static RELEASED: AtomicUsize = AtomicUsize::new(0);

struct Request(Pin<Box<dyn Future<Output = (u64, u64)> + Send>>);

impl Drop for Request {
    fn drop(&mut self) {
        RELEASED.fetch_add(1, Ordering::SeqCst);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn released() -> usize {
    RELEASED.load(Ordering::SeqCst)
}

/// # Safety
///
/// `url` must reference `length` readable UTF-8 bytes for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn request(url: *const u8, length: usize) -> PluginFuture {
    let url = unsafe { std::str::from_utf8(std::slice::from_raw_parts(url, length)) }
        .unwrap()
        .to_owned();
    let future = async move {
        let task_id = tokio::task::id().to_string().parse().unwrap();
        let worker = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(1)).await;
            tokio::task::spawn_blocking(|| 7).await.unwrap()
        });
        let value = reqwest::get(url)
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
            .parse::<u64>()
            .unwrap();
        (value + worker.await.unwrap(), task_id)
    };
    PluginFuture {
        data: Box::into_raw(Box::new(Request(Box::pin(future)))).cast(),
        poll,
        release,
    }
}

unsafe extern "C" fn poll(data: *mut c_void, waker: *const telekio::Waker) -> PluginPoll {
    match catch_unwind(AssertUnwindSafe(|| {
        let request = unsafe { &mut *data.cast::<Request>() };
        let waker = unsafe { (*waker).clone_rust_waker() };
        request.0.as_mut().poll(&mut Context::from_waker(&waker))
    })) {
        Ok(Poll::Pending) => PluginPoll {
            state: telekio::Poll::Pending,
            value: 0,
            task_id: 0,
        },
        Ok(Poll::Ready((value, task_id))) => PluginPoll {
            state: telekio::Poll::Ready,
            value,
            task_id,
        },
        Err(_) => PluginPoll {
            state: telekio::Poll::Panicked,
            value: 0,
            task_id: 0,
        },
    }
}

unsafe extern "C" fn release(data: *mut c_void) -> telekio::CallResult {
    match catch_unwind(AssertUnwindSafe(|| {
        drop(unsafe { Box::from_raw(data.cast::<Request>()) });
    })) {
        Ok(()) => telekio::CallResult::ok(),
        Err(payload) => telekio::CallResult::panicked(&*payload),
    }
}
