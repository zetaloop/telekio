#[cfg(target_has_atomic = "64")]
use crate::runtime::handle::telekio::Connection;
#[cfg(target_has_atomic = "64")]
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, OnceLock},
    task::{Context, Poll},
};

#[cfg(target_has_atomic = "64")]
pub(crate) struct Workers {
    threads: Mutex<Vec<Option<std::thread::ThreadId>>>,
    observing: OnceLock<()>,
}

#[cfg(target_has_atomic = "64")]
pub(crate) struct Root<'a, F> {
    future: F,
    connection: &'a Connection,
}

pub(crate) fn worker_index(handle: &::telekio::Handle) -> Option<usize> {
    usize::try_from(
        handle
            .metric(::telekio::Metric::CurrentWorkerIndex, 0)
            .checked_sub(1)?,
    )
    .ok()
}

#[cfg(target_has_atomic = "64")]
pub(crate) fn record_worker(connection: &Connection) {
    if let Some(worker) = worker_index(&connection.handle) {
        connection.workers.store(worker);
    }
}

#[cfg(target_has_atomic = "64")]
pub(crate) fn root<F: Future>(connection: &Connection, future: F) -> Root<'_, F> {
    Root { future, connection }
}

#[cfg(target_has_atomic = "64")]
impl<F: Future> Future for Root<'_, F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        record_worker(this.connection);
        unsafe { Pin::new_unchecked(&mut this.future) }.poll(context)
    }
}

#[cfg(target_has_atomic = "64")]
impl Workers {
    pub(crate) fn new() -> Self {
        Self {
            threads: Mutex::new(Vec::new()),
            observing: OnceLock::new(),
        }
    }

    fn store(&self, worker: usize) {
        let mut threads = self.threads.lock().unwrap();
        if threads.len() <= worker {
            threads.resize(worker + 1, None);
        }
        threads[worker] = Some(std::thread::current().id());
    }

    pub(crate) fn thread_id(
        self: &Arc<Self>,
        handle: &::telekio::Handle,
        worker: usize,
    ) -> Option<std::thread::ThreadId> {
        assert!(worker < handle.metric(::telekio::Metric::NumWorkers, 0) as usize);
        self.observing.get_or_init(|| {
            let workers = Arc::clone(self);
            let callback = ::telekio::WorkerCallback::from_arc(Arc::new(move |worker| {
                workers.store(worker);
            }));
            handle
                .observe_workers(callback)
                .resume("failed to observe Tokio worker threads");
        });
        self.threads.lock().unwrap().get(worker).copied().flatten()
    }
}
