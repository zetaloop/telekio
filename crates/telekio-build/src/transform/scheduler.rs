use std::{error::Error, fs, path::Path};

use super::{mount, patch};
use crate::edit;

pub(super) fn patch_current_thread(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        edit::append_fields(
            source,
            "Handle",
            &[edit::Field {
                visibility: Some("pub(crate)"),
                name: "telekio",
                ty: "std::sync::OnceLock<Arc<crate::runtime::handle::telekio::Connection>>",
            }],
        )?;
        edit::append_record_fields(
            source,
            edit::Scope::Method {
                owner: "CurrentThread",
                name: "new",
            },
            "Handle",
            &[edit::FieldInit {
                name: "telekio",
                value: "std::sync::OnceLock::new()",
            }],
        )?;
        for (method, bind) in [("spawn", "bind"), ("spawn_local", "bind_local")] {
            edit::redirect_call(
                source,
                edit::Scope::Method {
                    owner: "Handle",
                    name: method,
                },
                bind,
                &format!("{bind}_host"),
            )?;
            edit::redirect_call(
                source,
                edit::Scope::Method {
                    owner: "Handle",
                    name: method,
                },
                "spawn",
                "spawn_host",
            )?;
        }
        for target in [
            edit::AttrTarget::Method {
                owner: "CurrentThread",
                name: "block_on",
            },
            edit::AttrTarget::Struct("Core"),
            edit::AttrTarget::Impl {
                owner: "Core",
                method: "tick",
            },
            edit::AttrTarget::Impl {
                owner: "Context",
                method: "run_task",
            },
            edit::AttrTarget::Impl {
                owner: "Handle",
                method: "next_remote_task",
            },
            edit::AttrTarget::Method {
                owner: "CoreGuard<'_>",
                name: "block_on",
            },
        ] {
            edit::add_attr(source, target, "#[expect(dead_code)]")?;
        }
        edit::add_attr(
            source,
            edit::AttrTarget::Method {
                owner: "Handle",
                name: "spawned_tasks_count",
            },
            "#[expect(dead_code)]",
        )?;
        edit::rename_method(source, "Handle", "owned_id", "owned_id_inner")?;
        for name in [
            "worker_local_queue_depth",
            "num_blocking_threads",
            "num_idle_blocking_threads",
            "blocking_queue_depth",
        ] {
            edit::add_attr(
                source,
                edit::AttrTarget::Method {
                    owner: "Handle",
                    name,
                },
                "#[expect(dead_code)]",
            )?;
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
        edit::rename_method(source, "Handle", "owned_id", "owned_id_inner")?;
        edit::redirect_call(
            source,
            edit::Scope::Method {
                owner: "Handle",
                name: "bind_new_task",
            },
            "bind",
            "bind_host",
        )?;
        edit::redirect_call(
            source,
            edit::Scope::Method {
                owner: "Handle",
                name: "bind_new_task",
            },
            "spawn",
            "spawn_host",
        )?;
        edit::append_fields(
            source,
            "Handle",
            &[edit::Field {
                visibility: Some("pub(crate)"),
                name: "telekio",
                ty: "std::sync::OnceLock<Arc<crate::runtime::handle::telekio::Connection>>",
            }],
        )?;
        Ok(())
    })?;
    patch(&path.join("worker.rs"), |source| {
        edit::append_record_fields(
            source,
            edit::Scope::Function("create"),
            "Handle",
            &[edit::FieldInit {
                name: "telekio",
                value: "std::sync::OnceLock::new()",
            }],
        )?;
        edit::redirect_call(
            source,
            edit::Scope::Method {
                owner: "Launch",
                name: "launch",
            },
            "runtime::spawn_blocking",
            "super::telekio::host_worker",
        )?;
        edit::redirect_call(
            source,
            edit::Scope::Function("block_in_place"),
            "coop::stop",
            "crate::runtime::context::telekio::stop",
        )?;
        edit::redirect_call(
            source,
            edit::Scope::Function("block_in_place"),
            "crate::runtime::context::exit_runtime",
            "telekio::exit_host_runtime",
        )?;
        mount(
            source,
            None,
            "telekio",
            "guest/runtime/scheduler/multi_thread/worker.rs",
        )
    })?;
    patch(&path.join("handle/metrics.rs"), |source| {
        for name in [
            "injection_queue_depth",
            "num_workers",
            "num_alive_tasks",
            "spawned_tasks_count",
        ] {
            edit::add_attr(
                source,
                edit::AttrTarget::Method {
                    owner: "Handle",
                    name,
                },
                "#[expect(dead_code)]",
            )?;
        }
        edit::add_attr(
            source,
            edit::AttrTarget::Method {
                owner: "Handle",
                name: "worker_metrics",
            },
            "#[cfg_attr(target_has_atomic = \"64\", expect(dead_code))]",
        )?;
        for name in [
            "num_blocking_threads",
            "num_idle_blocking_threads",
            "worker_local_queue_depth",
            "blocking_queue_depth",
        ] {
            edit::add_attr(
                source,
                edit::AttrTarget::Method {
                    owner: "Handle",
                    name,
                },
                "#[expect(dead_code)]",
            )?;
        }
        Ok(())
    })?;
    patch(&path.join("worker/metrics.rs"), |source| {
        for name in ["injection_queue_depth", "worker_local_queue_depth"] {
            edit::add_attr(
                source,
                edit::AttrTarget::Method {
                    owner: "Shared",
                    name,
                },
                "#[expect(dead_code)]",
            )?;
        }
        Ok(())
    })?;
    patch(&path.join("mod.rs"), |source| {
        edit::add_attr(
            source,
            edit::AttrTarget::Method {
                owner: "MultiThread",
                name: "block_on",
            },
            "#[expect(dead_code)]",
        )?;
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
            edit::add_attr(
                source,
                edit::AttrTarget::Method {
                    owner: "Handle",
                    name,
                },
                "#[expect(dead_code)]",
            )?;
            edit::rename_method(source, "Handle", name, &format!("{name}_inner"))?;
        }
        edit::add_attr(
            source,
            edit::AttrTarget::Method {
                owner: "Handle",
                name: "spawn",
            },
            "#[track_caller]",
        )?;
        for name in ["num_workers", "num_alive_tasks", "spawned_tasks_count"] {
            edit::add_attr(
                source,
                edit::AttrTarget::Method {
                    owner: "Handle",
                    name,
                },
                "#[expect(dead_code)]",
            )?;
        }
        for name in [
            "injection_queue_depth",
            "num_blocking_threads",
            "num_idle_blocking_threads",
            "worker_local_queue_depth",
            "blocking_queue_depth",
        ] {
            edit::add_attr(
                source,
                edit::AttrTarget::Method {
                    owner: "Handle",
                    name,
                },
                "#[expect(dead_code)]",
            )?;
        }
        edit::add_attr(
            source,
            edit::AttrTarget::Method {
                owner: "Handle",
                name: "worker_metrics",
            },
            "#[cfg_attr(target_has_atomic = \"64\", expect(dead_code))]",
        )?;
        mount(
            source,
            Some("pub(crate)"),
            "telekio",
            "guest/runtime/scheduler/mod.rs",
        )?;
        edit::add_attr(
            source,
            edit::AttrTarget::Module("telekio"),
            "#[cfg(feature = \"rt\")]",
        )
    })
}

pub(super) fn host(source: &Path) -> Result<(), Box<dyn Error>> {
    let target = source.join("src/runtime/scheduler/multi_thread/stats.rs");
    let mut contents = fs::read_to_string(&target)?;
    mount(
        &mut contents,
        Some("pub(super)"),
        "telekio",
        "host/runtime/scheduler/multi_thread/stats.rs",
    )?;
    edit::redirect_call(
        &mut contents,
        edit::Scope::Method {
            owner: "Stats",
            name: "start_processing_scheduled_tasks",
        },
        "Instant::now",
        "telekio::start_poll_batch",
    )?;
    edit::redirect_call(
        &mut contents,
        edit::Scope::Method {
            owner: "Stats",
            name: "end_processing_scheduled_tasks",
        },
        "Instant::now",
        "telekio::finish_poll_batch",
    )?;
    fs::write(target, contents)?;

    if std::env::var_os("CARGO_CFG_TOKIO_UNSTABLE").is_none() {
        return Ok(());
    }

    let target = source.join("src/runtime/scheduler/multi_thread/handle.rs");
    let mut contents = fs::read_to_string(&target)?;
    edit::append_fields(
        &mut contents,
        "Handle",
        &[edit::Field {
            visibility: Some("pub(crate)"),
            name: "telekio",
            ty: "super::worker::telekio::WorkerObservers",
        }],
    )?;
    fs::write(target, contents)?;

    let target = source.join("src/runtime/scheduler/multi_thread/worker.rs");
    let mut contents = fs::read_to_string(&target)?;
    mount(
        &mut contents,
        Some("pub(crate)"),
        "telekio",
        "host/runtime/scheduler/multi_thread/worker.rs",
    )?;
    edit::append_record_fields(
        &mut contents,
        edit::Scope::Function("create"),
        "Handle",
        &[edit::FieldInit {
            name: "telekio",
            value: "telekio::WorkerObservers::new(size)",
        }],
    )?;
    edit::redirect_call(
        &mut contents,
        edit::Scope::Method {
            owner: "Context",
            name: "run",
        },
        "assert_lifo_enabled_is_correct",
        "telekio_tick",
    )?;
    edit::redirect_call(
        &mut contents,
        edit::Scope::Method {
            owner: "Context",
            name: "run",
        },
        "park",
        "telekio_park",
    )?;
    edit::delegate_closure(
        &mut contents,
        edit::Scope::Function("run"),
        edit::Call::Function("crate::runtime::context::enter_runtime"),
        2,
        "telekio::enter_worker",
        &["worker.clone()"],
    )?;
    fs::write(target, contents)?;

    Ok(())
}
