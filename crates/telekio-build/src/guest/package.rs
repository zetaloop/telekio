use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use toml::{Table, Value};

use crate::{
    prepare_tokio,
    source::{crate_preamble, include_source, prepare_tokio_in},
};

use super::prepare_guest_with;

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

fn patch_current(directory: &Path, role: Role) -> Result<bool, Box<dyn Error>> {
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

fn write_patch(directory: &Path, source: &Path, role: Role) -> Result<(), Box<dyn Error>> {
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
