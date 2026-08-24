use super::Inject;
use crate::runtime::task::{self, telekio::HostSchedule};

impl<T: HostSchedule> Inject<T> {
    pub(crate) fn push_host_task(&self, task: task::Notified<T>) {
        task.schedule_host(false);
    }
}
