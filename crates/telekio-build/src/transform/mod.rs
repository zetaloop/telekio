mod io;
mod metrics;
mod runtime;
mod scheduler;
mod signal;
mod task;
mod time;

use std::{error::Error, fs, path::Path};

use ra_ap_syntax::{AstNode, Edition, SourceFile, ast::HasModuleItem};

use r#override::{Source, item, root};

pub(crate) fn guest(generated: &Path) -> Result<(), Box<dyn Error>> {
    runtime::patch_panicking(generated)?;
    patch_guest_root(&generated.join("src/lib.rs"))?;
    task::patch_task_id(&generated.join("src/runtime/task/id.rs"))?;
    task::patch_task(&generated.join("src/runtime/task/mod.rs"))?;
    patch(&generated.join("src/runtime/task_hooks.rs"), |source| {
        mount(
            source,
            Some("pub(crate)"),
            "telekio",
            "guest/runtime/task_hooks.rs",
        )
    })?;
    task::patch_task_trace(&generated.join("src/runtime/task/trace/mod.rs"))?;
    task::patch_task_trace_tree(&generated.join("src/runtime/task/trace/tree.rs"))?;
    task::patch_dump(&generated.join("src/runtime/dump.rs"))?;
    task::patch_task_list(&generated.join("src/runtime/task/list.rs"))?;
    task::patch_sharded_list(&generated.join("src/util/sharded_list.rs"))?;
    scheduler::patch_current_thread(
        &generated.join("src/runtime/scheduler/current_thread/mod.rs"),
    )?;
    scheduler::patch_multi_thread(&generated.join("src/runtime/scheduler/multi_thread"))?;
    scheduler::patch_scheduler(&generated.join("src/runtime/scheduler/mod.rs"))?;
    runtime::patch_builder(&generated.join("src/runtime/builder.rs"))?;
    runtime::patch_runtime(&generated.join("src/runtime/runtime.rs"))?;
    runtime::patch_local_runtime(&generated.join("src/runtime/local_runtime/runtime.rs"))?;
    runtime::patch_blocking(&generated.join("src/runtime/blocking/pool.rs"))?;
    patch(&generated.join("src/runtime/metrics/mod.rs"), |source| {
        mount(
            source,
            Some("pub(crate)"),
            "telekio",
            "guest/runtime/metrics/mod.rs",
        )
    })?;
    metrics::patch_histogram(&generated.join("src/runtime/metrics/histogram.rs"))?;
    metrics::patch_worker_metrics(&generated.join("src/runtime/metrics/worker.rs"))?;
    metrics::patch_runtime_metrics(&generated.join("src/runtime/metrics/runtime.rs"))?;
    runtime::patch_context(&generated.join("src/runtime/context/blocking.rs"))?;
    patch_coop(&generated.join("src/task/coop/mod.rs"))?;
    patch_rand(generated)?;
    runtime::patch_defer(generated)?;
    time::patch_time(generated)?;
    io::patch_io(generated)?;
    signal::patch_signal(generated)?;
    Ok(())
}

pub(crate) fn host(source: &Path) -> Result<(), Box<dyn Error>> {
    runtime::patch_panicking(source)?;
    for (module, visibility, attribute) in [
        ("runtime/builder.rs", None, None),
        ("runtime/context.rs", Some("pub(crate)"), None),
        ("task/coop/mod.rs", None, None),
        ("util/rand.rs", None, None),
        (
            "util/trace.rs",
            Some("pub(crate)"),
            Some("#[cfg(feature = \"rt\")]"),
        ),
        ("runtime/handle.rs", Some("pub(crate)"), None),
        ("runtime/id.rs", None, None),
        (
            "runtime/mod.rs",
            Some("pub"),
            Some("#[cfg(feature = \"rt\")]"),
        ),
        ("runtime/process.rs", None, Some("#[cfg(unix)]")),
        (
            "process/unix/mod.rs",
            Some("pub(crate)"),
            Some("#[cfg(unix)]"),
        ),
        (
            "runtime/metrics/histogram.rs",
            None,
            Some("#[cfg(tokio_unstable)]"),
        ),
        ("runtime/io/registration.rs", None, None),
        (
            "runtime/io/mod.rs",
            Some("pub(crate)"),
            Some(
                "#[cfg(all(feature = \"rt\", any(target_os = \"freebsd\", target_os = \"linux\")))]",
            ),
        ),
        (
            "runtime/io/driver.rs",
            Some("pub(crate)"),
            Some("#[cfg(target_os = \"linux\")]"),
        ),
        ("io/poll_evented.rs", None, Some("#[cfg(windows)]")),
        ("io/async_fd.rs", None, None),
        (
            "net/mod.rs",
            Some("pub"),
            Some("#[cfg(all(windows, feature = \"net\"))]"),
        ),
        ("net/windows/named_pipe.rs", None, None),
        ("runtime/task/trace/mod.rs", Some("pub(crate)"), None),
        ("runtime/dump.rs", None, None),
    ] {
        patch(&source.join("src").join(module), |contents| {
            mount(contents, visibility, "telekio", &format!("host/{module}"))?;
            if module == "io/async_fd.rs" {
                contents
                    .select(root().import("Interest"))?
                    .redirect("crate::io::interest::Interest")?;
                contents
                    .select(root().import("Ready"))?
                    .redirect("crate::io::ready::Ready")?;
            }
            if visibility.is_some() {
                contents
                    .select(item("telekio"))?
                    .add_attribute("#[doc(hidden)]")?;
            }
            if let Some(attribute) = attribute {
                contents.select(item("telekio"))?.add_attribute(attribute)?;
            }
            Ok(())
        })?;
    }
    for (file, owner, methods) in [
        (
            "runtime/handle.rs",
            "Handle",
            &["spawn_named", "spawn_local_named", "block_on_inner"][..],
        ),
        ("runtime/runtime.rs", "Runtime", &["block_on_inner"][..]),
        (
            "runtime/local_runtime/runtime.rs",
            "LocalRuntime",
            &["block_on_inner"][..],
        ),
    ] {
        patch(&source.join("src").join(file), |contents| {
            for name in methods {
                contents
                    .select(
                        root()
                            .implementation(owner)
                            .item(name)
                            .call("crate::util::trace::task"),
                    )?
                    .redirect("crate::util::trace::telekio::task")?;
            }
            Ok(())
        })?;
    }
    patch(&source.join("src/runtime/blocking/pool.rs"), |contents| {
        contents
            .select(root().import("blocking_task"))?
            .redirect("crate::util::trace::telekio::blocking_task")
    })?;
    if std::env::var_os("CARGO_CFG_UNIX").is_some()
        && std::env::var_os("CARGO_FEATURE_RT").is_some()
        && std::env::var_os("CARGO_FEATURE_NET").is_none()
    {
        patch(&source.join("src/runtime/io/driver.rs"), |contents| {
            contents
                .select(
                    root()
                        .macro_call("cfg_net_unix")
                        .has(item("ReadyEvent::with_ready")),
                )?
                .redirect("cfg_unix")
        })?;
        patch(&source.join("src/runtime/io/mod.rs"), |contents| {
            contents
                .select(root())?
                .mount_module("pub(crate) mod async_fd", source.join("src/io/async_fd.rs"))
        })?;
    }
    task::host(source)?;
    runtime::host(source)?;
    scheduler::host(source)?;
    signal::host(source)
}

fn patch_guest_root(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        mount(source, None, "telekio_context", "guest/lib.rs")?;
        source
            .select(item("telekio_context"))?
            .add_attribute("#[cfg(all(not(feature = \"rt\"), not(test)))]")
    })
}

fn patch_coop(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        source
            .select(
                item("inc_budget_forced_yield_count")
                    .call("context::with_current")
                    .child(root().closure()),
            )?
            .delegate("telekio::forced_yield", &[])?;
        mount(source, None, "telekio", "guest/task/coop/mod.rs")
    })
}

fn patch_rand(generated: &Path) -> Result<(), Box<dyn Error>> {
    patch(&generated.join("src/util/rand.rs"), |source| {
        mount(source, None, "telekio", "guest/util/rand.rs")
    })?;
    patch(&generated.join("src/util/rand/rt.rs"), |source| {
        mount(source, None, "telekio", "guest/util/rand/rt.rs")
    })
}

fn mount(
    source: &mut Source,
    visibility: Option<&str>,
    name: &str,
    helper: &str,
) -> Result<(), Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tokio")
        .join(helper);
    let visibility = visibility.map_or(String::new(), |visibility| format!("{visibility} "));
    source
        .select(root())?
        .mount_module(&format!("{visibility}mod {name}"), &path)
}

fn patch(
    path: &Path,
    transform: impl FnOnce(&mut Source) -> Result<(), Box<dyn Error>>,
) -> Result<(), Box<dyn Error>> {
    let mut source = Source::parse(
        &fs::read_to_string(path)?.replace("\r\n", "\n"),
        Edition::Edition2021,
    )?;
    transform(&mut source).map_err(|error| format!("{}: {error}", path.display()))?;
    fs::write(path, source.to_string())?;
    Ok(())
}

pub(crate) fn crate_preamble(source: &str) -> Result<&str, Box<dyn Error>> {
    let (preamble, _) = source_parts(source)?;
    Ok(preamble)
}

pub(crate) fn include_source(source: &str) -> Result<String, Box<dyn Error>> {
    let (_, body) = source_parts(source)?;
    Ok(body.to_owned())
}

fn source_parts(source: &str) -> Result<(&str, &str), Box<dyn Error>> {
    let parsed = SourceFile::parse(source, Edition::CURRENT);
    if !parsed.errors().is_empty() {
        return Err(format!("crate source has syntax errors: {:?}", parsed.errors()).into());
    }
    let start = parsed
        .tree()
        .items()
        .next()
        .map(|item| usize::from(item.syntax().text_range().start()))
        .unwrap_or(source.len());
    Ok(source.split_at(start))
}
