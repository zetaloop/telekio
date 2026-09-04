use super::*;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

type Observer = Arc<dyn Fn(usize) + Send + Sync>;

pub(crate) struct WorkerObservers {
    state: Mutex<State>,
    pending: Box<[AtomicBool]>,
}

struct State {
    next_id: u64,
    observers: HashMap<u64, Observer>,
    pending: Vec<Vec<u64>>,
}

impl WorkerObservers {
    pub(crate) fn new(workers: usize) -> Self {
        Self {
            state: Mutex::new(State {
                next_id: 0,
                observers: HashMap::new(),
                pending: (0..workers).map(|_| Vec::new()).collect(),
            }),
            pending: (0..workers).map(|_| AtomicBool::new(false)).collect(),
        }
    }

    fn add(&self, observer: Observer, current: Option<usize>) -> u64 {
        let mut state = self.state.lock().unwrap();
        let id = state.next_id;
        state.next_id += 1;
        state.observers.insert(id, Arc::clone(&observer));
        for (worker, pending) in state.pending.iter_mut().enumerate() {
            if Some(worker) != current {
                pending.push(id);
                self.pending[worker].store(true, Ordering::Release);
            }
        }
        drop(state);
        if let Some(worker) = current {
            observer(worker);
        }
        id
    }

    fn remove(&self, id: u64) {
        let mut state = self.state.lock().unwrap();
        state.observers.remove(&id);
        for pending in &mut state.pending {
            pending.retain(|candidate| *candidate != id);
        }
    }

    fn start(&self, worker: usize) {
        let observers = {
            let mut state = self.state.lock().unwrap();
            state.pending[worker].clear();
            self.pending[worker].store(false, Ordering::Release);
            state.observers.values().cloned().collect::<Vec<_>>()
        };
        for observer in observers {
            observer(worker);
        }
    }

    fn tick(&self, worker: usize) {
        if !self.pending[worker].swap(false, Ordering::Acquire) {
            return;
        }
        let observers = {
            let mut state = self.state.lock().unwrap();
            let pending = std::mem::take(&mut state.pending[worker]);
            pending
                .into_iter()
                .filter_map(|id| state.observers.get(&id).cloned())
                .collect::<Vec<_>>()
        };
        for observer in observers {
            observer(worker);
        }
    }
}

impl Handle {
    pub(crate) fn telekio_add_worker_observer(&self, observer: Observer) -> u64 {
        let current = crate::runtime::context::worker_index();
        let id = self.telekio.add(observer, current);
        for (worker, remote) in self.shared.remotes.iter().enumerate() {
            if Some(worker) != current {
                remote.unpark.unpark(&self.driver);
            }
        }
        id
    }

    pub(crate) fn telekio_remove_worker_observer(&self, id: u64) {
        self.telekio.remove(id);
    }
}

impl Context {
    pub(super) fn telekio_tick(&self, core: &Core) {
        self.assert_lifo_enabled_is_correct(core);
        self.worker.handle.telekio.tick(self.worker.index);
    }

    pub(super) fn telekio_park(&self, core: Box<Core>) -> Box<Core> {
        let core = self.park(core);
        self.worker.handle.telekio.tick(self.worker.index);
        core
    }
}

pub(super) fn enter_worker<F, R>(
    worker: Arc<Worker>,
    guest: F,
) -> impl FnOnce(&mut context::BlockingRegionGuard) -> R
where
    F: FnOnce(&mut context::BlockingRegionGuard) -> R,
{
    move |guard| {
        worker.handle.telekio.start(worker.index);
        guest(guard)
    }
}
