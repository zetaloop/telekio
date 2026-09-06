use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::runtime::{
    task::{self, SpawnLocation},
    TaskHooks, TaskMeta,
};

pub(crate) struct Hooks {
    hooks: TaskHooks,
    tasks: Mutex<HashMap<task::Id, TaskHookState>>,
    active: bool,
}

struct TaskHookState {
    location: SpawnLocation,
    // Rescheduling can overlap one poll's start with the preceding poll's stop.
    polls: usize,
    terminated: bool,
}

impl TaskHooks {
    pub(crate) fn spawn_host(&self, _: &TaskMeta<'_>) {}
}

impl Hooks {
    #[cfg(not(test))]
    pub(crate) fn empty() -> Arc<Self> {
        Self::new(TaskHooks {
            task_spawn_callback: None,
            task_terminate_callback: None,
            #[cfg(tokio_unstable)]
            before_poll_callback: None,
            #[cfg(tokio_unstable)]
            after_poll_callback: None,
        })
    }

    pub(crate) fn new(hooks: TaskHooks) -> Arc<Self> {
        let active = Self::hooks_active(&hooks);
        Arc::new(Self {
            hooks,
            tasks: Mutex::new(HashMap::new()),
            active,
        })
    }

    fn hooks_active(hooks: &TaskHooks) -> bool {
        hooks.task_spawn_callback.is_some() || hooks.task_terminate_callback.is_some() || {
            #[cfg(tokio_unstable)]
            {
                hooks.before_poll_callback.is_some() || hooks.after_poll_callback.is_some()
            }
            #[cfg(not(tokio_unstable))]
            {
                false
            }
        }
    }

    pub(crate) fn callback(self: &Arc<Self>) -> ::telekio::TaskCallback {
        if !self.active {
            return ::telekio::TaskCallback::none();
        }
        let hooks = Arc::clone(self);
        ::telekio::TaskCallback::from_arc(Arc::new(move |event, id| hooks.call(event, id)))
    }

    pub(crate) fn register(&self, id: task::Id, location: SpawnLocation) {
        if !self.active {
            return;
        }
        let previous = self.tasks.lock().unwrap().insert(
            id,
            TaskHookState {
                location,
                polls: 0,
                terminated: false,
            },
        );
        assert!(previous.is_none(), "Tokio task hook was registered twice");
    }

    pub(crate) fn remove(&self, id: task::Id) {
        if self.active {
            self.tasks.lock().unwrap().remove(&id);
        }
    }

    fn call(&self, event: ::telekio::TaskEvent, id: u64) {
        let id = task::Id::from_telekio(id);
        let mut tasks = self.tasks.lock().unwrap();
        let state = tasks
            .get_mut(&id)
            .unwrap_or_else(|| panic!("Tokio task hook {event:?} received unknown task {id:?}"));
        let spawned_at = state.location;
        match event {
            ::telekio::TaskEvent::PollStart => state.polls += 1,
            ::telekio::TaskEvent::PollStop => state.polls -= 1,
            ::telekio::TaskEvent::Terminate => state.terminated = true,
            ::telekio::TaskEvent::Spawn => {}
        }
        if state.terminated && state.polls == 0 {
            tasks.remove(&id);
        }
        drop(tasks);
        let meta = TaskMeta {
            id,
            spawned_at,
            _phantom: Default::default(),
        };
        match event {
            ::telekio::TaskEvent::Spawn => self.hooks.spawn(&meta),
            ::telekio::TaskEvent::PollStart => {
                #[cfg(tokio_unstable)]
                self.hooks.poll_start_callback(&meta);
            }
            ::telekio::TaskEvent::PollStop => {
                #[cfg(tokio_unstable)]
                self.hooks.poll_stop_callback(&meta);
            }
            ::telekio::TaskEvent::Terminate => {
                if let Some(callback) = &self.hooks.task_terminate_callback {
                    callback(&meta);
                }
            }
        }
    }
}
