#[cfg(all(tokio_unstable, target_has_atomic = "64"))]
use std::sync::{Arc, Mutex};
#[cfg(tokio_unstable)]
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use super::Host;

#[cfg(all(tokio_unstable, target_has_atomic = "64"))]
pub(super) struct Workers {
    threads: Mutex<Vec<Option<std::thread::ThreadId>>>,
}

#[cfg(tokio_unstable)]
pub(super) struct Root<'a, F> {
    future: F,
    host: &'a Host,
}

pub(crate) fn measure_poll(call: impl FnOnce()) -> u64 {
    let started = std::time::Instant::now();
    call();
    started.elapsed().as_nanos().min(u64::MAX.into()) as u64
}

#[cfg(tokio_unstable)]
impl<F: Future> Future for Root<'_, F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        #[cfg(target_has_atomic = "64")]
        this.host.record_worker();
        unsafe { Pin::new_unchecked(&mut this.future) }.poll(context)
    }
}

#[cfg(all(tokio_unstable, target_has_atomic = "64"))]
impl Workers {
    pub(super) fn new() -> Self {
        Self {
            threads: Mutex::new(Vec::new()),
        }
    }

    fn store(&self, worker: usize) {
        let mut threads = self.threads.lock().unwrap();
        if threads.len() <= worker {
            threads.resize(worker + 1, None);
        }
        threads[worker] = Some(std::thread::current().id());
    }

    fn get(&self, worker: usize) -> Option<std::thread::ThreadId> {
        self.threads.lock().unwrap().get(worker).copied().flatten()
    }
}

impl Host {
    #[cfg(feature = "taskdump")]
    pub(crate) async fn dump(&self) -> crate::runtime::Dump {
        let ::telekio::DumpResult { call, mut dump } = self.handle.dump();
        call.resume("failed to start Tokio runtime dump");
        let bytes = std::future::poll_fn(|context| {
            let waker = unsafe { ::telekio::Waker::from_ref(context.waker()) };
            dump.poll(&waker)
        })
        .await;
        crate::runtime::dump::telekio::decode(&bytes)
    }

    #[cfg(feature = "taskdump")]
    pub(crate) fn trace_leaf(
        &self,
        root: *const std::ffi::c_void,
        leaf: *const std::ffi::c_void,
    ) -> ::telekio::CallResult {
        unsafe { self.handle.trace_leaf(root, leaf) }
    }

    pub(crate) fn metric(&self, metric: ::telekio::Metric, worker: usize) -> u64 {
        self.handle.metric(metric, worker)
    }

    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    pub(crate) fn metric_bucket(
        &self,
        metric: ::telekio::Metric,
        worker: usize,
        bucket: usize,
    ) -> u64 {
        self.handle.metric_bucket(metric, worker, bucket)
    }

    #[cfg(tokio_unstable)]
    pub(crate) fn worker_index(&self) -> Option<usize> {
        usize::try_from(
            self.metric(::telekio::Metric::CurrentWorkerIndex, 0)
                .checked_sub(1)?,
        )
        .ok()
    }

    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    fn observe_workers(self: &Arc<Self>) {
        self.observing.get_or_init(|| {
            let workers = Arc::clone(&self.workers);
            let callback = ::telekio::WorkerCallback::from_arc(Arc::new(move |worker| {
                workers.store(worker);
            }));
            self.handle
                .observe_workers(callback)
                .resume("failed to observe Tokio worker threads");
        });
    }

    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    pub(crate) fn record_worker(&self) {
        if let Some(worker) = self.worker_index() {
            self.store_worker(worker);
        }
    }

    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    fn store_worker(&self, worker: usize) {
        self.workers.store(worker);
    }

    #[cfg(all(tokio_unstable, target_has_atomic = "64"))]
    pub(crate) fn worker_thread_id(
        self: &Arc<Self>,
        worker: usize,
    ) -> Option<std::thread::ThreadId> {
        assert!(worker < self.metric(::telekio::Metric::NumWorkers, 0) as usize);
        self.observe_workers();
        self.workers.get(worker)
    }

    #[cfg(tokio_unstable)]
    pub(super) fn root<F: Future>(&self, future: F) -> Root<'_, F> {
        Root { future, host: self }
    }
}
