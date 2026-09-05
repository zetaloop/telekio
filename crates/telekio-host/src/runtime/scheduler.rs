use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex},
};

use telekio::{CallResult, OwnedBytes, Status, Waker};

use crate::owner::CallbackCleanup;
use crate::{host_panic, result};

use super::HandleContext;

struct Deferred {
    state: Mutex<DeferredState>,
}

struct DeferredState {
    waker: Option<std::task::Waker>,
    cleanup: Option<CallbackCleanup>,
    woken: bool,
}

impl Deferred {
    fn wake(&self) {
        let (waker, cleanup) = {
            let mut state = self.state.lock().unwrap();
            state.woken = true;
            (state.waker.take(), state.cleanup.take())
        };
        if let Some(waker) = waker {
            waker.wake_by_ref();
        }
        drop(cleanup);
    }

    fn install(&self, cleanup: CallbackCleanup) {
        let mut state = self.state.lock().unwrap();
        if state.woken {
            drop(state);
            drop(cleanup);
        } else {
            state.cleanup = Some(cleanup);
        }
    }
}

impl std::task::Wake for Deferred {
    fn wake(self: Arc<Self>) {
        Deferred::wake(&self);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        Deferred::wake(self);
    }
}

pub(super) unsafe extern "C" fn defer(context: *const c_void, waker: *const Waker) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| {
        let guest = unsafe { (*waker).clone_rust_waker() };
        let deferred = Arc::new(Deferred {
            state: Mutex::new(DeferredState {
                waker: Some(guest.clone()),
                cleanup: None,
                woken: false,
            }),
        });
        let waker = std::task::Waker::from(Arc::clone(&deferred));
        let Some(cleanup) = context.owner.callback(waker.clone()) else {
            guest.wake();
            return;
        };
        deferred.install(cleanup);
        tokio::runtime::telekio::defer(&waker);
    })) {
        Ok(()) => result(Status::Ok, OwnedBytes::empty()),
        Err(payload) => host_panic(&*payload),
    }
}
