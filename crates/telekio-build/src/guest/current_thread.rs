use super::*;
use crate::runtime::{TaskMeta, task::telekio::{Host, HostSchedule, Registry}};
use std::num::NonZeroU64;

impl CurrentThread {
    #[track_caller]
    pub(crate) fn block_on<F: Future>(&self, handle: &scheduler::Handle, future: F) -> F::Output {
        crate::runtime::context::enter_runtime(handle, false, |_| {
            handle
                .as_current_thread()
                .telekio
                .host()
                .runtime_block_on(context::telekio::active(future))
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

    #[cfg(feature = "taskdump")]
    pub(crate) fn dump(&self) -> crate::runtime::Dump {
        let tasks = self
            .telekio
            .dump_current()
            .into_iter()
            .map(|(id, trace)| crate::runtime::dump::Task::new(id, trace))
            .collect();
        crate::runtime::Dump::new(tasks)
    }

}

#[cfg(tokio_unstable)]
pub(super) fn unhandled_panic<'a, F>(
    handle: &'a Arc<Handle>,
    _: F,
) -> impl FnOnce(Option<&scheduler::Context>) + 'a {
    move |_| {
        handle.shared.owned.close_and_shutdown_all(0);
        handle.telekio.host().unhandled_panic();
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
            scheduler::Handle::CurrentThread(handle) => Arc::ptr_eq(handle, self),
            #[cfg(feature = "rt-multi-thread")]
            scheduler::Handle::MultiThread(_) => false,
        })
        .unwrap_or(false);
        if current {
            context::telekio::enter(run)
        } else {
            let handle = scheduler::Handle::CurrentThread(Arc::clone(self));
            context::enter_runtime(&handle, false, |_| context::telekio::enter(run))
        }
    }

    fn enter<R>(&self, call: impl FnOnce() -> R) -> R {
        let handle = scheduler::Handle::CurrentThread(Arc::clone(self));
        let _guard = context::try_set_current(&handle);
        call()
    }

    fn spawn(&self, meta: &TaskMeta<'_>) {
        self.task_hooks.spawn(meta);
    }
}
