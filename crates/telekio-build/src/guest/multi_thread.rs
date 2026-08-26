use super::*;
use crate::runtime::context;
use crate::runtime::task::{
    self,
    telekio::{Host, HostSchedule, Registry},
};

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
                .runtime_block_on(context::telekio::active(task::telekio::budget(future))),
            _ => unreachable!("expected MultiThread scheduler"),
        })
    }
}

impl Handle {
    pub(crate) fn install(&self, host: Arc<Host>) {
        self.telekio.install(host);
    }

    pub(super) fn schedule_host_task(self: &Arc<Self>, task: task::Notified<Arc<Self>>, _: bool) {
        task.schedule_host(false);
    }

    pub(super) fn schedule_host_option(self: &Arc<Self>, task: Option<task::Notified<Arc<Self>>>) {
        if let Some(task) = task {
            self.schedule_host_task(task, false);
        }
    }
}

impl HostSchedule for Arc<Handle> {
    fn registry(&self) -> &Registry<Self> {
        &self.telekio
    }

    fn run(&self, task: task::Notified<Self>) {
        let current = context::with_current(|handle| match handle {
            scheduler::Handle::CurrentThread(_) => false,
            scheduler::Handle::MultiThread(handle) => Arc::ptr_eq(handle, self),
        })
        .unwrap_or(false);
        if current {
            context::telekio::enter(|| {
                crate::task::coop::budget(|| self.shared.owned.assert_owner(task).run())
            });
        } else {
            let handle = scheduler::Handle::MultiThread(Arc::clone(self));
            context::enter_runtime(&handle, true, |_| {
                context::telekio::enter(|| {
                    crate::task::coop::budget(|| self.shared.owned.assert_owner(task).run())
                })
            });
        }
    }

    fn enter<R>(&self, call: impl FnOnce() -> R) -> R {
        let handle = scheduler::Handle::MultiThread(Arc::clone(self));
        let _guard = context::try_set_current(&handle);
        call()
    }
}

pub(super) fn host_worker<F>(worker: F)
where
    F: FnOnce() + Send + 'static,
{
    drop(worker);
}
