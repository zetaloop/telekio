use std::{
    env,
    error::Error,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::Value as Json;

use crate::invocation;

const TOKIO_VERSION: &str = "1.53.1";

pub fn prepare_tokio() -> Result<PathBuf, Box<dyn Error>> {
    prepare_tokio_artifact(invocation::offline()?).map(|(source, _)| source)
}

pub(crate) fn prepare_tokio_artifact(offline: bool) -> Result<(PathBuf, Json), Box<dyn Error>> {
    prepare_tokio_in(&output_directory()?, offline, true)
}

pub(crate) fn prepare_tokio_in(
    output: &Path,
    offline: bool,
    emit: bool,
) -> Result<(PathBuf, Json), Box<dyn Error>> {
    prepare_package(output, "tokio", TOKIO_VERSION, offline, emit)
}

pub(crate) fn prepare_tokio_workspace(offline: bool) -> Result<PathBuf, Box<dyn Error>> {
    let output = output_directory()?;
    let source = output.join(format!("tokio-workspace-source-{TOKIO_VERSION}"));
    if !source.exists() {
        if offline {
            return Err("Tokio workspace source is unavailable offline".into());
        }
        let status = Command::new("git")
            .args([
                "clone",
                "--depth",
                "1",
                "--branch",
                &format!("tokio-{TOKIO_VERSION}"),
                "https://github.com/tokio-rs/tokio.git",
            ])
            .arg(&source)
            .status()?;
        if !status.success() {
            return Err(format!("git clone failed with {status}").into());
        }
    }
    let destination = output.join(format!("tokio-workspace-{TOKIO_VERSION}"));
    if destination.is_dir() {
        fs::remove_dir_all(&destination)?;
    }
    copy_directory(&source, &destination)?;
    println!(
        "cargo:rerun-if-changed={}",
        source.join("Cargo.toml").display()
    );
    Ok(destination)
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
) -> Result<(PathBuf, Json), Box<dyn Error>> {
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
    let (source, features) = package_source(&manifest, package, version, offline)?;
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
    Ok((destination, features))
}

fn package_source(
    manifest: &Path,
    package: &str,
    version: &str,
    offline: bool,
) -> Result<(PathBuf, Json), Box<dyn Error>> {
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
    let source = Path::new(manifest)
        .parent()
        .ok_or("registry package manifest has no parent")?
        .to_owned();
    Ok((source, selected["features"].clone()))
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
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
