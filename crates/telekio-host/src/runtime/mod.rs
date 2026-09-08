mod blocking;
mod builder;
mod context;
mod dump;
mod handle;
#[cfg(all(
    any(unix, windows),
    any(
        feature = "net",
        all(unix, any(feature = "process", feature = "signal")),
        all(
            tokio_unstable,
            target_os = "linux",
            feature = "fs",
            feature = "io-uring"
        )
    )
))]
mod io;
#[cfg(not(all(
    any(unix, windows),
    any(
        feature = "net",
        all(unix, any(feature = "process", feature = "signal")),
        all(
            tokio_unstable,
            target_os = "linux",
            feature = "fs",
            feature = "io-uring"
        )
    )
)))]
#[path = "io/other.rs"]
mod io;
mod location;
mod metrics;
mod process;
mod scheduler;
mod signal;
mod task;
mod time;

pub(crate) use handle::{HandleContext, raw_handle};
#[cfg(all(unix, feature = "process"))]
pub use process::reap_process;
pub use task::next_task_id;

use std::{
    cell::UnsafeCell,
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, OnceLock, RwLock, Weak},
    thread::ThreadId,
    time::Duration,
};

use telekio_abi::{
    BuildResult, CallResult, Flavor, Future, OwnedBytes, RawRuntime, RuntimeApi, RuntimeConfig,
    Shutdown, Status,
};

use crate::owner::{self, Owner, OwnerState, owner_state};
use crate::{host_callback, host_panic, result};

use self::{
    builder::build_runtime,
    context::{GuestFuture, block_on_result},
    handle::handle_context,
};

static RUNTIME_API: RuntimeApi = RuntimeApi {
    runtime_block_on,
    handle_block_on: handle::handle_block_on,
    retain_handle: owner::retain_handle,
    release_handle: owner::release_handle,
    release_runtime,
    detach: owner::detach,
    task_id: task::task_id,
    abort: task::abort,
    spawn: task::spawn,
    spawn_local: task::spawn_local,
    spawn_blocking: blocking::spawn_blocking,
    task_panicked: task::panicked,
    trace_leaf: dump::trace_leaf,
    dump: dump::start,
    block_in_place: blocking::block_in_place,
    build,
    clock: time::clock,
    pause: time::pause,
    resume: time::resume,
    advance: time::advance,
    timer: time::timer,
    register_io: io::register,
    register_io_driver: io::register_driver,
    signal: signal::signal,
    reap_process: process::reap_process_abi,
    shutdown,
    defer: scheduler::defer,
    metric: metrics::metric,
    flavor: handle::flavor,
    id: handle::id,
    name: handle::name,
    observe_workers: metrics::observe_workers,
};

pub struct Runtime {
    runtime: Arc<tokio::runtime::Runtime>,
    flavor: Flavor,
}

pub(super) struct RuntimeOwner {
    id: OnceLock<u64>,
    owner: Weak<OwnerState>,
    root_owner: Option<Arc<OwnerState>>,
    kind: RwLock<RuntimeKind>,
}

enum RuntimeKind {
    Runtime(Option<tokio::runtime::Runtime>),
    Local(Arc<LocalSlot>),
    Closed,
}

struct LocalSlot {
    thread: ThreadId,
    runtime: UnsafeCell<Option<tokio::runtime::LocalRuntime>>,
}

// LocalRuntime stays in its originating thread. HandleContext only shares the slot's
// address and checks that thread before every access to the contained runtime.
unsafe impl Send for LocalSlot {}
unsafe impl Sync for LocalSlot {}

impl Runtime {
    #[cfg(feature = "rt-multi-thread")]
    pub fn new() -> std::io::Result<Self> {
        tokio::runtime::Runtime::new().map(Self::from_tokio)
    }

    pub fn from_tokio(runtime: tokio::runtime::Runtime) -> Self {
        let flavor = match runtime.handle().runtime_flavor() {
            tokio::runtime::RuntimeFlavor::CurrentThread => Flavor::CurrentThread,
            tokio::runtime::RuntimeFlavor::MultiThread => Flavor::MultiThread,
            _ => unreachable!("unsupported Tokio runtime flavor"),
        };
        Self {
            runtime: Arc::new(runtime),
            flavor,
        }
    }

    pub fn owner(&self) -> Owner {
        Owner {
            runtime: Arc::clone(&self.runtime),
            handle: handle_context(
                self.runtime.handle().clone(),
                owner_state(),
                None,
                self.flavor,
            ),
        }
    }

    pub fn tokio(&self) -> &tokio::runtime::Runtime {
        &self.runtime
    }
}

#[doc(hidden)]
pub fn build_root(config: RuntimeConfig) -> BuildResult {
    match catch_unwind(AssertUnwindSafe(|| {
        let owner = owner_state();
        match build_runtime(config, Arc::clone(&owner)) {
            Ok((kind, handle, workers)) => {
                let runtime = Arc::new(RuntimeOwner {
                    id: OnceLock::new(),
                    owner: Arc::downgrade(&owner),
                    root_owner: Some(owner),
                    kind: RwLock::new(kind),
                });
                let raw_handle = raw_handle(handle);
                BuildResult::success(
                    unsafe {
                        RawRuntime::from_raw(Arc::into_raw(runtime).cast_mut().cast(), raw_handle)
                    },
                    workers,
                )
            }
            Err(error) => BuildResult::error(result(
                Status::Error,
                OwnedBytes::from_string(error.to_string()),
            )),
        }
    })) {
        Ok(result) => result,
        Err(payload) => BuildResult::error(host_panic(&*payload)),
    }
}

impl RuntimeOwner {
    fn owner(&self) -> Option<Arc<OwnerState>> {
        self.root_owner
            .as_ref()
            .cloned()
            .or_else(|| self.owner.upgrade())
    }

    pub(super) fn is_closed(&self) -> bool {
        matches!(&*self.kind.read().unwrap(), RuntimeKind::Closed)
    }

    pub(super) fn local_open(&self) -> bool {
        matches!(&*self.kind.read().unwrap(), RuntimeKind::Local(_))
    }

    pub(super) fn close(&self, mode: Shutdown, duration: Duration) -> std::io::Result<()> {
        let mut kind = self.kind.write().unwrap();
        match &mut *kind {
            RuntimeKind::Runtime(runtime) => {
                let runtime = runtime.take();
                *kind = RuntimeKind::Closed;
                drop(kind);
                if let Some(runtime) = runtime {
                    shutdown_runtime(runtime, mode, duration);
                }
            }
            RuntimeKind::Local(local) => {
                let runtime = local.take().map_err(std::io::Error::other)?;
                *kind = RuntimeKind::Closed;
                drop(kind);
                if let Some(runtime) = runtime {
                    shutdown_local(runtime, mode, duration);
                }
            }
            RuntimeKind::Closed => {}
        }
        Ok(())
    }
}

impl LocalSlot {
    fn new(runtime: tokio::runtime::LocalRuntime) -> Self {
        Self {
            thread: std::thread::current().id(),
            runtime: UnsafeCell::new(Some(runtime)),
        }
    }

    fn with<R>(&self, call: impl FnOnce(&tokio::runtime::LocalRuntime) -> R) -> Result<R, String> {
        if std::thread::current().id() != self.thread {
            return Err("LocalRuntime was used from another thread".to_owned());
        }
        let runtime = unsafe { &*self.runtime.get() }
            .as_ref()
            .ok_or_else(|| "Tokio runtime has shut down".to_owned())?;
        Ok(call(runtime))
    }

    fn take(&self) -> Result<Option<tokio::runtime::LocalRuntime>, String> {
        if std::thread::current().id() != self.thread {
            return Err("LocalRuntime was used from another thread".to_owned());
        }
        Ok(unsafe { &mut *self.runtime.get() }.take())
    }
}

pub(super) unsafe extern "C" fn release_runtime(owner: *mut c_void) -> CallResult {
    host_callback(|| {
        let runtime = unsafe { &*owner.cast::<RuntimeOwner>() };
        if runtime.root_owner.is_some() {
            drop(unsafe { Arc::from_raw(owner.cast::<RuntimeOwner>()) });
        } else if let Some(owner) = runtime.owner.upgrade() {
            let _runtime = owner.unregister_runtime(*runtime.id.get().unwrap());
        }
    })
}

pub(super) unsafe extern "C" fn build(
    context: *const c_void,
    config: RuntimeConfig,
) -> BuildResult {
    match catch_unwind(AssertUnwindSafe(|| {
        let context = unsafe { &*context.cast::<HandleContext>() };
        let _activity = match context.owner.activity() {
            Ok(activity) => activity,
            Err(error) => {
                return BuildResult::error(result(Status::Error, OwnedBytes::from_string(error)));
            }
        };
        match build_runtime(config, Arc::clone(&context.owner)) {
            Ok((kind, handle, workers)) => {
                let runtime = Arc::new(RuntimeOwner {
                    id: OnceLock::new(),
                    owner: Arc::downgrade(&context.owner),
                    root_owner: None,
                    kind: RwLock::new(kind),
                });
                let id = match context.owner.register_runtime(Arc::clone(&runtime)) {
                    Ok(id) => id,
                    Err(error) => {
                        let closed = std::thread::spawn(move || {
                            runtime.close(Shutdown::Wait, Duration::ZERO)
                        })
                        .join();
                        return BuildResult::error(match closed {
                            Ok(Ok(())) => result(Status::Error, OwnedBytes::from_string(error)),
                            Ok(Err(error)) => {
                                result(Status::Error, OwnedBytes::from_string(error.to_string()))
                            }
                            Err(payload) => host_panic(&*payload),
                        });
                    }
                };
                runtime.id.set(id).unwrap();
                let raw_handle = raw_handle(handle);
                BuildResult::success(
                    unsafe {
                        RawRuntime::from_raw(Arc::as_ptr(&runtime).cast_mut().cast(), raw_handle)
                    },
                    workers,
                )
            }
            Err(error) => BuildResult::error(result(
                Status::Error,
                OwnedBytes::from_string(error.to_string()),
            )),
        }
    })) {
        Ok(result) => result,
        Err(payload) => BuildResult::error(host_panic(&*payload)),
    }
}

pub(super) unsafe extern "C" fn shutdown(
    owner: *mut c_void,
    mode: Shutdown,
    seconds: u64,
    nanoseconds: u32,
) -> CallResult {
    let runtime = unsafe { &*owner.cast::<RuntimeOwner>() };
    match catch_unwind(AssertUnwindSafe(|| {
        runtime
            .close(mode, Duration::new(seconds, nanoseconds))
            .unwrap_or_else(|error| panic!("{error}"));
    })) {
        Ok(()) => result(Status::Ok, OwnedBytes::empty()),
        Err(payload) => host_panic(&*payload),
    }
}

fn shutdown_runtime(runtime: tokio::runtime::Runtime, mode: Shutdown, duration: Duration) {
    match mode {
        Shutdown::Wait => drop(runtime),
        Shutdown::Background => runtime.shutdown_background(),
        Shutdown::Timeout => runtime.shutdown_timeout(duration),
    }
}

fn shutdown_local(runtime: tokio::runtime::LocalRuntime, mode: Shutdown, duration: Duration) {
    match mode {
        Shutdown::Wait => drop(runtime),
        Shutdown::Background => runtime.shutdown_background(),
        Shutdown::Timeout => runtime.shutdown_timeout(duration),
    }
}

pub(super) unsafe extern "C" fn runtime_block_on(owner: *mut c_void, future: Future) -> CallResult {
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<Status, String> {
        let runtime = unsafe { &*owner.cast::<RuntimeOwner>() };
        let owner = runtime
            .owner()
            .ok_or_else(|| "Tokio owner has gone away".to_owned())?;
        let activity = owner.activity()?;
        let kind = runtime.kind.read().unwrap();
        let status = match &*kind {
            RuntimeKind::Runtime(runtime) => runtime
                .as_ref()
                .expect("Tokio runtime has shut down")
                .block_on(GuestFuture::new(future, &activity)),
            RuntimeKind::Local(runtime) => runtime
                .with(|runtime| runtime.block_on(GuestFuture::new(future, &activity)))
                .unwrap_or_else(|error| panic!("{error}")),
            RuntimeKind::Closed => panic!("Tokio runtime has shut down"),
        };
        drop(activity);
        Ok(status)
    }));
    match outcome {
        Ok(Ok(status)) => block_on_result(Ok(status)),
        Ok(Err(error)) => result(Status::Error, OwnedBytes::from_string(error)),
        Err(payload) => host_panic(&*payload),
    }
}
