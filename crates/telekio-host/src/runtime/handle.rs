#[cfg(tokio_unstable)]
use std::sync::Mutex;
use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

use telekio::{CallResult, Flavor, Future, NameResult, OwnedBytes, RawHandle, Status};

use crate::owner::OwnerState;
use crate::{host_panic, result};

#[cfg(tokio_unstable)]
use super::metrics::WorkerObserver;
use super::{
    LocalSlot, RUNTIME_API,
    context::{GuestFuture, block_on_result},
};

pub(crate) struct HandleContext {
    pub(super) handle: tokio::runtime::Handle,
    pub(crate) owner: Arc<OwnerState>,
    pub(super) flavor: Flavor,
    pub(super) local: Option<Arc<LocalSlot>>,
    pub(super) io_enabled: bool,
    #[cfg(tokio_unstable)]
    pub(super) worker_observer: Mutex<Option<WorkerObserver>>,
}

pub(super) unsafe extern "C" fn flavor(context: *const c_void) -> Flavor {
    unsafe { &*context.cast::<HandleContext>() }.flavor
}

pub(super) unsafe extern "C" fn id(context: *const c_void) -> u64 {
    unsafe { &*context.cast::<HandleContext>() }
        .handle
        .id()
        .telekio_value()
}

pub(super) unsafe extern "C" fn name(context: *const c_void) -> NameResult {
    match catch_unwind(AssertUnwindSafe(|| {
        unsafe { &*context.cast::<HandleContext>() }
            .handle
            .name()
            .map(str::to_owned)
    })) {
        Ok(Some(value)) => NameResult {
            call: CallResult::ok(),
            value: OwnedBytes::from_string(value),
            is_some: true,
        },
        Ok(None) => NameResult {
            call: CallResult::ok(),
            value: OwnedBytes::empty(),
            is_some: false,
        },
        Err(payload) => NameResult {
            call: host_panic(&*payload),
            value: OwnedBytes::empty(),
            is_some: false,
        },
    }
}

pub(super) fn handle_context(
    handle: tokio::runtime::Handle,
    owner: Arc<OwnerState>,
    local: Option<Arc<LocalSlot>>,
    io_enabled: bool,
    flavor: Flavor,
) -> Arc<HandleContext> {
    Arc::new(HandleContext {
        handle,
        owner,
        local,
        io_enabled,
        flavor,
        #[cfg(tokio_unstable)]
        worker_observer: Mutex::new(None),
    })
}

pub(crate) fn raw_handle(context: Arc<HandleContext>) -> RawHandle {
    context.owner.export_handle(Arc::clone(&context));
    unsafe { RawHandle::from_raw(Arc::as_ptr(&context).cast(), &raw const RUNTIME_API) }
}

pub(super) unsafe extern "C" fn handle_block_on(
    context: *const c_void,
    future: Future,
) -> CallResult {
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<Status, String> {
        let context = unsafe { &*context.cast::<HandleContext>() };
        let activity = context.owner.activity()?;
        let status = context.handle.block_on(GuestFuture::new(future, &activity));
        drop(activity);
        Ok(status)
    }));
    match outcome {
        Ok(Ok(status)) => block_on_result(Ok(status)),
        Ok(Err(error)) => result(Status::Error, OwnedBytes::from_string(error)),
        Err(payload) => host_panic(&*payload),
    }
}
