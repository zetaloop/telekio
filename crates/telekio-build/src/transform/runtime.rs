use std::{error::Error, fs, path::Path};

use super::{mount, patch};
use crate::edit;

pub(super) fn patch_builder(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        edit::redirect_call(
            source,
            edit::Scope::Method {
                owner: "Builder",
                name: "build",
            },
            "build_current_thread_runtime",
            "build_hosted_current_thread",
        )?;
        edit::redirect_call(
            source,
            edit::Scope::Method {
                owner: "Builder",
                name: "build",
            },
            "build_threaded_runtime",
            "build_hosted_multi_thread",
        )?;
        edit::redirect_call(
            source,
            edit::Scope::Method {
                owner: "Builder",
                name: "build_local",
            },
            "build_current_thread_local_runtime",
            "build_hosted_local",
        )?;
        mount(source, None, "telekio", "guest/runtime/builder.rs")
    })
}

pub(super) fn patch_runtime(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        for variant in ["Scheduler::CurrentThread", "Scheduler::MultiThread"] {
            edit::delegate_call(
                source,
                edit::Scope::MethodArm {
                    owner: "Runtime",
                    name: "block_on_inner",
                    variant,
                },
                edit::Call::Method("block_on"),
                "telekio::block_on",
                &["&self.blocking_pool"],
            )?;
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
        edit::delegate_call(
            source,
            edit::Scope::Method {
                owner: "LocalRuntime",
                name: "block_on_inner",
            },
            edit::Call::Method("block_on"),
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
        edit::append_fields(
            source,
            "BlockingPool",
            &[edit::Field {
                visibility: None,
                name: "telekio",
                ty: "std::sync::OnceLock<::telekio_abi::Runtime>",
            }],
        )?;
        edit::append_record_fields(
            source,
            edit::Scope::Method {
                owner: "BlockingPool",
                name: "new",
            },
            "BlockingPool",
            &[edit::FieldInit {
                name: "telekio",
                value: "std::sync::OnceLock::new()",
            }],
        )?;
        edit::rename_method(source, "BlockingPool", "shutdown", "shutdown_workers")?;
        edit::add_attr(
            source,
            edit::AttrTarget::Enum("Mandatory"),
            "#[cfg_attr(not(tokio_unstable), expect(dead_code))]",
        )?;
        edit::add_attr(
            source,
            edit::AttrTarget::Impl {
                owner: "Spawner",
                method: "spawn_blocking",
            },
            "#[expect(dead_code)]",
        )?;
        edit::add_attr(
            source,
            edit::AttrTarget::Method {
                owner: "SpawnerMetrics",
                name: "queue_depth",
            },
            "#[expect(dead_code)]",
        )?;
        edit::add_attr(
            source,
            edit::AttrTarget::Impl {
                owner: "Spawner",
                method: "num_threads",
            },
            "#[expect(dead_code)]",
        )?;
        mount(source, None, "telekio", "guest/runtime/blocking/pool.rs")
    })?;
    patch(
        &path
            .parent()
            .and_then(Path::parent)
            .ok_or("blocking path has no runtime parent")?
            .join("handle.rs"),
        |source| {
            edit::redirect_call(
                source,
                edit::Scope::Method {
                    owner: "Handle",
                    name: "spawn_blocking",
                },
                "spawn_blocking",
                "spawn_host_blocking",
            )
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
            edit::rename_method(source, "Handle", "runtime_flavor", "runtime_flavor_inner")?;
            edit::set_method_visibility(source, "Handle", "runtime_flavor_inner", "pub(crate)")?;
            edit::delegate_async_body(
                source,
                edit::Scope::Method {
                    owner: "Handle",
                    name: "dump",
                },
                "crate::runtime::dump::telekio::dump",
                &["self"],
            )?;
            edit::redirect_call(
                source,
                edit::Scope::Method {
                    owner: "Handle",
                    name: "is_tracing",
                },
                "super::task::trace::Context::is_tracing",
                "telekio::is_tracing",
            )?;
            edit::redirect_call(
                source,
                edit::Scope::MethodArgument {
                    owner: "Handle",
                    name: "block_on_inner",
                    call: edit::Call::Function("context::enter_runtime"),
                },
                "block_on",
                "block_on_host",
            )?;
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
        for (function, call, helper) in [
            ("thread_rng_n", "with", "telekio::rng"),
            ("budget", "try_with", "telekio::budget"),
            ("set_current_task_id", "try_with", "telekio::task_id"),
            ("current_task_id", "try_with", "telekio::task_id"),
        ] {
            edit::delegate_closure(
                source,
                edit::Scope::Function(function),
                edit::Call::Method(call),
                0,
                helper,
                &[],
            )?;
        }
        edit::delegate_closure(
            source,
            edit::Scope::Function("worker_index"),
            edit::Call::Function("with_scheduler"),
            0,
            "telekio::worker_index",
            &[],
        )
    })?;
    patch(&generated.join("src/task/yield_now.rs"), |source| {
        edit::redirect_call(
            source,
            edit::Scope::FunctionArgument {
                name: "yield_now",
                call: edit::Call::Function("poll_fn"),
            },
            "context::defer",
            "crate::runtime::context::telekio::defer",
        )?;
        edit::remove_use(source, "context")
    })?;
    patch(&generated.join("src/task/coop/mod.rs"), |source| {
        edit::redirect_call(
            source,
            edit::Scope::Function("register_waker"),
            "context::defer",
            "crate::runtime::context::telekio::defer",
        )
    })
}

pub(super) fn host(source: &Path) -> Result<(), Box<dyn Error>> {
    let target = source.join("src/runtime/context.rs");
    let mut contents = fs::read_to_string(&target)?;
    for (function, call, helper) in [
        ("thread_rng_n", "with", "telekio::rng"),
        ("budget", "try_with", "telekio::budget"),
        ("set_current_task_id", "try_with", "telekio::task_id"),
        ("current_task_id", "try_with", "telekio::task_id"),
    ] {
        edit::delegate_closure(
            &mut contents,
            edit::Scope::Function(function),
            edit::Call::Method(call),
            0,
            helper,
            &[],
        )?;
    }
    fs::write(target, contents)?;

    Ok(())
}
