use std::{error::Error, path::Path};

use r#override::{item, root};

use super::{mount, patch};

pub(super) fn patch_panicking(root: &Path) -> Result<(), Box<dyn Error>> {
    for (file, scope) in [
        ("runtime/blocking/shutdown.rs", item("Receiver::wait")),
        (
            "runtime/context/current.rs",
            item("SetCurrentGuard::drop")
                .call("with")
                .child(r#override::root().closure()),
        ),
        (
            "runtime/scheduler/current_thread/mod.rs",
            item("CurrentThread::shutdown"),
        ),
        (
            "runtime/scheduler/multi_thread/queue.rs",
            r#override::root().implementation("Local<T>").item("drop"),
        ),
        (
            "runtime/scheduler/multi_thread/worker.rs",
            item("AbortOnPanic::drop"),
        ),
        (
            "util/idle_notified_set.rs",
            r#override::root()
                .implementation("IdleNotifiedSet<T>")
                .item("drop"),
        ),
    ] {
        patch(&root.join("src").join(file), |source| {
            source
                .select(scope.call("std::thread::panicking"))?
                .redirect("crate::runtime::context::telekio::panicking")
        })?;
    }
    Ok(())
}

pub(super) fn patch_builder(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        source
            .select(item("Builder::build").call("build_current_thread_runtime"))?
            .redirect("build_hosted_current_thread")?;
        source
            .select(item("Builder::build").call("build_threaded_runtime"))?
            .redirect("build_hosted_multi_thread")?;
        source
            .select(item("Builder::build_local").call("build_current_thread_local_runtime"))?
            .redirect("build_hosted_local")?;
        mount(source, None, "telekio", "guest/runtime/builder.rs")
    })
}

pub(super) fn patch_runtime(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        for variant in ["Scheduler::CurrentThread", "Scheduler::MultiThread"] {
            source
                .select(
                    item("Runtime::block_on_inner")
                        .arm(variant)
                        .call("block_on"),
                )?
                .delegate("telekio::block_on", &["&self.blocking_pool"])?;
        }
        mount(
            source,
            Some("pub(crate)"),
            "telekio",
            "guest/runtime/runtime.rs",
        )
    })
}

pub(super) fn patch_local_runtime(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        source
            .select(item("LocalRuntime::block_on_inner").call("block_on"))?
            .delegate(
                "crate::runtime::runtime::telekio::block_on",
                &["&self.blocking_pool"],
            )?;
        mount(
            source,
            None,
            "telekio",
            "guest/runtime/local_runtime/runtime.rs",
        )
    })
}

pub(super) fn patch_blocking(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        source
            .select(item("BlockingPool"))?
            .add_field("telekio: std::sync::OnceLock<::telekio_abi::Runtime>")?;
        source
            .select(item("BlockingPool::new").record("BlockingPool"))?
            .add_field("telekio: std::sync::OnceLock::new()")?;
        source
            .select(item("BlockingPool::shutdown"))?
            .rename("shutdown_workers")?;
        source
            .select(item("Mandatory"))?
            .add_attribute("#[cfg_attr(not(tokio_unstable), expect(dead_code))]")?;
        source
            .select(root().implementation("Spawner").has(item("spawn_blocking")))?
            .add_attribute("#[expect(dead_code)]")?;
        source
            .select(item("SpawnerMetrics::queue_depth"))?
            .add_attribute("#[expect(dead_code)]")?;
        source
            .select(root().implementation("Spawner").has(item("num_threads")))?
            .add_attribute("#[expect(dead_code)]")?;
        mount(source, None, "telekio", "guest/runtime/blocking/pool.rs")
    })?;
    patch(
        &path
            .parent()
            .and_then(Path::parent)
            .ok_or("blocking path has no runtime parent")?
            .join("handle.rs"),
        |source| {
            source
                .select(item("Handle::spawn_blocking").call("spawn_blocking"))?
                .redirect("spawn_host_blocking")
        },
    )
}

pub(super) fn patch_context(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        mount(source, None, "telekio", "guest/runtime/context/blocking.rs")
    })?;
    patch(
        &path
            .parent()
            .and_then(Path::parent)
            .ok_or("context path has no runtime parent")?
            .join("handle.rs"),
        |source| {
            source
                .select(item("Handle::runtime_flavor"))?
                .rename("runtime_flavor_inner")?;
            source
                .select(item("Handle::runtime_flavor_inner"))?
                .set_visibility("pub(crate)")?;
            source
                .select(item("Handle::dump"))?
                .delegate("crate::runtime::dump::telekio::dump", &["self"])?;
            source
                .select(item("Handle::is_tracing").call("super::task::trace::Context::is_tracing"))?
                .redirect("telekio::is_tracing")?;
            source
                .select(
                    item("Handle::block_on_inner")
                        .call("context::enter_runtime")
                        .child(root().closure())
                        .call("block_on"),
                )?
                .redirect("block_on_host")?;
            source
                .select(item("Handle::block_on_inner").call("context::enter_runtime"))?
                .redirect("crate::runtime::context::telekio::block_on")?;
            mount(
                source,
                Some("pub(crate)"),
                "telekio",
                "guest/runtime/handle.rs",
            )
        },
    )
}

pub(super) fn patch_defer(generated: &Path) -> Result<(), Box<dyn Error>> {
    patch(&generated.join("src/runtime/context.rs"), |source| {
        mount(
            source,
            Some("pub(crate)"),
            "telekio",
            "guest/runtime/context.rs",
        )?;
        source
            .select(
                item("worker_index")
                    .call("with_scheduler")
                    .child(root().closure()),
            )?
            .delegate("telekio::worker_index", &[])
    })?;
    patch_runtime_context(generated)?;
    patch(&generated.join("src/runtime/context.rs"), |source| {
        source
            .select(root().import("exit_runtime"))?
            .redirect("runtime_mt::telekio::exit_runtime")
    })?;
    patch(
        &generated.join("src/runtime/context/runtime_mt.rs"),
        |source| {
            source
                .select(item("exit_runtime"))?
                .add_attribute("#[expect(dead_code)]")?;
            mount(
                source,
                Some("pub(crate)"),
                "telekio",
                "guest/runtime/context/runtime_mt.rs",
            )
        },
    )?;
    patch(&generated.join("src/task/yield_now.rs"), |source| {
        source
            .select(
                item("yield_now")
                    .call("poll_fn")
                    .child(root().closure())
                    .call("context::defer"),
            )?
            .redirect("crate::runtime::context::telekio::defer")?;
        source.select(root().import("context"))?.remove()
    })?;
    patch(&generated.join("src/task/coop/mod.rs"), |source| {
        source
            .select(item("register_waker").call("context::defer"))?
            .redirect("crate::runtime::context::telekio::defer")
    })
}

pub(super) fn host(source: &Path) -> Result<(), Box<dyn Error>> {
    patch_runtime_context(source)
}

fn patch_runtime_context(source: &Path) -> Result<(), Box<dyn Error>> {
    patch(&source.join("src/runtime/context.rs"), |contents| {
        for (function, call, helper, context) in [
            ("thread_rng_n", "with", "telekio::rng", &[][..]),
            ("budget", "try_with", "telekio::budget", &[][..]),
            (
                "set_current_task_id",
                "try_with",
                "telekio::set_task_id",
                &["id"][..],
            ),
            ("current_task_id", "try_with", "telekio::task_id", &[][..]),
        ] {
            contents
                .select(item(function).call(call).child(root().closure()))?
                .delegate(helper, context)?;
        }
        Ok(())
    })?;
    for (file, calls, helper) in [
        (
            "context.rs",
            vec![item("with_scheduler").call("try_with")],
            "runtime",
        ),
        (
            "context/runtime.rs",
            vec![
                item("enter_runtime").call("with"),
                item("EnterRuntimeGuard::drop").call("with"),
            ],
            "enter_runtime",
        ),
        (
            "context/runtime_mt.rs",
            vec![
                item("current_enter_context").call("with"),
                item("exit_runtime").body().child(root().call("with")),
                item("Reset::drop").call("with"),
            ],
            "runtime",
        ),
        (
            "context/blocking.rs",
            vec![
                item("try_enter_blocking_region").call("try_with"),
                item("disallow_block_in_place").call("try_with"),
            ],
            "runtime",
        ),
        (
            "context/blocking.rs",
            vec![item("DisallowBlockInPlaceGuard::drop").call("with")],
            "runtime",
        ),
    ] {
        patch(&source.join("src/runtime").join(file), |contents| {
            for call in calls {
                contents
                    .select(call.child(root().closure()))?
                    .delegate(&format!("crate::runtime::context::telekio::{helper}"), &[])?;
            }
            Ok(())
        })?;
    }
    Ok(())
}
