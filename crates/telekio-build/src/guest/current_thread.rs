use super::*;
use crate::runtime::task::telekio::{Host, HostSchedule, Registry};
use std::num::NonZeroU64;

impl CurrentThread {
    #[track_caller]
    pub(crate) fn block_on<F: Future>(&self, handle: &scheduler::Handle, future: F) -> F::Output {
        crate::runtime::context::enter_runtime(handle, false, |_| {
            handle
                .as_current_thread()
                .telekio
                .host()
                .runtime_block_on(context::telekio::active(task::telekio::budget(future)))
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

    fn run_host_task(&self, task: task::Notified<Arc<Self>>) -> u64 {
        #[cfg(tokio_unstable)]
        self.telekio.host().record_worker();
        task::telekio::measure_poll(|| {
            #[cfg(tokio_unstable)]
            let task_meta = task.telekio_task_meta();
            #[cfg(tokio_unstable)]
            self.task_hooks.poll_start_callback(&task_meta);
            self.shared.owned.assert_owner(task).run();
            #[cfg(tokio_unstable)]
            self.task_hooks.poll_stop_callback(&task_meta);
        })
    }

    pub(super) fn schedule_local(&self, task: task::Notified<Arc<Self>>) {
        task.schedule_host(true);
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

impl Core {
    pub(super) fn push_host_task(&mut self, _: &Handle, task: Notified) {
        task.schedule_host(false);
    }
}

impl HostSchedule for Arc<Handle> {
    fn registry(&self) -> &Registry<Self> {
        &self.telekio
    }

    fn run(&self, task: task::Notified<Self>) -> u64 {
        let current = context::with_current(|handle| match handle {
            scheduler::Handle::CurrentThread(handle) => Arc::ptr_eq(handle, self),
            #[cfg(feature = "rt-multi-thread")]
            scheduler::Handle::MultiThread(_) => false,
        })
        .unwrap_or(false);
        if current {
            context::telekio::enter(|| crate::task::coop::budget(|| self.run_host_task(task)))
        } else {
            let handle = scheduler::Handle::CurrentThread(Arc::clone(self));
            context::enter_runtime(&handle, false, |_| {
                context::telekio::enter(|| crate::task::coop::budget(|| self.run_host_task(task)))
            })
        }
    }

    fn enter<R>(&self, call: impl FnOnce() -> R) -> R {
        let handle = scheduler::Handle::CurrentThread(Arc::clone(self));
        let _guard = context::try_set_current(&handle);
        call()
    }
}
