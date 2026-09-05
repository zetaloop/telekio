use std::{error::Error, fs, path::Path};

use super::{mount, patch};
use crate::edit;

pub(super) fn patch_task_id(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        edit::rename_method(source, "Id", "next", "next_local")?;
        edit::add_attr(
            source,
            edit::AttrTarget::Method {
                owner: "Id",
                name: "next_local",
            },
            "#[expect(dead_code)]",
        )?;
        mount(source, None, "telekio", "guest/runtime/task/id.rs")
    })
}

pub(super) fn patch_task(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        edit::rename_method(source, "SpawnLocation", "capture", "capture_local")?;
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
        edit::redirect_call(
            source,
            edit::Scope::Function("trace_leaf"),
            "Context::try_with_current_trace_leaf_fn",
            "telekio::trace_leaf",
        )?;
        edit::add_attr(
            source,
            edit::AttrTarget::Method {
                owner: "Context",
                name: "is_tracing",
            },
            "#[expect(dead_code)]",
        )?;
        edit::append_fields(
            source,
            "Trace",
            &[edit::Field {
                visibility: None,
                name: "foreign",
                ty: "Option<crate::runtime::dump::telekio::ForeignTrace>",
            }],
        )?;
        edit::append_record_fields(
            source,
            edit::Scope::Method {
                owner: "Trace",
                name: "empty",
            },
            "Self",
            &[edit::FieldInit {
                name: "foreign",
                value: "None",
            }],
        )?;
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
        edit::rename_method(
            source,
            "Trace",
            "resolve_backtraces",
            "resolve_backtraces_local",
        )?;
        edit::add_attr(
            source,
            edit::AttrTarget::Method {
                owner: "Trace",
                name: "resolve_backtraces_local",
            },
            "#[doc(hidden)]",
        )?;
        edit::redirect_call(
            source,
            edit::Scope::Method {
                owner: "Trace",
                name: "fmt",
            },
            "fmt",
            "telekio_fmt",
        )?;
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
            edit::add_attr(
                source,
                edit::AttrTarget::Method {
                    owner: "OwnedTasks<S>",
                    name,
                },
                "#[expect(dead_code)]",
            )?;
        }
        Ok(())
    })
}

pub(super) fn patch_sharded_list(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        edit::add_attr(
            source,
            edit::AttrTarget::Struct("ShardedList"),
            "#[cfg_attr(not(tokio_unstable), expect(dead_code))]",
        )?;
        for target in [
            edit::AttrTarget::Struct("ShardGuard"),
            edit::AttrTarget::Method {
                owner: "ShardedList<L>",
                name: "lock_shard",
            },
            edit::AttrTarget::Method {
                owner: "ShardGuard<'a, L>",
                name: "push",
            },
            edit::AttrTarget::Method {
                owner: "ShardedList<L>",
                name: "added",
            },
        ] {
            edit::add_attr(source, target, "#[expect(dead_code)]")?;
        }
        Ok(())
    })
}

pub(super) fn host(source: &Path) -> Result<(), Box<dyn Error>> {
    let target = source.join("src/runtime/task/id.rs");
    let mut contents = fs::read_to_string(&target)?;
    edit::rename_method(&mut contents, "Id", "next", "next_local")?;
    mount(&mut contents, None, "telekio", "host/runtime/task/id.rs")?;
    fs::write(target, contents)?;

    let target = source.join("src/runtime/task/mod.rs");
    let mut contents = fs::read_to_string(&target)?;
    edit::rename_method(&mut contents, "SpawnLocation", "capture", "capture_local")?;
    fs::write(target, contents)?;

    Ok(())
}
