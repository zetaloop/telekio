use std::{error::Error, path::Path};

use r#override::{item, root};

use super::{mount, patch};

pub(super) fn patch_current_thread(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        source.select(item("Handle"))?.add_field("pub(crate) telekio: std::sync::OnceLock<Arc<crate::runtime::handle::telekio::Connection>>")?;
        source
            .select(item("CurrentThread::new").record("Handle"))?
            .add_field("telekio: std::sync::OnceLock::new()")?;
        for (method, bind) in [("spawn", "bind"), ("spawn_local", "bind_local")] {
            source
                .select(root().implementation("Handle").item(method).call(bind))?
                .redirect(&format!("{bind}_host"))?;
            source
                .select(root().implementation("Handle").item(method).call("spawn"))?
                .redirect("spawn_host")?;
        }
        for target in [
            item("CurrentThread::block_on"),
            item("Core"),
            root().implementation("Core").has(item("tick")),
            root().implementation("Context").has(item("run_task")),
            root()
                .implementation("Handle")
                .has(item("next_remote_task")),
            root().implementation("CoreGuard<'_>").item("block_on"),
        ] {
            source
                .select(target)?
                .add_attribute("#[expect(dead_code)]")?;
        }
        source
            .select(item("Handle::spawned_tasks_count"))?
            .add_attribute("#[expect(dead_code)]")?;
        source
            .select(item("Handle::owned_id"))?
            .rename("owned_id_inner")?;
        for name in [
            "worker_local_queue_depth",
            "num_blocking_threads",
            "num_idle_blocking_threads",
            "blocking_queue_depth",
        ] {
            source
                .select(root().implementation("Handle").item(name))?
                .add_attribute("#[expect(dead_code)]")?;
        }
        mount(
            source,
            None,
            "telekio",
            "guest/runtime/scheduler/current_thread/mod.rs",
        )
    })
}

pub(super) fn patch_multi_thread(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(&path.join("handle.rs"), |source| {
        source
            .select(item("Handle::owned_id"))?
            .rename("owned_id_inner")?;
        source
            .select(item("Handle::bind_new_task").call("bind"))?
            .redirect("bind_host")?;
        source
            .select(item("Handle::bind_new_task").call("spawn"))?
            .redirect("spawn_host")?;
        source.select(item("Handle"))?.add_field("pub(crate) telekio: std::sync::OnceLock<Arc<crate::runtime::handle::telekio::Connection>>")
    })?;
    patch(&path.join("worker.rs"), |source| {
        source
            .select(item("create").record("Handle"))?
            .add_field("telekio: std::sync::OnceLock::new()")?;
        source
            .select(item("Launch::launch").call("runtime::spawn_blocking"))?
            .redirect("super::telekio::host_worker")?;
        source
            .select(item("block_in_place").call("coop::stop"))?
            .redirect("crate::runtime::context::telekio::stop")
    })?;
    patch(&path.join("handle/metrics.rs"), |source| {
        for name in [
            "injection_queue_depth",
            "num_workers",
            "num_alive_tasks",
            "spawned_tasks_count",
        ] {
            source
                .select(root().implementation("Handle").item(name))?
                .add_attribute("#[expect(dead_code)]")?;
        }
        source
            .select(item("Handle::worker_metrics"))?
            .add_attribute("#[cfg_attr(target_has_atomic = \"64\", expect(dead_code))]")?;
        for name in [
            "num_blocking_threads",
            "num_idle_blocking_threads",
            "worker_local_queue_depth",
            "blocking_queue_depth",
        ] {
            source
                .select(root().implementation("Handle").item(name))?
                .add_attribute("#[expect(dead_code)]")?;
        }
        Ok(())
    })?;
    patch(&path.join("worker/metrics.rs"), |source| {
        for name in ["injection_queue_depth", "worker_local_queue_depth"] {
            source
                .select(root().implementation("Shared").item(name))?
                .add_attribute("#[expect(dead_code)]")?;
        }
        Ok(())
    })?;
    patch(&path.join("mod.rs"), |source| {
        source
            .select(item("MultiThread::block_on"))?
            .add_attribute("#[expect(dead_code)]")?;
        mount(
            source,
            None,
            "telekio",
            "guest/runtime/scheduler/multi_thread/mod.rs",
        )
    })
}

pub(super) fn patch_scheduler(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        for name in ["is_local", "can_spawn_local_on_local_runtime"] {
            source
                .select(root().implementation("Handle").item(name))?
                .add_attribute("#[expect(dead_code)]")?;
            source
                .select(root().implementation("Handle").item(name))?
                .rename(&format!("{name}_inner"))?;
        }
        source
            .select(item("Handle::spawn"))?
            .add_attribute("#[track_caller]")?;
        for name in [
            "num_workers",
            "num_alive_tasks",
            "spawned_tasks_count",
            "injection_queue_depth",
            "num_blocking_threads",
            "num_idle_blocking_threads",
            "worker_local_queue_depth",
            "blocking_queue_depth",
        ] {
            source
                .select(root().implementation("Handle").item(name))?
                .add_attribute("#[expect(dead_code)]")?;
        }
        source
            .select(item("Handle::worker_metrics"))?
            .add_attribute("#[cfg_attr(target_has_atomic = \"64\", expect(dead_code))]")?;
        mount(
            source,
            Some("pub(crate)"),
            "telekio",
            "guest/runtime/scheduler/mod.rs",
        )?;
        source
            .select(item("telekio"))?
            .add_attribute("#[cfg(feature = \"rt\")]")
    })
}

pub(super) fn host(source: &Path) -> Result<(), Box<dyn Error>> {
    if std::env::var_os("CARGO_CFG_TOKIO_UNSTABLE").is_none() {
        return Ok(());
    }
    patch(
        &source.join("src/runtime/scheduler/multi_thread/handle.rs"),
        |contents| {
            contents
                .select(item("Handle"))?
                .add_field("pub(crate) telekio: super::worker::telekio::WorkerObservers")
        },
    )?;
    patch(
        &source.join("src/runtime/scheduler/multi_thread/worker.rs"),
        |contents| {
            mount(
                contents,
                Some("pub(crate)"),
                "telekio",
                "host/runtime/scheduler/multi_thread/worker.rs",
            )?;
            contents
                .select(item("create").record("Handle"))?
                .add_field("telekio: telekio::WorkerObservers::new(size)")?;
            contents
                .select(item("Context::run").call("assert_lifo_enabled_is_correct"))?
                .redirect("telekio_tick")?;
            contents
                .select(item("Context::run").call("park"))?
                .redirect("telekio_park")?;
            contents
                .select(
                    root()
                        .child(item("run"))
                        .call("crate::runtime::context::enter_runtime")
                        .child(root().closure()),
                )?
                .delegate("telekio::enter_worker", &["worker.clone()"])
        },
    )
}
