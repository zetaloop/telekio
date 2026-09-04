use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::runtime::{TaskHooks, TaskMeta, task};

use super::super::SpawnLocation;

pub(crate) struct HostTaskHooks {
    hooks: TaskHooks,
    tasks: Mutex<HashMap<task::Id, TaskHookState>>,
    active: bool,
}

struct TaskHookState {
    location: SpawnLocation,
    polling: bool,
    terminated: bool,
}

thread_local! {
    static SPAWN_LOCATION: std::cell::Cell<Option<::telekio::SourceLocation>> = const { std::cell::Cell::new(None) };
}

impl SpawnLocation {
    #[track_caller]
    pub(crate) fn capture() -> Self {
        SPAWN_LOCATION.set(Some(::telekio::SourceLocation::caller()));
        Self::capture_local()
    }

    pub(crate) fn take_telekio() -> ::telekio::SourceLocation {
        SPAWN_LOCATION
            .take()
            .expect("Tokio spawn location is missing")
    }
}

impl HostTaskHooks {
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
        hooks.task_spawn_callback.is_some()
            || hooks.task_terminate_callback.is_some()
            || {
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

    pub(super) fn register(&self, id: task::Id, location: SpawnLocation) {
        if !self.active {
            return;
        }
        let previous = self.tasks.lock().unwrap().insert(
            id,
            TaskHookState {
                location,
                polling: false,
                terminated: false,
            },
        );
        assert!(previous.is_none(), "Tokio task hook was registered twice");
    }

    pub(super) fn remove(&self, id: task::Id) {
        if self.active {
            self.tasks.lock().unwrap().remove(&id);
        }
    }

    fn call(&self, event: ::telekio::TaskEvent, id: u64) {
        let id = task::Id::from_telekio(id);
        let mut tasks = self.tasks.lock().unwrap();
        let state = tasks
            .get_mut(&id)
            .expect("Tokio task hook received an unknown task");
        let spawned_at = state.location;
        match event {
            ::telekio::TaskEvent::PollStart => state.polling = true,
            ::telekio::TaskEvent::PollStop => state.polling = false,
            ::telekio::TaskEvent::Terminate => state.terminated = true,
            ::telekio::TaskEvent::Spawn => {}
        }
        if state.terminated && !state.polling {
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

