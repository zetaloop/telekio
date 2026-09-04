use super::*;

#[inline(never)]
pub(crate) fn trace_leaf() -> Poll<()> {
    let state = ::telekio::execution_state();
    if state.is_null() || unsafe { (*state).tracing } == 0 {
        return super::trace_leaf();
    }
    let root = Context::current_frame_addr().unwrap_or(std::ptr::null());
    crate::runtime::scheduler::Handle::current()
        .host()
        .trace_leaf(root, trace_leaf as *const c_void)
        .resume("failed to trace Tokio task");
    Poll::Pending
}

pub(crate) fn display_foreign<T>(
    traces: Vec<Vec<T>>,
    formatter: &mut fmt::Formatter<'_>,
) -> fmt::Result
where
    T: Clone + Eq + std::hash::Hash + fmt::Display,
{
    tree::telekio::format(traces, formatter)
}

impl Trace {
    pub(crate) fn from_telekio(
        backtraces: Vec<Vec<crate::runtime::dump::telekio::ForeignFrame>>,
    ) -> Self {
        Self {
            backtraces: Vec::new(),
            foreign: Some(crate::runtime::dump::telekio::ForeignTrace { backtraces }),
        }
    }

    pub(crate) fn telekio_foreign(&self) -> Option<&crate::runtime::dump::telekio::ForeignTrace> {
        self.foreign.as_ref()
    }

    pub(crate) fn telekio_fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.foreign {
            Some(trace) => crate::runtime::dump::telekio::display(trace, formatter),
            None => fmt::Display::fmt(self, formatter),
        }
    }
}
