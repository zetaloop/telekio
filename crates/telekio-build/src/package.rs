use std::{
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use toml::{Table, Value};

use crate::{
    prepare_tokio,
    source::{prepare_tokio_in, prepare_tokio_workspace},
    transform::{self, crate_preamble, include_source},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Host,
    Guest,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Guest => "guest",
        }
    }
}

pub fn prepare_tests(support: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let workspace = prepare_tokio_workspace(crate::invocation::offline()?)?;
    let source = workspace.join("tokio");
    let abi = Value::Table(Table::from_iter([
        (
            "package".to_owned(),
            Value::String("telekio-test-support".to_owned()),
        ),
        (
            "path".to_owned(),
            Value::String(support.to_string_lossy().into_owned()),
        ),
    ]));
    prepare_guest_with(source, abi)?;
    Ok(workspace)
}

pub fn prepare_guest() -> Result<PathBuf, Box<dyn Error>> {
    let generated = prepare_guest_with(prepare_tokio()?, package_dependency("telekio-abi", true))?;
    let root = generated.join("src/lib.rs");
    fs::write(&root, include_source(&fs::read_to_string(&root)?)?)?;
    Ok(generated)
}

pub fn prepare_patch(directory: &Path, role: Role, offline: bool) -> Result<(), Box<dyn Error>> {
    prepare_patch_with(directory, role, role, offline)
}

pub fn prepare_mixed_guest_patch(directory: &Path, offline: bool) -> Result<(), Box<dyn Error>> {
    prepare_patch_with(directory, Role::Guest, Role::Host, offline)
}

fn prepare_patch_with(
    directory: &Path,
    source_role: Role,
    dependency_role: Role,
    offline: bool,
) -> Result<(), Box<dyn Error>> {
    if patch_current(directory, source_role, dependency_role)? {
        return Ok(());
    }
    let source_cache = directory
        .parent()
        .ok_or("patch directory has no parent")?
        .join("source");
    let (source, features) = prepare_tokio_in(&source_cache, offline, false)?;
    write_patch(directory, &source, features, source_role, dependency_role)?;
    fs::remove_dir_all(source_cache)?;
    Ok(())
}

fn patch_current(
    directory: &Path,
    source_role: Role,
    dependency_role: Role,
) -> Result<bool, Box<dyn Error>> {
    let manifest = directory.join("Cargo.toml");
    let build = directory.join("build.rs");
    let source = directory.join("src/lib.rs");
    if ![&manifest, &build, &source]
        .into_iter()
        .all(|path| path.is_file())
    {
        return Ok(false);
    }
    let build = fs::read_to_string(build)?;
    let source = fs::read_to_string(source)?;
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
    let host = targets
        .and_then(|targets| targets.get("cfg(all(any(unix, windows), not(loom)))"))
        .and_then(|target| target.get("dependencies"))
        .and_then(Value::as_table)
        .and_then(|dependencies| dependencies.get("telekio-host"));
    let host_features = features.is_some_and(|features| {
        features.iter().all(|(name, values)| {
            name == "default"
                || values.as_array().is_some_and(|values| {
                    values.contains(&Value::String(format!("telekio-host/{name}")))
                        == (dependency_role == Role::Host)
                })
        })
    });
    Ok(version == Some(concat!("=", env!("CARGO_PKG_VERSION")))
        && rust_version == Some(env!("CARGO_PKG_RUST_VERSION"))
        && features.is_some_and(|features| !features.contains_key("telekio-test"))
        && dependencies
            .get("telekio-abi")
            .and_then(|dependency| dependency.get("features"))
            .and_then(Value::as_array)
            .is_some_and(|features| {
                features.contains(&Value::String(source_role.name().to_owned()))
            })
        && !dependencies.contains_key("telekio-host")
        && build.contains("rustc-cfg=telekio_host") == (source_role == Role::Host)
        && source.contains("#[doc(inline)]\npub use telekio_host::*;")
            == (source_role == Role::Host)
        && host.is_some() == (dependency_role == Role::Host)
        && host.is_none_or(|host| {
            host.get("default-features").and_then(Value::as_bool) == Some(false)
        })
        && host_features
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

fn write_patch(
    directory: &Path,
    source: &Path,
    source_features: serde_json::Value,
    source_role: Role,
    dependency_role: Role,
) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(directory.join("src"))?;
    let mut manifest: Value = toml::from_str(&fs::read_to_string(source.join("Cargo.toml"))?)?;
    manifest["features"] = Value::try_from(source_features)?;
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
    let features = manifest
        .get_mut("features")
        .and_then(Value::as_table_mut)
        .ok_or("Tokio manifest has no features")?;
    if dependency_role == Role::Host {
        forward_features(features)?;
    }

    let dependencies = manifest
        .get_mut("dependencies")
        .and_then(Value::as_table_mut)
        .ok_or("Tokio manifest has no dependencies")?;
    dependencies.insert(
        "telekio-abi".to_owned(),
        registry_dependency(Some(source_role)),
    );
    dependencies.remove("telekio-host");
    if dependency_role == Role::Host {
        let mut host = registry_dependency(None);
        host.as_table_mut()
            .ok_or("generated host dependency is not a table")?
            .insert("default-features".to_owned(), Value::Boolean(false));
        manifest
            .as_table_mut()
            .ok_or("Tokio manifest is not a table")?
            .entry("target")
            .or_insert_with(|| Value::Table(Table::new()))
            .as_table_mut()
            .ok_or("Tokio targets are not a table")?
            .entry("cfg(all(any(unix, windows), not(loom)))")
            .or_insert_with(|| Value::Table(Table::new()))
            .as_table_mut()
            .ok_or("Tokio native target is not a table")?
            .entry("dependencies")
            .or_insert_with(|| Value::Table(Table::new()))
            .as_table_mut()
            .ok_or("Tokio native dependencies are not a table")?
            .insert("telekio-host".to_owned(), host);
    }
    manifest
        .as_table_mut()
        .ok_or("Tokio manifest is not a table")?
        .insert(
            "build-dependencies".to_owned(),
            Value::Table(Table::from_iter([(
                "telekio-build".to_owned(),
                registry_dependency(None),
            )])),
        );

    let source_root = fs::read_to_string(source.join("src/lib.rs"))?;
    let host = if source_role == Role::Host {
        "    if (std::env::var_os(\"CARGO_CFG_UNIX\").is_some() || std::env::var_os(\"CARGO_CFG_WINDOWS\").is_some()) && std::env::var_os(\"CARGO_CFG_LOOM\").is_none() {\n        println!(\"cargo::rustc-cfg=telekio_host\");\n        return;\n    }\n"
    } else {
        ""
    };
    fs::write(
        directory.join("Cargo.toml"),
        format!(
            "# Generated by Telekio. Defines Tokio for this target.\n\n{}",
            toml::to_string(&manifest)?
        ),
    )?;
    fs::write(
        directory.join("build.rs"),
        format!(
            "// Generated by Telekio. Prepares Tokio for this target.\n\nfn main() {{\n    println!(\"cargo::rustc-check-cfg=cfg(telekio_host)\");\n{host}    let source = telekio_build::prepare_guest().unwrap();\n    println!(\"cargo::rustc-env=TELEKIO_TOKIO_SOURCE={{}}\", source.join(\"src/lib.rs\").display());\n}}\n"
        ),
    )?;
    let body = if source_role == Role::Host {
        "#[cfg(telekio_host)]\n#[doc(inline)]\npub use telekio_host::*;\n\n#[cfg(not(telekio_host))]\ninclude!(env!(\"TELEKIO_TOKIO_SOURCE\"));\n"
    } else {
        "include!(env!(\"TELEKIO_TOKIO_SOURCE\"));\n"
    };
    fs::write(
        directory.join("src/lib.rs"),
        format!(
            "{}// Generated by Telekio. Exposes Tokio for this target.\n\n{body}",
            crate_preamble(&source_root)?
        ),
    )?;
    Ok(())
}

fn forward_features(features: &mut Table) -> Result<(), Box<dyn Error>> {
    for (name, values) in features {
        if name == "default" {
            continue;
        }
        values
            .as_array_mut()
            .ok_or("Tokio feature is not an array")?
            .push(Value::String(format!("telekio-host/{name}")));
    }
    Ok(())
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

fn registry_dependency(role: Option<Role>) -> Value {
    let mut dependency = Table::from_iter([(
        "version".to_owned(),
        Value::String(format!("={}", env!("CARGO_PKG_VERSION"))),
    )]);
    if let Some(role) = role {
        dependency.insert(
            "features".to_owned(),
            Value::Array(vec![Value::String(role.name().to_owned())]),
        );
    }
    Value::Table(dependency)
}

fn prepare_guest_with(generated: PathBuf, abi: Value) -> Result<PathBuf, Box<dyn Error>> {
    let native =
        env::var_os("CARGO_CFG_UNIX").is_some() || env::var_os("CARGO_CFG_WINDOWS").is_some();
    if native && env::var_os("CARGO_CFG_LOOM").is_none() {
        patch_manifest(&generated.join("Cargo.toml"), abi)?;
        transform::guest(&generated)?;
    }
    Ok(generated)
}

pub fn prepare_tokio_host() -> Result<PathBuf, Box<dyn Error>> {
    let directory = prepare_tokio()?;
    let path = directory.join("src/lib.rs");
    let source = fs::read_to_string(&path)?;
    fs::write(path, include_source(&source)?)?;
    transform::host(&directory)?;
    Ok(directory)
}

fn patch_manifest(path: &Path, abi: Value) -> Result<(), Box<dyn Error>> {
    let manifest = fs::read_to_string(path)?;
    let dependency = toml::to_string(&abi)?;
    fs::write(
        path,
        format!("{manifest}\n[dependencies.telekio-abi]\n{dependency}"),
    )?;
    Ok(())
}
