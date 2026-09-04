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

pub(super) fn offline(arguments: &[OsString]) -> bool {
    arguments
        .iter()
        .any(|argument| matches!(argument.to_str(), Some("--offline" | "--frozen")))
}

pub(super) fn expand_alias(
    cargo: &OsStr,
    arguments: &[OsString],
) -> Result<Vec<OsString>, Box<dyn Error>> {
    let mut expanded = arguments.to_vec();
    for _ in 0..16 {
        if operation(&expanded).is_some() {
            return Ok(expanded);
        }
        let output = Command::new(cargo)
            .args(global_options(&expanded))
            .args(["--color", "never", "--list"])
            .output()?;
        if !output.status.success() {
            return Ok(expanded);
        }
        let aliases = String::from_utf8(output.stdout)?;
        let Some((index, target)) = expanded.iter().enumerate().find_map(|(index, argument)| {
            let name = argument.to_str()?;
            aliases.lines().find_map(|line| {
                let (alias, target) = line.split_once(" alias: ")?;
                (alias.trim() == name).then_some((index, target))
            })
        }) else {
            return Ok(expanded);
        };
        if target.starts_with('!') {
            return Err("Cargo shell aliases cannot determine a Telekio artifact role; use the direct Cargo command".into());
        }
        expanded.splice(index..=index, target.split_whitespace().map(OsString::from));
    }
    Err("Cargo alias expansion is recursive".into())
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
    if let Some(path) = option(arguments, "--manifest-path") {
        command.arg("--manifest-path").arg(path);
    } else if let Some(path) = option(arguments, "--path") {
        command
            .arg("--manifest-path")
            .arg(Path::new(path).join("Cargo.toml"));
    }
    let output = command.output()?;
    if !output.status.success() {
        return Ok(None);
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
            || matches!(
                argument,
                "--all-features" | "--no-default-features" | "--locked" | "--frozen" | "--offline"
            )
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
        if argument == "--config" || argument == "-C" {
            if let Some(value) = arguments.get(index + 1) {
                options.extend([arguments[index].as_os_str(), value.as_os_str()]);
            }
            index += 2;
            continue;
        }
        if argument.starts_with("--config=") || argument.starts_with("-C=") {
            options.push(arguments[index].as_os_str());
            index += 1;
            continue;
        }
        index += 1;
    }
    options
}

pub(super) fn operation(arguments: &[OsString]) -> Option<&str> {
    const COMMANDS: &[&str] = &[
        "add",
        "bench",
        "build",
        "check",
        "clean",
        "clippy",
        "doc",
        "fetch",
        "fix",
        "fmt",
        "generate-lockfile",
        "help",
        "info",
        "init",
        "install",
        "locate-project",
        "login",
        "logout",
        "metadata",
        "new",
        "owner",
        "package",
        "publish",
        "remove",
        "report",
        "run",
        "rustc",
        "rustdoc",
        "search",
        "test",
        "tree",
        "uninstall",
        "update",
        "vendor",
        "yank",
    ];
    arguments
        .iter()
        .take_while(|argument| *argument != "--")
        .filter_map(|argument| argument.to_str())
        .find(|argument| COMMANDS.contains(argument))
}

pub(super) fn kind(operation: &str) -> Kind {
    match operation {
        "bench" | "build" | "check" | "clippy" | "doc" | "fix" | "package" | "publish" | "test"
        | "tree" => Kind::Packages,
        "fetch" | "generate-lockfile" | "update" => Kind::Workspace,
        "add" | "install" | "remove" | "run" | "rustc" | "rustdoc" => Kind::Single,
        "metadata" | "vendor" => Kind::Output,
        "clean" | "fmt" | "help" | "info" | "init" | "locate-project" | "login" | "logout"
        | "new" | "owner" | "report" | "search" | "uninstall" | "yank" => Kind::Plain,
        _ => Kind::Output,
    }
}

pub(super) fn package_selects_workspace(operation: &str) -> bool {
    operation != "install" && matches!(kind(operation), Kind::Packages | Kind::Single)
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
            } else if *name == "-p" && argument.starts_with("-p") && argument.len() > "-p".len() {
                values.push(OsStr::new(&argument["-p".len()..]));
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

fn option<'a>(arguments: &'a [OsString], name: &str) -> Option<&'a OsStr> {
    option_values(arguments, &[name]).into_iter().next()
}
