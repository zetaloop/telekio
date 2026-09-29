use std::{error::Error, path::Path};

use r#override::{item, root};

use super::{mount, patch};

pub(super) fn patch_task_id(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        source.select(item("Id::next"))?.rename("next_local")?;
        source
            .select(item("Id::next_local"))?
            .add_attribute("#[expect(dead_code)]")?;
        mount(source, None, "telekio", "guest/runtime/task/id.rs")
    })
}

pub(super) fn patch_task(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        source
            .select(item("SpawnLocation::capture"))?
            .rename("capture_local")?;
        mount(
            source,
            Some("pub(crate)"),
            "telekio",
            "guest/runtime/task/mod.rs",
        )
    })
}

pub(super) fn patch_task_trace(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        source
            .select(item("trace_leaf").call("Context::try_with_current_trace_leaf_fn"))?
            .redirect("telekio::trace_leaf")?;
        source
            .select(item("Context::is_tracing"))?
            .add_attribute("#[expect(dead_code)]")?;
        source
            .select(item("Trace"))?
            .add_field("foreign: Option<crate::runtime::dump::telekio::ForeignTrace>")?;
        source
            .select(item("Trace::empty").record("Self"))?
            .add_field("foreign: None")?;
        mount(
            source,
            Some("pub(crate)"),
            "telekio",
            "guest/runtime/task/trace/mod.rs",
        )
    })
}

pub(super) fn patch_task_trace_tree(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        mount(
            source,
            Some("pub(super)"),
            "telekio",
            "guest/runtime/task/trace/tree.rs",
        )
    })
}

pub(super) fn patch_dump(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        source
            .select(item("Trace::resolve_backtraces"))?
            .rename("resolve_backtraces_local")?;
        source
            .select(item("Trace::resolve_backtraces_local"))?
            .add_attribute("#[doc(hidden)]")?;
        source
            .select(item("Trace::fmt").call("fmt"))?
            .redirect("telekio_fmt")?;
        mount(
            source,
            Some("pub(crate)"),
            "telekio",
            "guest/runtime/dump.rs",
        )
    })
}

pub(super) fn patch_task_list(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        for name in ["bind", "bind_local", "bind_inner", "spawned_tasks_count"] {
            source
                .select(root().implementation("OwnedTasks<S>").item(name))?
                .add_attribute("#[expect(dead_code)]")?;
        }
        Ok(())
    })
}

pub(super) fn patch_sharded_list(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        source
            .select(item("ShardedList"))?
            .add_attribute("#[cfg_attr(not(tokio_unstable), expect(dead_code))]")?;
        for target in [
            item("ShardGuard"),
            root().implementation("ShardedList<L>").item("lock_shard"),
            root().implementation("ShardGuard<'a, L>").item("push"),
            root().implementation("ShardedList<L>").item("added"),
        ] {
            source
                .select(target)?
                .add_attribute("#[expect(dead_code)]")?;
        }
        Ok(())
    })
}

pub(super) fn host(source: &Path) -> Result<(), Box<dyn Error>> {
    patch(&source.join("src/runtime/task/id.rs"), |contents| {
        contents.select(item("Id::next"))?.rename("next_local")?;
        mount(contents, None, "telekio", "host/runtime/task/id.rs")
    })?;
    patch(&source.join("src/runtime/task/mod.rs"), |contents| {
        contents
            .select(item("SpawnLocation::capture"))?
            .rename("capture_local")
    })
}
