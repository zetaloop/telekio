use super::*;

impl Handle {
    pub(crate) async fn dump(&self) -> Dump {
        let tasks = self
            .telekio
            .dump()
            .await
            .into_iter()
            .map(|(id, trace)| crate::runtime::dump::Task::new(id, trace))
            .collect();
        Dump::new(tasks)
    }
}
