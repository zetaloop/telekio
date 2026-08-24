use super::*;
use crate::runtime::task::telekio::{Host, HostSchedule, Registry};

impl CurrentThread {
    pub(crate) fn block_on<F: Future>(
        &self,
        handle: &scheduler::Handle,
        future: F,
    ) -> F::Output {
        crate::runtime::context::enter_runtime(handle, false, |_| {
            handle
                .as_current_thread()
                .telekio
                .host()
                .runtime_block_on(future)
        })
    }
}

impl Handle {
    pub(crate) fn install(&self, host: Arc<Host>) {
        self.telekio.install(host);
    }

    pub(super) fn schedule_local(self: &Arc<Self>, task: task::Notified<Arc<Self>>) {
        task.schedule_host(true);
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
            crate::task::coop::budget(|| self.shared.owned.assert_owner(task).run());
        } else {
            let handle = scheduler::Handle::CurrentThread(Arc::clone(self));
            context::enter_runtime(&handle, false, |_| {
                crate::task::coop::budget(|| self.shared.owned.assert_owner(task).run())
            });
        }
    }
}
