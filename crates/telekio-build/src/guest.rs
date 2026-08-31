use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use toml::{Table, Value};

use crate::{
    edit, prepare_tokio,
    source::{crate_preamble, include_source, prepare_tokio_in},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Host,
    Guest,
}

pub fn prepare_tests() -> Result<PathBuf, Box<dyn Error>> {
    let generated = prepare_guest_with(prepare_tokio()?, package_dependency("telekio", true))?;
    let path = generated.join("Cargo.toml");
    let mut manifest: Value = toml::from_str(&fs::read_to_string(&path)?)?;
    let features = manifest
        .get_mut("features")
        .and_then(Value::as_table_mut)
        .ok_or("Tokio manifest has no features")?;
    features.insert(
        "telekio-test".to_owned(),
        Value::Array(vec![Value::String("dep:telekio-host".to_owned())]),
    );
    let mut host_dependency = package_dependency("telekio-host", false);
    host_dependency
        .as_table_mut()
        .ok_or("generated host dependency is not a table")?
        .insert("optional".to_owned(), Value::Boolean(true));
    manifest
        .get_mut("dependencies")
        .and_then(Value::as_table_mut)
        .ok_or("Tokio manifest has no dependencies")?
        .insert("telekio-host".to_owned(), host_dependency);
    let root = manifest
        .as_table_mut()
        .ok_or("Tokio manifest is not a table")?;
    root.insert("workspace".to_owned(), Value::Table(Table::new()));
    root.entry("patch")
        .or_insert_with(|| Value::Table(Table::new()))
        .as_table_mut()
        .ok_or("Tokio patches are not a table")?
        .entry("crates-io")
        .or_insert_with(|| Value::Table(Table::new()))
        .as_table_mut()
        .ok_or("Tokio crates.io patches are not a table")?
        .insert(
            "tokio".to_owned(),
            Value::Table(Table::from_iter([(
                "path".to_owned(),
                Value::String(".".to_owned()),
            )])),
        );
    fs::write(path, toml::to_string(&manifest)?)?;
    Ok(generated)
}

pub fn prepare_guest() -> Result<PathBuf, Box<dyn Error>> {
    let generated = prepare_guest_with(prepare_tokio()?, package_dependency("telekio", true))?;
    let root = generated.join("src/lib.rs");
    fs::write(&root, include_source(&fs::read_to_string(&root)?)?)?;
    Ok(generated)
}

pub fn prepare_patch(directory: &Path, role: Role, offline: bool) -> Result<(), Box<dyn Error>> {
    if patch_current(directory, role)? {
        return Ok(());
    }
    let source_cache = directory
        .parent()
        .ok_or("patch directory has no parent")?
        .join("source");
    let source = prepare_tokio_in(&source_cache, offline, false)?;
    write_patch(directory, &source, role)?;
    fs::remove_dir_all(source_cache)?;
    Ok(())
}

pub(crate) fn patch_current(directory: &Path, role: Role) -> Result<bool, Box<dyn Error>> {
    let manifest = directory.join("Cargo.toml");
    let build = directory.join("build.rs");
    let source = directory.join("src/lib.rs");
    if ![&manifest, &build, &source]
        .into_iter()
        .all(|path| path.is_file())
    {
        return Ok(false);
    }
    let manifest: Value = toml::from_str(&fs::read_to_string(manifest)?)?;
    let dependencies = manifest
        .get("dependencies")
        .and_then(Value::as_table)
        .ok_or("generated Tokio manifest has no dependencies")?;
    let version = manifest
        .get("build-dependencies")
        .and_then(|dependencies| dependencies.get("telekio-build"))
        .and_then(|dependency| dependency.get("version"))
        .and_then(Value::as_str);
    let rust_version = manifest
        .get("package")
        .and_then(|package| package.get("rust-version"))
        .and_then(Value::as_str);
    let features = manifest.get("features").and_then(Value::as_table);
    let package = manifest.get("package").and_then(Value::as_table);
    let targets = manifest.get("target").and_then(Value::as_table);
    Ok(version == Some(concat!("=", env!("CARGO_PKG_VERSION")))
        && rust_version == Some(env!("CARGO_PKG_RUST_VERSION"))
        && features.is_some_and(|features| features.contains_key("telekio-test"))
        && dependencies.contains_key("telekio-host") == (role == Role::Host)
        && dependencies
            .values()
            .chain(
                manifest
                    .get("build-dependencies")
                    .and_then(Value::as_table)
                    .into_iter()
                    .flat_map(|dependencies| dependencies.values()),
            )
            .all(|dependency| dependency.get("path").is_none())
        && ["bench", "bin", "dev-dependencies", "example", "test"]
            .into_iter()
            .all(|key| manifest.get(key).is_none())
        && package.is_some_and(|package| !package.contains_key("metadata"))
        && targets.is_none_or(|targets| {
            targets.values().all(|target| {
                target
                    .as_table()
                    .is_none_or(|target| !target.contains_key("dev-dependencies"))
            })
        }))
}

pub(crate) fn write_patch(
    directory: &Path,
    source: &Path,
    role: Role,
) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(directory.join("src"))?;
    let mut manifest: Value = toml::from_str(&fs::read_to_string(source.join("Cargo.toml"))?)?;
    let package = manifest
        .get_mut("package")
        .and_then(Value::as_table_mut)
        .ok_or("Tokio manifest has no package table")?;
    package.insert("build".to_owned(), Value::String("build.rs".to_owned()));
    package.insert(
        "rust-version".to_owned(),
        Value::String(env!("CARGO_PKG_RUST_VERSION").to_owned()),
    );
    for field in [
        "authors",
        "categories",
        "homepage",
        "keywords",
        "metadata",
        "readme",
        "repository",
    ] {
        package.remove(field);
    }
    let root = manifest
        .as_table_mut()
        .ok_or("Tokio manifest is not a table")?;
    root.remove("dev-dependencies");
    for target in ["bench", "bin", "example", "test"] {
        root.remove(target);
    }
    if let Some(targets) = root.get_mut("target").and_then(Value::as_table_mut) {
        targets.retain(|_, target| {
            if let Some(target) = target.as_table_mut() {
                target.remove("dev-dependencies");
                !target.is_empty()
            } else {
                true
            }
        });
    }
    manifest
        .get_mut("features")
        .and_then(Value::as_table_mut)
        .ok_or("Tokio manifest has no features")?
        .insert("telekio-test".to_owned(), Value::Array(Vec::new()));

    let dependencies = manifest
        .get_mut("dependencies")
        .and_then(Value::as_table_mut)
        .ok_or("Tokio manifest has no dependencies")?;
    dependencies.insert("telekio".to_owned(), registry_dependency(true));
    if role == Role::Host {
        dependencies.insert("telekio-host".to_owned(), registry_dependency(false));
    } else {
        dependencies.remove("telekio-host");
    }
    manifest
        .as_table_mut()
        .ok_or("Tokio manifest is not a table")?
        .insert(
            "build-dependencies".to_owned(),
            Value::Table(Table::from_iter([(
                "telekio-build".to_owned(),
                registry_dependency(false),
            )])),
        );

    let source_root = fs::read_to_string(source.join("src/lib.rs"))?;
    let host = if role == Role::Host {
        "    println!(\"cargo::rustc-cfg=telekio_host\");\n"
    } else {
        ""
    };
    fs::write(
        directory.join("Cargo.toml"),
        format!(
            "# Generated by Telekio. Defines the transformed Tokio guest.\n\n{}",
            toml::to_string(&manifest)?
        ),
    )?;
    fs::write(
        directory.join("build.rs"),
        format!(
            "// Generated by Telekio. Builds the transformed Tokio guest.\n\nfn main() {{\n    let source = telekio_build::prepare_guest().unwrap();\n    println!(\"cargo::rustc-env=TELEKIO_TOKIO_SOURCE={{}}\", source.join(\"src/lib.rs\").display());\n    println!(\"cargo::rustc-check-cfg=cfg(telekio_host)\");\n{host}}}\n"
        ),
    )?;
    fs::write(
        directory.join("src/lib.rs"),
        format!(
            "{}// Generated by Telekio. Includes the transformed Tokio guest.\n\ninclude!(env!(\"TELEKIO_TOKIO_SOURCE\"));\n",
            crate_preamble(&source_root)?
        ),
    )?;
    Ok(())
}

fn prepare_guest_with(generated: PathBuf, telekio: Value) -> Result<PathBuf, Box<dyn Error>> {
    patch_manifest(&generated.join("Cargo.toml"), telekio)?;
    patch_task(&generated.join("src/runtime/task/mod.rs"))?;
    patch_current_thread(&generated.join("src/runtime/scheduler/current_thread/mod.rs"))?;
    patch_inject(&generated.join("src/runtime/scheduler/inject.rs"))?;
    patch_multi_thread(&generated.join("src/runtime/scheduler/multi_thread"))?;
    patch_scheduler(&generated.join("src/runtime/scheduler/mod.rs"))?;
    patch_builder(&generated.join("src/runtime/builder.rs"))?;
    patch_runtime(&generated.join("src/runtime/runtime.rs"))?;
    patch_local_runtime(&generated.join("src/runtime/local_runtime/runtime.rs"))?;
    patch_blocking(&generated.join("src/runtime/blocking/pool.rs"))?;
    patch_metrics(&generated.join("src/runtime/metrics/batch.rs"))?;
    patch_runtime_metrics(&generated.join("src/runtime/metrics/runtime.rs"))?;
    patch_context(&generated.join("src/runtime/context/blocking.rs"))?;
    patch_defer(&generated)?;
    patch_time(&generated)?;
    patch_io(&generated)?;
    patch_signal(&generated)?;
    Ok(generated)
}

fn patch_manifest(path: &Path, telekio: Value) -> Result<(), Box<dyn Error>> {
    let mut manifest: Value = toml::from_str(&fs::read_to_string(path)?)?;
    let dependencies = manifest
        .get_mut("dependencies")
        .and_then(Value::as_table_mut)
        .ok_or("Tokio manifest has no dependencies")?;
    dependencies.insert("telekio".to_owned(), telekio);
    manifest
        .get_mut("features")
        .and_then(Value::as_table_mut)
        .ok_or("Tokio manifest has no features")?
        .insert("telekio-test".to_owned(), Value::Array(Vec::new()));
    let check_cfg = manifest
        .get_mut("lints")
        .and_then(|lints| lints.get_mut("rust"))
        .and_then(|rust| rust.get_mut("unexpected_cfgs"))
        .and_then(|lint| lint.get_mut("check-cfg"))
        .and_then(Value::as_array_mut)
        .ok_or("Tokio manifest has no unexpected_cfgs check-cfg list")?;
    let telekio_host = Value::String("cfg(telekio_host)".to_owned());
    if !check_cfg.contains(&telekio_host) {
        check_cfg.push(telekio_host);
    }
    fs::write(path, toml::to_string(&manifest)?)?;
    Ok(())
}

fn dependency(path: &Path, guest: bool) -> Value {
    let mut dependency = Table::from_iter([
        (
            "path".to_owned(),
            Value::String(path.to_string_lossy().into_owned()),
        ),
        (
            "version".to_owned(),
            Value::String(format!("={}", env!("CARGO_PKG_VERSION"))),
        ),
    ]);
    if guest {
        dependency.insert(
            "features".to_owned(),
            Value::Array(vec![Value::String("guest".to_owned())]),
        );
    }
    Value::Table(dependency)
}

fn registry_dependency(guest: bool) -> Value {
    let mut dependency = Table::from_iter([(
        "version".to_owned(),
        Value::String(format!("={}", env!("CARGO_PKG_VERSION"))),
    )]);
    if guest {
        dependency.insert(
            "features".to_owned(),
            Value::Array(vec![Value::String("guest".to_owned())]),
        );
    }
    Value::Table(dependency)
}

fn package_dependency(name: &str, guest: bool) -> Value {
    let sibling = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|parent| {
            [
                parent.join(name),
                parent.join(format!("{name}-{}", env!("CARGO_PKG_VERSION"))),
            ]
            .into_iter()
            .find(|path| path.join("Cargo.toml").is_file())
        });
    if let Some(sibling) = sibling {
        dependency(&sibling, guest)
    } else {
        let mut dependency = Table::from_iter([(
            "version".to_owned(),
            Value::String(format!("={}", env!("CARGO_PKG_VERSION"))),
        )]);
        if guest {
            dependency.insert(
                "features".to_owned(),
                Value::Array(vec![Value::String("guest".to_owned())]),
            );
        }
        Value::Table(dependency)
    }
}

fn patch_task(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        mount_with(source, Some("pub(crate)"), "telekio", "task.rs")
    })
}

fn patch_current_thread(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        edit::append_fields(
            source,
            "Handle",
            &[edit::Field {
                visibility: Some("pub(crate)"),
                name: "telekio",
                ty: "task::telekio::Registry<Arc<Handle>>",
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
                value: "task::telekio::Registry::new()",
            }],
        )?;
        edit::rename_method(source, "CurrentThread", "block_on", "drive")?;
        for target in [
            edit::AttrTarget::Method {
                owner: "CurrentThread",
                name: "drive",
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
        edit::redirect_call(
            source,
            edit::Scope::Method {
                owner: "Arc<Handle>",
                name: "release",
            },
            "remove",
            "remove_host_task",
        )?;
        edit::redirect_call(
            source,
            edit::Scope::MethodArgument {
                owner: "Arc<Handle>",
                name: "schedule",
                call: edit::Call::Function("context::with_scheduler"),
            },
            "push_task",
            "push_host_task",
        )?;
        edit::redirect_call(
            source,
            edit::Scope::MethodArgument {
                owner: "Arc<Handle>",
                name: "schedule",
                call: edit::Call::Function("context::with_scheduler"),
            },
            "push",
            "push_host_task",
        )?;
        edit::redirect_call(
            source,
            edit::Scope::Method {
                owner: "Handle",
                name: "spawn_local",
            },
            "schedule",
            "schedule_local",
        )?;
        mount(source, "telekio", "current_thread.rs")
    })
}

fn patch_inject(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        edit::add_attr(
            source,
            edit::AttrTarget::Method {
                owner: "Inject<T>",
                name: "push",
            },
            "#[expect(dead_code)]",
        )?;
        mount(source, "telekio", "inject.rs")
    })
}

fn patch_multi_thread(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(&path.join("handle.rs"), |source| {
        edit::append_fields(
            source,
            "Handle",
            &[edit::Field {
                visibility: Some("pub(crate)"),
                name: "telekio",
                ty: "task::telekio::Registry<Arc<Handle>>",
            }],
        )?;
        edit::redirect_call(
            source,
            edit::Scope::Method {
                owner: "Handle",
                name: "bind_new_task",
            },
            "schedule_option_task_without_yield",
            "schedule_host_option",
        )?;
        for function in ["release", "schedule", "yield_now"] {
            edit::redirect_call(
                source,
                edit::Scope::Method {
                    owner: "Arc<Handle>",
                    name: function,
                },
                if function == "release" {
                    "remove"
                } else {
                    "schedule_task"
                },
                if function == "release" {
                    "remove_host_task"
                } else {
                    "schedule_host_task"
                },
            )?;
        }
        Ok(())
    })?;
    patch(&path.join("worker.rs"), |source| {
        edit::append_record_fields(
            source,
            edit::Scope::Function("create"),
            "Handle",
            &[edit::FieldInit {
                name: "telekio",
                value: "task::telekio::Registry::new()",
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
        edit::add_attr(
            source,
            edit::AttrTarget::Impl {
                owner: "Handle",
                method: "schedule_task",
            },
            "#[expect(dead_code)]",
        )?;
        mount(source, "telekio", "worker.rs")
    })?;
    patch(&path.join("handle/metrics.rs"), |source| {
        for name in ["injection_queue_depth", "worker_metrics"] {
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
        edit::add_attr(
            source,
            edit::AttrTarget::Method {
                owner: "Shared",
                name: "injection_queue_depth",
            },
            "#[expect(dead_code)]",
        )
    })?;
    patch(&path.join("stats.rs"), |source| {
        edit::add_attr(
            source,
            edit::AttrTarget::Method {
                owner: "Stats",
                name: "inc_local_schedule_count",
            },
            "#[expect(dead_code)]",
        )
    })?;
    patch(&path.join("mod.rs"), |source| {
        edit::rename_method(source, "MultiThread", "block_on", "drive")?;
        edit::add_attr(
            source,
            edit::AttrTarget::Method {
                owner: "MultiThread",
                name: "drive",
            },
            "#[expect(dead_code)]",
        )?;
        mount(source, "telekio", "multi_thread.rs")
    })
}

fn patch_scheduler(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        for name in ["injection_queue_depth", "worker_metrics"] {
            edit::add_attr(
                source,
                edit::AttrTarget::Method {
                    owner: "Handle",
                    name,
                },
                "#[expect(dead_code)]",
            )?;
        }
        mount(source, "telekio", "scheduler.rs")?;
        edit::add_attr(
            source,
            edit::AttrTarget::Module("telekio"),
            "#[cfg(feature = \"rt\")]",
        )
    })
}

fn patch_builder(path: &Path) -> Result<(), Box<dyn Error>> {
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
        mount(source, "telekio", "builder.rs")
    })
}

fn patch_runtime(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| mount(source, "telekio", "runtime.rs"))
}

fn patch_local_runtime(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| mount(source, "telekio", "local_runtime.rs"))
}

fn patch_blocking(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        edit::append_fields(
            source,
            "BlockingPool",
            &[edit::Field {
                visibility: None,
                name: "telekio",
                ty: "std::sync::OnceLock<std::sync::Arc<crate::runtime::task::telekio::Host>>",
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
        for target in [
            edit::AttrTarget::Enum("Mandatory"),
            edit::AttrTarget::Impl {
                owner: "Spawner",
                method: "spawn_blocking",
            },
        ] {
            edit::add_attr(source, target, "#[expect(dead_code)]")?;
        }
        mount(source, "telekio", "blocking.rs")
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

fn patch_metrics(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        edit::add_attr(
            source,
            edit::AttrTarget::Method {
                owner: "MetricsBatch",
                name: "inc_local_schedule_count",
            },
            "#[expect(dead_code)]",
        )
    })
}

fn patch_time(generated: &Path) -> Result<(), Box<dyn Error>> {
    patch(&generated.join("src/time/sleep.rs"), |source| {
        edit::retarget_use(source, "Timer", "self::telekio::Timer")?;
        mount(source, "telekio", "time.rs")
    })?;
    patch(&generated.join("src/time/clock.rs"), |source| {
        edit::delegate_closure(
            source,
            edit::Scope::Function("pause"),
            edit::Call::Function("with_clock"),
            0,
            "telekio::pause",
            &[],
        )
        .map_err(|error| format!("pause clock integration: {error}"))?;
        edit::delegate_closure(
            source,
            edit::Scope::Function("resume"),
            edit::Call::Function("with_clock"),
            0,
            "telekio::resume",
            &[],
        )
        .map_err(|error| format!("resume clock integration: {error}"))?;
        edit::delegate_closure(
            source,
            edit::Scope::Function("advance"),
            edit::Call::Function("with_clock"),
            0,
            "telekio::advance",
            &["duration"],
        )
        .map_err(|error| format!("advance clock integration: {error}"))?;
        edit::delegate_closure(
            source,
            edit::Scope::Function("now"),
            edit::Call::Function("with_clock"),
            0,
            "telekio::now",
            &[],
        )?;
        mount(source, "telekio", "clock.rs")?;
        edit::add_attr(
            source,
            edit::AttrTarget::Module("telekio"),
            "#[cfg(feature = \"test-util\")]",
        )
    })?;
    patch(&generated.join("src/runtime/mod.rs"), |source| {
        edit::add_attr(
            source,
            edit::AttrTarget::Enum("Timer"),
            "#[expect(dead_code)]",
        )?;
        edit::add_attr(
            source,
            edit::AttrTarget::Impl {
                owner: "Timer",
                method: "new",
            },
            "#[expect(dead_code)]",
        )
    })
}

fn patch_io(generated: &Path) -> Result<(), Box<dyn Error>> {
    patch(&generated.join("src/runtime/io/mod.rs"), |source| {
        mount_with(source, Some("pub(crate)"), "telekio", "io.rs")
    })?;
    patch(
        &generated.join("src/runtime/io/registration.rs"),
        |source| {
            for (name, replacement) in [
                ("new_with_interest_and_handle", "register_local"),
                ("deregister", "deregister_local"),
                ("clear_readiness", "clear_local_readiness"),
                ("poll_read_ready", "poll_local_read_ready"),
                ("poll_write_ready", "poll_local_write_ready"),
                ("poll_ready", "poll_local_ready"),
                ("readiness", "local_readiness"),
                ("try_io", "local_try_io"),
            ] {
                edit::rename_method(source, "Registration", name, replacement)?;
                edit::add_attr(
                    source,
                    edit::AttrTarget::Method {
                        owner: "Registration",
                        name: replacement,
                    },
                    "#[expect(dead_code)]",
                )?;
            }
            mount(source, "telekio", "registration.rs")
        },
    )?;
    patch(
        &generated.join("src/runtime/io/scheduled_io.rs"),
        |source| {
            edit::append_fields(
                source,
                "ScheduledIo",
                &[edit::Field {
                    visibility: None,
                    name: "telekio",
                    ty: "std::sync::Mutex<telekio::State>",
                }],
            )?;
            edit::append_record_fields(
                source,
                edit::Scope::Method {
                    owner: "ScheduledIo",
                    name: "default",
                },
                "ScheduledIo",
                &[edit::FieldInit {
                    name: "telekio",
                    value: "std::sync::Mutex::new(telekio::State::default())",
                }],
            )?;
            mount(source, "telekio", "scheduled_io.rs")
        },
    )?;
    patch(&generated.join("src/io/poll_evented.rs"), |source| {
        edit::set_type_parameter(
            source,
            "PollEvented<E>",
            "new_with_interest_and_handle",
            "E",
            "E: Source + crate::runtime::io::telekio::Source",
        )
    })?;
    patch(&generated.join("src/io/async_fd.rs"), |source| {
        edit::retarget_use(source, "SourceFd", "self::telekio::SourceFd")?;
        mount(source, "telekio", "async_fd.rs")
    })?;
    patch(&generated.join("src/net/windows/named_pipe.rs"), |source| {
        edit::redirect_call(
            source,
            edit::Scope::Method {
                owner: "NamedPipeServer",
                name: "connect",
            },
            "connect",
            "connect_host",
        )?;
        edit::redirect_call(
            source,
            edit::Scope::MethodArgument {
                owner: "NamedPipeServer",
                name: "connect",
                call: edit::Call::Method("async_io"),
            },
            "connect",
            "connect_host",
        )?;
        edit::redirect_call(
            source,
            edit::Scope::Method {
                owner: "NamedPipeServer",
                name: "disconnect",
            },
            "disconnect",
            "disconnect_host",
        )?;
        for owner in ["NamedPipeServer", "NamedPipeClient"] {
            for (method, replacement) in [
                ("poll_read", "poll_read_host"),
                ("poll_write", "poll_write_host"),
                ("poll_write_vectored", "poll_write_vectored_host"),
            ] {
                edit::redirect_call(
                    source,
                    edit::Scope::Method {
                        owner,
                        name: method,
                    },
                    method,
                    replacement,
                )?;
            }
        }
        for owner in ["NamedPipeServer", "NamedPipeClient"] {
            for (method, kind, data) in [
                ("try_read", "Read", "buf.as_mut_ptr()"),
                ("try_write", "Write", "buf.as_ptr().cast_mut()"),
            ] {
                edit::delegate_closure(
                    source,
                    edit::Scope::Method {
                        owner,
                        name: method,
                    },
                    edit::Call::Method("try_io"),
                    1,
                    "crate::runtime::io::telekio::delegate",
                    &[
                        "self.io.registration()",
                        &format!("::telekio::IoOperationKind::{kind}"),
                        data,
                        "buf.len()",
                    ],
                )?;
            }
            for (method, helper, data, len) in [
                (
                    "try_read_vectored",
                    "crate::runtime::io::telekio::delegate_read_vectored",
                    "bufs.as_mut_ptr().cast()",
                    "bufs.len()",
                ),
                (
                    "try_write_vectored",
                    "crate::runtime::io::telekio::delegate_write_vectored",
                    "buf.as_ptr().cast()",
                    "buf.len()",
                ),
            ] {
                edit::delegate_closure(
                    source,
                    edit::Scope::Method {
                        owner,
                        name: method,
                    },
                    edit::Call::Method("try_io"),
                    1,
                    helper,
                    &["self.io.registration()", data, len],
                )?;
            }
            edit::delegate_closure(
                source,
                edit::Scope::Method {
                    owner,
                    name: "try_read_buf",
                },
                edit::Call::Method("try_io"),
                1,
                "crate::runtime::io::telekio::delegate_read_buf",
                &["self.io.registration()", "std::ptr::from_mut(buf)"],
            )?;
        }
        mount(source, "telekio", "named_pipe.rs")
    })
}

fn patch_signal(generated: &Path) -> Result<(), Box<dyn Error>> {
    patch(&generated.join("src/signal/mod.rs"), |source| {
        for target in [
            edit::AttrTarget::Struct("RxFuture"),
            edit::AttrTarget::Function("make_future"),
            edit::AttrTarget::Impl {
                owner: "RxFuture",
                method: "new",
            },
            edit::AttrTarget::Module("reusable_box"),
        ] {
            edit::add_attr(source, target, "#[cfg_attr(not(test), expect(dead_code))]")?;
        }
        Ok(())
    })?;
    patch(&generated.join("src/signal/unix.rs"), |source| {
        edit::retarget_use(source, "RxFuture", "self::telekio::RxFuture")?;
        edit::redirect_call(
            source,
            edit::Scope::Function("signal"),
            "signal_with_handle",
            "telekio::signal",
        )?;
        mount_with(source, Some("pub(crate)"), "telekio", "signal.rs")
    })?;
    patch(&generated.join("src/process/unix/mod.rs"), |source| {
        edit::redirect_call(
            source,
            edit::Scope::Method {
                owner: "GlobalOrphanQueue",
                name: "push_orphan",
            },
            "push_orphan",
            "reap_host_orphan",
        )?;
        mount(source, "telekio", "process.rs")
    })?;
    patch(&generated.join("src/signal/windows.rs"), |source| {
        edit::add_attr(
            source,
            edit::AttrTarget::Modules("imp"),
            "#[cfg_attr(not(test), expect(dead_code))]",
        )?;
        edit::retarget_use(source, "RxFuture", "self::telekio::RxFuture")?;
        for name in [
            "ctrl_c",
            "ctrl_break",
            "ctrl_close",
            "ctrl_logoff",
            "ctrl_shutdown",
        ] {
            edit::redirect_call(
                source,
                edit::Scope::Function(name),
                &format!("self::imp::{name}"),
                &format!("telekio::{name}"),
            )?;
        }
        mount(source, "telekio", "signal.rs")
    })
}

fn patch_defer(generated: &Path) -> Result<(), Box<dyn Error>> {
    patch(&generated.join("src/runtime/context.rs"), |source| {
        mount_with(source, Some("pub(crate)"), "telekio", "defer.rs")?;
        edit::add_attr(
            source,
            edit::AttrTarget::Module("telekio"),
            "#[cfg(feature = \"rt\")]",
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

fn patch_runtime_metrics(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| {
        edit::redirect_call(
            source,
            edit::Scope::Method {
                owner: "RuntimeMetrics",
                name: "global_queue_depth",
            },
            "injection_queue_depth",
            "host_global_queue_depth",
        )?;
        for name in [
            "worker_total_busy_duration",
            "worker_park_count",
            "worker_park_unpark_count",
        ] {
            edit::redirect_call(
                source,
                edit::Scope::Method {
                    owner: "RuntimeMetrics",
                    name,
                },
                "worker_metrics",
                "host_worker_metrics",
            )?;
        }
        Ok(())
    })
}

fn patch_context(path: &Path) -> Result<(), Box<dyn Error>> {
    patch(path, |source| mount(source, "telekio", "context.rs"))?;
    patch(
        &path
            .parent()
            .and_then(Path::parent)
            .ok_or("context path has no runtime parent")?
            .join("handle.rs"),
        |source| {
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
            mount(source, "telekio", "handle.rs")
        },
    )
}

fn mount(source: &mut String, name: &str, helper: &str) -> Result<(), Box<dyn Error>> {
    mount_with(source, None, name, helper)
}

fn mount_with(
    source: &mut String,
    visibility: Option<&str>,
    name: &str,
    helper: &str,
) -> Result<(), Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("guest")
        .join(helper);
    println!("cargo::rerun-if-changed={}", path.display());
    edit::mount_module(source, visibility, name, &path)
}

fn patch(
    path: &Path,
    transform: impl FnOnce(&mut String) -> Result<(), Box<dyn Error>>,
) -> Result<(), Box<dyn Error>> {
    let mut source = fs::read_to_string(path)?;
    transform(&mut source).map_err(|error| format!("{}: {error}", path.display()))?;
    fs::write(path, source)?;
    Ok(())
}
