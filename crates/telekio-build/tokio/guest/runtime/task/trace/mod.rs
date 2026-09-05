use super::*;

pub(super) fn trace_leaf<F, R>(capture: F) -> Option<R>
where
    F: for<'a> FnOnce(&'a mut dyn FnMut(&TraceMeta)) -> R,
{
    let state = ::telekio::execution_state();
    if state.is_null() || unsafe { (*state).tracing } == 0 {
        return Context::try_with_current_trace_leaf_fn(capture);
    }
    Some(capture(&mut |meta| {
        unsafe {
            crate::runtime::scheduler::Handle::current()
                .connection()
                .handle
                .trace_leaf(
                    meta.root_addr.unwrap_or(std::ptr::null()),
                    meta.trace_leaf_addr,
                )
        }
        .resume("failed to trace Tokio task");
    }))
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
