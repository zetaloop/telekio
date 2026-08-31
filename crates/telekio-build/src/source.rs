use std::{
    env,
    error::Error,
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

use flate2::read::GzDecoder;
use ra_ap_syntax::{AstNode, Edition, SourceFile, ast::HasModuleItem};
use sha2::{Digest, Sha256};
use toml::Value;

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

pub fn prepare_tokio_host(version: &str) -> Result<PathBuf, Box<dyn Error>> {
    if version != TOKIO_VERSION {
        return Err(format!("telekio-tokio {version} requires Tokio {TOKIO_VERSION}").into());
    }
    let directory = prepare_package(
        &output_directory()?,
        "tokio",
        version,
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
            .current_dir(&out)
            .arg("generate-lockfile")
            .arg("--manifest-path")
            .arg(&manifest)
            .args(offline.then_some("--offline"))
            .status()?;
        if !status.success() {
            return Err(format!("cargo generate-lockfile failed with {status}").into());
        }
    }
    let (locked, checksum) = locked_package(&lock, package)?;
    if locked != version {
        return Err(format!("Cargo.lock selected {package} {locked}, expected {version}").into());
    }
    prepare_archive(
        output,
        &manifest,
        &lock,
        (package, version, &checksum),
        offline,
        emit,
    )
}

fn prepare_archive(
    output: &Path,
    manifest: &Path,
    lock: &Path,
    package: (&str, &str, &str),
    offline: bool,
    emit: bool,
) -> Result<PathBuf, Box<dyn Error>> {
    let (name, version, checksum) = package;
    let archive = registry_archive(manifest, name, version, checksum, offline)?;
    let destination = output.join(format!("{name}-{version}"));
    if destination.is_dir() {
        fs::remove_dir_all(&destination)?;
    }
    unpack(&archive, &destination, name, version)?;
    verify_manifest(&destination.join("Cargo.toml"), name, version)?;
    if emit {
        println!("cargo:rerun-if-changed={}", archive.display());
        println!("cargo:rerun-if-changed={}", lock.display());
        println!("cargo:rerun-if-env-changed=CARGO_HOME");
    }
    Ok(destination)
}

fn locked_package(lock: &Path, name: &str) -> Result<(String, String), Box<dyn Error>> {
    let lock: Value = toml::from_str(&fs::read_to_string(lock)?)?;
    let packages = lock
        .get("package")
        .and_then(Value::as_array)
        .ok_or("Cargo.lock has no packages")?;
    let selected = packages
        .iter()
        .filter(|package| {
            package.get("name").and_then(Value::as_str) == Some(name)
                && package
                    .get("source")
                    .and_then(Value::as_str)
                    .is_some_and(|source| source.starts_with("registry+"))
        })
        .collect::<Vec<_>>();
    let [package] = selected.as_slice() else {
        return Err(format!(
            "Cargo.lock must select one registry {name} package, got {}",
            selected.len()
        )
        .into());
    };
    let version = package
        .get("version")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("locked {name} has no version"))?
        .to_owned();
    let checksum = package
        .get("checksum")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("locked {name} has no checksum"))?;
    Ok((version, checksum))
}

fn registry_archive(
    manifest: &Path,
    package: &str,
    version: &str,
    checksum: &str,
    offline: bool,
) -> Result<PathBuf, Box<dyn Error>> {
    let cargo_home = cargo_home()?;
    if let Some(archive) = find_archive(&cargo_home, package, version, checksum)? {
        return Ok(archive);
    }

    let status = Command::new(cargo())
        .current_dir(manifest.parent().ok_or("manifest has no parent")?)
        .arg("fetch")
        .arg("--locked")
        .arg("--manifest-path")
        .arg(manifest)
        .args(offline.then_some("--offline"))
        .status()?;
    if !status.success() {
        return Err(format!("cargo fetch --locked failed with {status}").into());
    }

    find_archive(&cargo_home, package, version, checksum)?.ok_or_else(|| {
        format!(
            "could not find {package}-{version}.crate under {}",
            cargo_home.join("registry/cache").display()
        )
        .into()
    })
}

fn cargo() -> OsString {
    env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
}

fn cargo_home() -> Result<PathBuf, Box<dyn Error>> {
    env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
        .or_else(|| env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".cargo")))
        .ok_or_else(|| "CARGO_HOME is unavailable".into())
}

fn find_archive(
    cargo_home: &Path,
    package: &str,
    version: &str,
    checksum: &str,
) -> Result<Option<PathBuf>, Box<dyn Error>> {
    let cache = cargo_home.join("registry/cache");
    let registries = match fs::read_dir(cache) {
        Ok(registries) => registries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let name = format!("{package}-{version}.crate");
    let mut candidates = Vec::new();
    for registry in registries {
        let candidate = registry?.path().join(&name);
        if candidate.is_file() {
            candidates.push(candidate);
        }
    }
    candidates.sort();
    if candidates.is_empty() {
        return Ok(None);
    }
    let mut mismatches = Vec::new();
    for candidate in candidates {
        let actual = digest(&candidate)?;
        if actual == checksum {
            return Ok(Some(candidate));
        }
        mismatches.push(format!("{}: {actual}", candidate.display()));
    }
    Err(format!(
        "{package} archive checksum mismatch: expected {checksum}, got {}",
        mismatches.join(", ")
    )
    .into())
}

fn digest(path: &Path) -> Result<String, Box<dyn Error>> {
    Ok(Sha256::digest(fs::read(path)?)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn unpack(
    archive: &Path,
    destination: &Path,
    package: &str,
    version: &str,
) -> Result<(), Box<dyn Error>> {
    let decoder = GzDecoder::new(fs::File::open(archive)?);
    let mut archive = tar::Archive::new(decoder);
    let package_dir = format!("{package}-{version}");

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let relative = checked_path(&path, &package_dir)?;
        let destination = destination.join(relative);
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            fs::create_dir_all(&destination)?;
        } else if kind.is_file() {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            entry.unpack(destination)?;
        } else {
            return Err(format!("unsupported archive entry {}", path.display()).into());
        }
    }
    Ok(())
}

fn checked_path(path: &Path, package_dir: &str) -> Result<PathBuf, Box<dyn Error>> {
    let mut components = path.components();
    match components.next() {
        Some(Component::Normal(component)) if component == package_dir => {}
        _ => return Err(format!("unexpected archive path {}", path.display()).into()),
    }
    let mut relative = PathBuf::new();
    for component in components {
        match component {
            Component::Normal(component) => relative.push(component),
            _ => return Err(format!("unsupported archive path {}", path.display()).into()),
        }
    }
    Ok(relative)
}

fn verify_manifest(path: &Path, package_name: &str, version: &str) -> Result<(), Box<dyn Error>> {
    let manifest: Value = toml::from_str(&fs::read_to_string(path)?)?;
    let package = manifest
        .get("package")
        .ok_or_else(|| format!("{package_name} has no package table"))?;
    let name = package.get("name").and_then(Value::as_str);
    let actual_version = package.get("version").and_then(Value::as_str);
    if name != Some(package_name) || actual_version != Some(version) {
        return Err(format!(
            "archive manifest describes {} {}, expected {package_name} {version}",
            name.unwrap_or("<unknown>"),
            actual_version.unwrap_or("<unknown>")
        )
        .into());
    }
    Ok(())
}
