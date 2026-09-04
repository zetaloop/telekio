use super::*;

pub(crate) fn is_tracing() -> bool {
    Context::is_tracing()
}

pub(crate) fn trace_leaf(root_addr: *const c_void, trace_leaf_addr: *const c_void) {
    Context::try_with_current_trace_leaf_fn(|trace| {
        trace(&TraceMeta {
            root_addr: (!root_addr.is_null()).then_some(root_addr),
            trace_leaf_addr,
        });
    });
}
