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

    pub(crate) fn install(&self, host: Arc<Host>) {
        self.telekio.install(host);
    }

    fn run_host_task(self: &Arc<Self>, task: task::Notified<Arc<Self>>) {
        #[cfg(tokio_unstable)]
        self.telekio.host().record_worker();
        #[cfg(tokio_unstable)]
        let task_meta = task.task_meta();
        #[cfg(tokio_unstable)]
        self.task_hooks.poll_start_callback(&task_meta);
        self.shared.owned.assert_owner(task).run();
        #[cfg(tokio_unstable)]
        self.task_hooks.poll_stop_callback(&task_meta);
    }

    pub(super) fn schedule_local(self: &Arc<Self>, task: task::Notified<Arc<Self>>) {
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

    fn run(&self, task: task::Notified<Self>) {
        let current = context::with_current(|handle| match handle {
            scheduler::Handle::CurrentThread(handle) => Arc::ptr_eq(handle, self),
            #[cfg(feature = "rt-multi-thread")]
            scheduler::Handle::MultiThread(_) => false,
        })
        .unwrap_or(false);
        if current {
            context::telekio::enter(|| crate::task::coop::budget(|| self.run_host_task(task)));
        } else {
            let handle = scheduler::Handle::CurrentThread(Arc::clone(self));
            context::enter_runtime(&handle, false, |_| {
                context::telekio::enter(|| crate::task::coop::budget(|| self.run_host_task(task)))
            });
        }
    }

    fn enter<R>(&self, call: impl FnOnce() -> R) -> R {
        let handle = scheduler::Handle::CurrentThread(Arc::clone(self));
        let _guard = context::try_set_current(&handle);
        call()
    }
}
