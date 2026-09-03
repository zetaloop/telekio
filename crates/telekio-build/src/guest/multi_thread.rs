use super::*;
use crate::runtime::context;
use crate::runtime::TaskMeta;
use crate::runtime::task::{
    self,
    telekio::{Host, HostSchedule, Registry},
};
use std::num::NonZeroU64;

impl MultiThread {
    #[track_caller]
    pub(crate) fn block_on<F>(&self, handle: &scheduler::Handle, future: F) -> F::Output
    where
        F: Future,
    {
        crate::runtime::context::enter_runtime(handle, true, |_| match handle {
            scheduler::Handle::MultiThread(handle) => handle
                .telekio
                .host()
                .runtime_block_on(context::telekio::active(future)),
            _ => unreachable!("expected MultiThread scheduler"),
        })
    }
}

impl Handle {
    pub(crate) fn owned_id(&self) -> NonZeroU64 {
        let _ = self.owned_id_inner();
        NonZeroU64::new(self.telekio.host().id()).expect("invalid runtime ID")
    }

    pub(crate) fn install(&self, host: std::sync::Arc<Host>) {
        self.telekio.install(host);
    }

}

impl HostSchedule for Arc<Handle> {
    fn registry(&self) -> &Registry<Self> {
        &self.telekio
    }

    fn run(&self, _meta: &TaskMeta<'_>, call: impl FnOnce()) -> u64 {
        let run = || {
            #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
            self.telekio.host().record_worker();
            task::telekio::measure_poll(|| {
                #[cfg(tokio_unstable)]
                self.task_hooks.poll_start_callback(_meta);
                call();
                #[cfg(tokio_unstable)]
                self.task_hooks.poll_stop_callback(_meta);
            })
        };
        let current = context::with_current(|handle| match handle {
            scheduler::Handle::CurrentThread(_) => false,
            scheduler::Handle::MultiThread(handle) => Arc::ptr_eq(handle, self),
        })
        .unwrap_or(false);
        if current {
            context::telekio::enter(run)
        } else {
            let handle = scheduler::Handle::MultiThread(Arc::clone(self));
            context::enter_runtime(&handle, true, |_| context::telekio::enter(run))
        }
    }

    fn enter<R>(&self, call: impl FnOnce() -> R) -> R {
        let handle = scheduler::Handle::MultiThread(Arc::clone(self));
        let _guard = context::try_set_current(&handle);
        call()
    }

    fn spawn(&self, meta: &TaskMeta<'_>) {
        self.task_hooks.spawn(meta);
    }
}

pub(super) fn host_worker<F>(worker: F)
where
    F: FnOnce() + Send + 'static,
{
    drop(worker);
}
