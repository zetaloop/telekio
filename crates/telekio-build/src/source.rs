use std::{
    env,
    error::Error,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use toml::Value;

const PACKAGE: &str = "tokio";

pub fn prepare_tokio() -> Result<PathBuf, Box<dyn Error>> {
    let manifest = workspace_manifest()?;
    let lock = manifest.with_file_name("Cargo.lock");
    let version = supported_version()?;
    let checksum = locked_checksum(&lock, &version)?;
    let archive = registry_archive(&manifest, &version, &checksum)?;
    let destination = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is unavailable")?)
        .join(format!("{PACKAGE}-{version}"));

    unpack(&archive, &destination, &version)?;
    verify_manifest(&destination.join("Cargo.toml"), &version)?;

    println!("cargo::rerun-if-changed={}", archive.display());
    println!("cargo::rerun-if-changed={}", lock.display());
    println!("cargo::rerun-if-env-changed=CARGO_HOME");

    Ok(destination)
}

fn workspace_manifest() -> Result<PathBuf, Box<dyn Error>> {
    let cargo = env::var_os("CARGO").ok_or("CARGO is unavailable")?;
    let package = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").ok_or("CARGO_MANIFEST_DIR is unavailable")?,
    )
    .join("Cargo.toml");
    let output = Command::new(cargo)
        .arg("locate-project")
        .arg("--workspace")
        .arg("--message-format=plain")
        .arg("--manifest-path")
        .arg(package)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cargo locate-project failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let path = String::from_utf8(output.stdout)?;
    let path = path.trim();
    if path.is_empty() {
        return Err("cargo locate-project returned an empty path".into());
    }
    Ok(PathBuf::from(path))
}

fn supported_version() -> Result<String, Box<dyn Error>> {
    let manifest: Value = toml::from_str(include_str!("../Cargo.toml"))?;
    let dependency = manifest
        .get("target")
        .and_then(|value| value.get("cfg(any())"))
        .and_then(|value| value.get("dependencies"))
        .and_then(|value| value.get(PACKAGE))
        .ok_or("telekio-build has no cfg(any()) Tokio dependency")?;
    let requirement = match dependency {
        Value::String(requirement) => requirement.as_str(),
        Value::Table(dependency) => dependency
            .get("version")
            .and_then(Value::as_str)
            .ok_or("Telekio's Tokio dependency has no version")?,
        _ => return Err("Telekio's Tokio dependency has an invalid form".into()),
    };
    requirement
        .strip_prefix('=')
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            format!("Telekio's Tokio dependency must be exact, got {requirement}").into()
        })
}

fn locked_checksum(lock: &Path, supported: &str) -> Result<String, Box<dyn Error>> {
    let lock: Value = toml::from_str(&fs::read_to_string(lock)?)?;
    let packages = lock
        .get("package")
        .and_then(Value::as_array)
        .ok_or("Cargo.lock has no packages")?;
    let tokio = packages
        .iter()
        .filter(|package| package.get("name").and_then(Value::as_str) == Some(PACKAGE))
        .collect::<Vec<_>>();
    let versions = tokio
        .iter()
        .filter_map(|package| package.get("version").and_then(Value::as_str))
        .collect::<Vec<_>>();
    if versions != [supported] {
        return Err(format!(
            "Cargo.lock must select only Tokio {supported}, got {}",
            if versions.is_empty() {
                "none".to_owned()
            } else {
                versions.join(", ")
            }
        )
        .into());
    }
    let package = tokio[0];
    let source = package
        .get("source")
        .and_then(Value::as_str)
        .ok_or("locked Tokio has no source")?;
    if !source.starts_with("registry+") {
        return Err(format!("locked Tokio source is not a registry: {source}").into());
    }
    package
        .get("checksum")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "locked Tokio has no checksum".into())
}

fn registry_archive(
    manifest: &Path,
    version: &str,
    checksum: &str,
) -> Result<PathBuf, Box<dyn Error>> {
    let cargo_home = cargo_home()?;
    if let Some(archive) = find_archive(&cargo_home, version, checksum)? {
        return Ok(archive);
    }

    let cargo = env::var_os("CARGO").ok_or("CARGO is unavailable")?;
    let status = Command::new(cargo)
        .arg("fetch")
        .arg("--locked")
        .arg("--manifest-path")
        .arg(manifest)
        .status()?;
    if !status.success() {
        return Err(format!("cargo fetch --locked failed with {status}").into());
    }

    find_archive(&cargo_home, version, checksum)?.ok_or_else(|| {
        format!(
            "could not find {PACKAGE}-{version}.crate under {}",
            cargo_home.join("registry/cache").display()
        )
        .into()
    })
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
    version: &str,
    checksum: &str,
) -> Result<Option<PathBuf>, Box<dyn Error>> {
    let cache = cargo_home.join("registry/cache");
    let registries = match fs::read_dir(cache) {
        Ok(registries) => registries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let name = format!("{PACKAGE}-{version}.crate");
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
        "Tokio archive checksum mismatch: expected {checksum}, got {}",
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

fn unpack(archive: &Path, destination: &Path, version: &str) -> Result<(), Box<dyn Error>> {
    let decoder = GzDecoder::new(fs::File::open(archive)?);
    let mut archive = tar::Archive::new(decoder);
    let package_dir = format!("{PACKAGE}-{version}");

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

fn verify_manifest(path: &Path, version: &str) -> Result<(), Box<dyn Error>> {
    let manifest: Value = toml::from_str(&fs::read_to_string(path)?)?;
    let package = manifest
        .get("package")
        .ok_or("Tokio has no package table")?;
    let name = package.get("name").and_then(Value::as_str);
    let actual_version = package.get("version").and_then(Value::as_str);
    if name != Some(PACKAGE) || actual_version != Some(version) {
        return Err(format!(
            "archive manifest describes {} {}, expected {PACKAGE} {version}",
            name.unwrap_or("<unknown>"),
            actual_version.unwrap_or("<unknown>")
        )
        .into());
    }
    Ok(())
}
