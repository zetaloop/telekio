use std::{
    env,
    error::Error,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use ra_ap_syntax::{AstNode, Edition, SourceFile, ast::HasModuleItem};
use serde_json::Value as Json;

use crate::{edit, invocation};

const TOKIO_VERSION: &str = "1.53.1";

pub fn prepare_tokio() -> Result<PathBuf, Box<dyn Error>> {
    prepare_tokio_artifact(invocation::offline()?)
}

pub(crate) fn prepare_tokio_artifact(offline: bool) -> Result<PathBuf, Box<dyn Error>> {
    prepare_tokio_in(&output_directory()?, offline, true)
}

pub(crate) fn prepare_tokio_in(
    output: &Path,
    offline: bool,
    emit: bool,
) -> Result<PathBuf, Box<dyn Error>> {
    prepare_package(output, "tokio", TOKIO_VERSION, offline, emit)
}

pub fn prepare_tokio_host() -> Result<PathBuf, Box<dyn Error>> {
    let directory = prepare_package(
        &output_directory()?,
        "tokio",
        TOKIO_VERSION,
        invocation::offline()?,
        true,
    )?;
    let path = directory.join("src/lib.rs");
    let source = fs::read_to_string(&path)?;
    fs::write(path, include_source(&source)?)?;
    mount_host_modules(&directory)?;
    Ok(directory)
}

fn mount_host_modules(source: &Path) -> Result<(), Box<dyn Error>> {
    let helpers = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/host");
    for (target, helper, visibility, attribute) in [
        ("src/runtime/builder.rs", "builder.rs", None, None),
        ("src/runtime/handle.rs", "handle.rs", None, None),
        ("src/runtime/id.rs", "id.rs", None, None),
        (
            "src/runtime/metrics/histogram.rs",
            "histogram.rs",
            None,
            Some("#[cfg(tokio_unstable)]"),
        ),
        (
            "src/runtime/io/registration.rs",
            "registration.rs",
            None,
            None,
        ),
        (
            "src/io/poll_evented.rs",
            "poll_evented.rs",
            None,
            Some("#[cfg(windows)]"),
        ),
        ("src/io/async_fd.rs", "async_fd.rs", None, None),
        (
            "src/net/mod.rs",
            "socket.rs",
            Some("pub"),
            Some("#[cfg(all(windows, feature = \"net\"))]"),
        ),
        ("src/net/windows/named_pipe.rs", "named_pipe.rs", None, None),
    ] {
        let helper = helpers.join(helper);
        println!("cargo::rerun-if-changed={}", helper.display());
        let target = source.join(target);
        let mut contents = fs::read_to_string(&target)?;
        edit::mount_module(&mut contents, visibility, "telekio", &helper)?;
        if visibility.is_some() {
            edit::add_attr(
                &mut contents,
                edit::AttrTarget::Module("telekio"),
                "#[doc(hidden)]",
            )?;
        }
        if let Some(attribute) = attribute {
            edit::add_attr(
                &mut contents,
                edit::AttrTarget::Module("telekio"),
                attribute,
            )?;
        }
        fs::write(target, contents)?;
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

    let helper = helpers.join("worker.rs");
    println!("cargo::rerun-if-changed={}", helper.display());
    let target = source.join("src/runtime/scheduler/multi_thread/worker.rs");
    let mut contents = fs::read_to_string(&target)?;
    edit::mount_module(&mut contents, Some("pub(crate)"), "telekio", &helper)?;
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

fn output_directory() -> Result<PathBuf, Box<dyn Error>> {
    env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| "OUT_DIR is unavailable".into())
}

fn prepare_package(
    output: &Path,
    package: &str,
    version: &str,
    offline: bool,
    emit: bool,
) -> Result<PathBuf, Box<dyn Error>> {
    let out = output.join(format!("{package}-source-{version}"));
    fs::create_dir_all(out.join("src"))?;
    let manifest = out.join("Cargo.toml");
    if !manifest.is_file() {
        fs::write(
            &manifest,
            format!(
                "[package]\nname = \"telekio-{package}-source\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies]\n{package} = \"={version}\"\n"
            ),
        )?;
        fs::write(out.join("src/lib.rs"), "")?;
    } else {
        let source = fs::read_to_string(&manifest)?;
        if !source.contains("[workspace]") {
            fs::write(&manifest, format!("{source}\n[workspace]\n"))?;
        }
    }
    let lock = out.join("Cargo.lock");
    if !lock.is_file() {
        let status = Command::new(cargo())
            .current_dir(env::temp_dir())
            .arg("generate-lockfile")
            .arg("--manifest-path")
            .arg(&manifest)
            .args(offline.then_some("--offline"))
            .status()?;
        if !status.success() {
            return Err(format!("cargo generate-lockfile failed with {status}").into());
        }
    }
    let source = package_source(&manifest, package, version, offline)?;
    let destination = output.join(format!("{package}-{version}"));
    if destination.is_dir() {
        fs::remove_dir_all(&destination)?;
    }
    copy_directory(&source, &destination)?;
    if emit {
        println!(
            "cargo:rerun-if-changed={}",
            source.join("Cargo.toml").display()
        );
        println!("cargo:rerun-if-changed={}", lock.display());
        println!("cargo:rerun-if-env-changed=CARGO_HOME");
    }
    Ok(destination)
}

fn package_source(
    manifest: &Path,
    package: &str,
    version: &str,
    offline: bool,
) -> Result<PathBuf, Box<dyn Error>> {
    let output = Command::new(cargo())
        .current_dir(env::temp_dir())
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--manifest-path",
        ])
        .arg(manifest)
        .args(offline.then_some("--offline"))
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let metadata: Json = serde_json::from_slice(&output.stdout)?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or("Cargo metadata has no packages")?;
    let selected = packages
        .iter()
        .filter(|candidate| {
            candidate["name"].as_str() == Some(package)
                && candidate["version"].as_str() == Some(version)
                && candidate["source"]
                    .as_str()
                    .is_some_and(|source| source.starts_with("registry+"))
        })
        .collect::<Vec<_>>();
    let [selected] = selected.as_slice() else {
        return Err(format!(
            "Cargo metadata must select one registry {package} {version}, got {}",
            selected.len()
        )
        .into());
    };
    let manifest = selected["manifest_path"]
        .as_str()
        .ok_or("registry package has no manifest path")?;
    Path::new(manifest)
        .parent()
        .map(Path::to_owned)
        .ok_or_else(|| "registry package manifest has no parent".into())
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err(format!(
                "unsupported package source entry {}",
                entry.path().display()
            )
            .into());
        }
    }
    Ok(())
}

fn cargo() -> OsString {
    env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
}
