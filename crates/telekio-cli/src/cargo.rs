use std::{
    error::Error,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{self, Command},
};

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Kind {
    Packages,
    Workspace,
    Single,
    Output,
    Plain,
}

pub(super) fn status(
    cargo: &OsStr,
    arguments: &[OsString],
    config: Option<&Path>,
) -> Result<process::ExitStatus, Box<dyn Error>> {
    let mut arguments = arguments.to_vec();
    let mut command = Command::new(cargo);
    if arguments
        .first()
        .and_then(|argument| argument.to_str())
        .is_some_and(|argument| argument.starts_with('+'))
    {
        command.arg(arguments.remove(0));
    }
    if let Some(config) = config {
        let split = arguments
            .iter()
            .position(|argument| argument == "--")
            .unwrap_or(arguments.len());
        // Cargo extensions receive configuration after the subcommand.
        let mut cargo = Vec::new();
        let mut configs = Vec::new();
        let mut index = 0;
        while index < split {
            let argument = arguments[index].to_string_lossy();
            if argument == "--config" {
                if let Some(value) = arguments.get(index + 1) {
                    configs.extend([arguments[index].clone(), value.clone()]);
                }
                index += 2;
            } else if argument.starts_with("--config=") {
                configs.push(arguments[index].clone());
                index += 1;
            } else {
                cargo.push(arguments[index].clone());
                index += 1;
            }
        }
        cargo.extend(configs);
        cargo.extend([OsString::from("--config"), config.as_os_str().to_owned()]);
        cargo.extend_from_slice(&arguments[split..]);
        arguments = cargo;
    }
    Ok(command.args(arguments).status()?)
}

pub(super) fn expand_alias(
    cargo: &OsStr,
    arguments: &[OsString],
) -> Result<Vec<OsString>, Box<dyn Error>> {
    let mut expanded = arguments.to_vec();
    if operation(&expanded).is_none_or(builtin) {
        return Ok(expanded);
    }
    let output = Command::new(cargo)
        .args(global_options(arguments))
        .args(["--color", "never", "--list"])
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr)
            .trim()
            .to_owned()
            .into());
    }
    let aliases = String::from_utf8(output.stdout)?;
    let mut visited = Vec::new();
    while let Some(index) = command_index(&expanded) {
        let Some(name) = expanded[index].to_str() else {
            break;
        };
        if builtin(name) {
            break;
        }
        let Some(target) = aliases.lines().find_map(|line| {
            let (alias, target) = line.split_once(" alias: ")?;
            (alias.trim() == name).then_some(target)
        }) else {
            break;
        };
        if visited.iter().any(|alias| alias == name) {
            return Err("Cargo alias expansion is recursive".into());
        }
        visited.push(name.to_owned());
        if target.starts_with('!') {
            return Err("Cargo shell aliases cannot determine a Telekio artifact role; use the direct Cargo command".into());
        }
        expanded.splice(index..=index, target.split_whitespace().map(OsString::from));
    }
    Ok(expanded)
}

pub(super) fn metadata(
    cargo: &OsStr,
    arguments: &[OsString],
    manifest: &Path,
    config: Option<&Path>,
    no_deps: bool,
) -> Result<serde_json::Value, Box<dyn Error>> {
    let mut command = Command::new(cargo);
    command.args(global_options(arguments));
    if let Some(config) = config {
        command.arg("--config").arg(config);
    }
    command.args(["metadata", "--format-version", "1"]);
    if no_deps {
        command.arg("--no-deps");
    } else {
        command.args(metadata_options(arguments));
    }
    let output = command.arg("--manifest-path").arg(manifest).output()?;
    if !output.status.success() {
        let name = if config.is_some() { "patched " } else { "" };
        return Err(format!(
            "{name}Cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

pub(super) fn manifest(
    cargo: &OsStr,
    arguments: &[OsString],
) -> Result<Option<PathBuf>, Box<dyn Error>> {
    if operation(arguments) == Some("install") && !has_option(arguments, "--path") {
        return Ok(None);
    }
    let mut command = Command::new(cargo);
    command
        .args(global_options(arguments))
        .args(["locate-project", "--message-format", "plain"]);
    if let Some(path) = option(arguments, &["--manifest-path", "-m"]) {
        command.arg("--manifest-path").arg(path);
    } else if let Some(path) = option(arguments, &["--path"]) {
        command
            .arg("--manifest-path")
            .arg(Path::new(path).join("Cargo.toml"));
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr)
            .trim()
            .to_owned()
            .into());
    }
    let path = String::from_utf8(output.stdout)?;
    Ok(Some(PathBuf::from(path.trim())))
}

fn metadata_options(arguments: &[OsString]) -> Vec<OsString> {
    let mut options = Vec::new();
    let mut index = 0;
    while index < arguments.len() && arguments[index] != "--" {
        let Some(argument) = arguments[index].to_str() else {
            index += 1;
            continue;
        };
        if matches!(argument, "--features" | "-F" | "--target") {
            if let Some(value) = arguments.get(index + 1) {
                options.push(OsString::from(if argument == "--target" {
                    "--filter-platform"
                } else {
                    argument
                }));
                options.push(value.clone());
            }
            index += 2;
            continue;
        }
        if argument.starts_with("--features=")
            || (argument.starts_with("-F") && argument.len() > "-F".len())
            || matches!(argument, "--all-features" | "--no-default-features")
        {
            options.push(arguments[index].clone());
        } else if let Some(target) = argument.strip_prefix("--target=") {
            options.push(OsString::from(format!("--filter-platform={target}")));
        }
        index += 1;
    }
    options
}

fn global_options(arguments: &[OsString]) -> Vec<&OsStr> {
    let mut options = Vec::new();
    let mut index = 0;
    while index < arguments.len() && arguments[index] != "--" {
        let Some(argument) = arguments[index].to_str() else {
            index += 1;
            continue;
        };
        if argument.starts_with('+') && index == 0 {
            options.push(arguments[index].as_os_str());
            index += 1;
            continue;
        }
        if matches!(argument, "--config" | "-C" | "-Z") {
            if let Some(value) = arguments.get(index + 1) {
                options.extend([arguments[index].as_os_str(), value.as_os_str()]);
            }
            index += 2;
            continue;
        }
        if argument.starts_with("--config=")
            || (argument.starts_with("-C") || argument.starts_with("-Z")) && argument.len() > 2
        {
            options.push(arguments[index].as_os_str());
            index += 1;
            continue;
        }
        index += 1;
    }
    options
}

pub(super) fn operation(arguments: &[OsString]) -> Option<&str> {
    arguments.get(command_index(arguments)?)?.to_str()
}

fn command_index(arguments: &[OsString]) -> Option<usize> {
    let mut arguments = arguments.iter().enumerate();
    while let Some((index, argument)) = arguments.next() {
        let argument = argument.to_str()?;
        if argument == "--" {
            break;
        }
        if matches!(argument, "--config" | "--color" | "--explain" | "-C" | "-Z") {
            arguments.next();
        } else if !argument.starts_with('-') && !(index == 0 && argument.starts_with('+')) {
            return Some(index);
        }
    }
    None
}

fn builtin(operation: &str) -> bool {
    kind(operation).is_some() && !matches!(operation, "clippy" | "fmt")
}

pub(super) fn kind(operation: &str) -> Option<Kind> {
    Some(match operation {
        "bench" | "build" | "check" | "clippy" | "doc" | "fix" | "package" | "publish" | "test"
        | "tree" => Kind::Packages,
        "fetch" | "generate-lockfile" | "update" => Kind::Workspace,
        "add" | "install" | "remove" | "run" | "rustc" | "rustdoc" => Kind::Single,
        "metadata" | "vendor" => Kind::Output,
        "clean" | "config" | "fmt" | "help" | "info" | "init" | "locate-project" | "login"
        | "logout" | "new" | "owner" | "report" | "search" | "uninstall" | "yank" => Kind::Plain,
        _ => return None,
    })
}

pub(super) fn selected_packages(
    cargo: &OsStr,
    arguments: &[OsString],
    manifest: &Path,
) -> Result<Vec<String>, Box<dyn Error>> {
    let mut command = Command::new(cargo);
    command
        .args(global_options(arguments))
        .args([
            "tree", "--depth", "0", "--prefix", "none", "--color", "never",
        ])
        .arg("--manifest-path")
        .arg(manifest);
    if has_option(arguments, "--workspace") || has_option(arguments, "--all") {
        command.arg("--workspace");
    }
    for package in option_values(arguments, &["--package", "-p"]) {
        command.arg("--package").arg(package);
    }
    for package in option_values(arguments, &["--exclude"]) {
        command.arg("--exclude").arg(package);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr)
            .trim()
            .to_owned()
            .into());
    }
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .filter_map(|line| line.split_whitespace().next().map(str::to_owned))
        .collect())
}

pub(super) fn package_selects_workspace(operation: &str) -> bool {
    operation != "install" && matches!(kind(operation), Some(Kind::Packages | Kind::Single))
}

pub(super) fn package_arguments(arguments: &[OsString], packages: &[String]) -> Vec<OsString> {
    let split = arguments
        .iter()
        .position(|argument| argument == "--")
        .unwrap_or(arguments.len());
    let mut selected = Vec::new();
    let mut index = 0;
    while index < split {
        let argument = arguments[index].to_string_lossy();
        if matches!(argument.as_ref(), "--workspace" | "--all") {
            index += 1;
        } else if matches!(argument.as_ref(), "-p" | "--package" | "--exclude") {
            index += 2;
        } else if argument.starts_with("--package=")
            || argument.starts_with("--exclude=")
            || (argument.starts_with("-p") && argument.len() > 2)
        {
            index += 1;
        } else {
            selected.push(arguments[index].clone());
            index += 1;
        }
    }
    for package in packages {
        selected.extend([OsString::from("--package"), OsString::from(package)]);
    }
    selected.extend_from_slice(&arguments[split..]);
    selected
}

pub(super) fn option_values<'a>(arguments: &'a [OsString], names: &[&str]) -> Vec<&'a OsStr> {
    let mut values = Vec::new();
    for (index, argument) in arguments
        .iter()
        .take_while(|argument| *argument != "--")
        .enumerate()
    {
        let Some(argument) = argument.to_str() else {
            continue;
        };
        for name in names {
            if argument == *name {
                if let Some(value) = arguments.get(index + 1) {
                    values.push(value.as_os_str());
                }
            } else if name.len() == 2 && argument.starts_with(name) && argument.len() > name.len() {
                values.push(OsStr::new(
                    argument[name.len()..]
                        .strip_prefix('=')
                        .unwrap_or(&argument[name.len()..]),
                ));
            } else if let Some(value) = argument.strip_prefix(&format!("{name}=")) {
                values.push(OsStr::new(value));
            }
        }
    }
    values
}

pub(super) fn has_option(arguments: &[OsString], name: &str) -> bool {
    arguments
        .iter()
        .take_while(|argument| *argument != "--")
        .any(|argument| {
            argument.to_str().is_some_and(|argument| {
                argument == name || argument.starts_with(&format!("{name}="))
            })
        })
}

fn option<'a>(arguments: &'a [OsString], names: &[&str]) -> Option<&'a OsStr> {
    option_values(arguments, names).into_iter().next()
}
