use std::{
    env,
    error::Error,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::Value as Json;
use toml::Value as Toml;

struct Artifact {
    rlib: PathBuf,
    rmeta: PathBuf,
    dep: PathBuf,
}

struct ProcMacro {
    library: PathBuf,
}

#[derive(Clone, Copy)]
pub(crate) struct Options<'a> {
    pub configs: &'a [OsString],
    pub resolution: &'a [OsString],
    pub rustflags: &'a OsStr,
}

pub(crate) struct Tools {
    pub rustc: PathBuf,
    pub rustdoc: PathBuf,
}

struct Sources<'a> {
    guest: &'a Path,
    macros: &'a ProcMacro,
    abi: &'a Artifact,
    check_cfg: &'a str,
    version: &'a str,
}

pub(crate) fn build(
    out: &Path,
    guest: &Path,
    macros: &Path,
    target_dir: &Path,
    target: &OsStr,
    profile: &str,
    options: Options<'_>,
) -> Result<Tools, Box<dyn Error>> {
    let abi = build_abi(out, guest, target_dir, target, profile, options)?;
    let macros = build_macros(out, macros, target_dir, profile, options)?;
    let manifest: Toml = toml::from_str(&fs::read_to_string(guest.join("Cargo.toml"))?)?;
    let version = manifest
        .get("package")
        .and_then(Toml::as_table)
        .and_then(|package| package.get("version"))
        .and_then(Toml::as_str)
        .ok_or("generated Tokio has no version")?;
    let features = manifest
        .get("features")
        .and_then(Toml::as_table)
        .ok_or("generated Tokio has no features")?
        .keys()
        .map(|feature| format!("{feature:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let check_cfg = format!("cfg(feature, values({features}))");
    let rustc = env::var_os("RUSTC").ok_or("RUSTC is unavailable")?;
    let rustdoc = env::var_os("RUSTDOC").ok_or("RUSTDOC is unavailable")?;
    let sources = Sources {
        guest: &guest.join("src/lib.rs"),
        macros: &macros,
        abi: &abi,
        check_cfg: &check_cfg,
        version,
    };
    let rustc_proxy = helper(out, "rustc", &rustc, &sources, false)?;
    let rustdoc_proxy = helper(out, "rustdoc", &rustdoc, &sources, true)?;
    Ok(Tools {
        rustc: rustc_proxy,
        rustdoc: rustdoc_proxy,
    })
}

fn build_macros(
    out: &Path,
    macros: &Path,
    target_dir: &Path,
    profile: &str,
    options: Options<'_>,
) -> Result<ProcMacro, Box<dyn Error>> {
    let root = out.join("compiler-macros");
    fs::create_dir_all(root.join("src"))?;
    let manifest = format!(
        "[package]\nname = \"telekio-compiler-macros\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies]\ntokio-macros = {{ path = {:?} }}\n",
        macros
    );
    fs::write(root.join("Cargo.toml"), manifest)?;
    fs::write(root.join("src/lib.rs"), "")?;
    ensure_lock(&root, options)?;

    let cargo = env::var_os("CARGO").ok_or("CARGO is unavailable")?;
    let mut command = Command::new(cargo);
    command
        .current_dir(&root)
        .arg("build")
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target_dir)
        .arg("--profile")
        .arg(if profile == "debug" { "dev" } else { "release" })
        .arg("--message-format=json-render-diagnostics")
        .args(options.configs)
        .args(options.resolution)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS");
    clear_clippy(&mut command);
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!(
            "Tokio macros compilation failed with {}:\n{}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    proc_macro(&output.stdout, "tokio_macros")
}

fn helper(
    out: &Path,
    name: &str,
    tool: &OsStr,
    sources: &Sources<'_>,
    documentation: bool,
) -> Result<PathBuf, Box<dyn Error>> {
    let source_path = out.join(format!("{name}.rs"));
    let executable = out.join(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    });
    let source_text = source(tool, sources, &executable, documentation);
    let changed = fs::read_to_string(&source_path).map_or(true, |current| current != source_text);
    if changed {
        fs::write(&source_path, &source_text)?;
    }
    if changed || !executable.is_file() {
        let rustc = env::var_os("RUSTC").ok_or("RUSTC is unavailable")?;
        let status = Command::new(rustc)
            .args(["--edition", "2024"])
            .arg(&source_path)
            .arg("-o")
            .arg(&executable)
            .status()?;
        if !status.success() {
            return Err(format!("{name} helper compilation failed with {status}").into());
        }
    }
    Ok(executable)
}

fn build_abi(
    out: &Path,
    guest: &Path,
    target_dir: &Path,
    target: &OsStr,
    profile: &str,
    options: Options<'_>,
) -> Result<Artifact, Box<dyn Error>> {
    let root = out.join("compiler-abi");
    fs::create_dir_all(root.join("src"))?;
    let guest_manifest: Toml = toml::from_str(&fs::read_to_string(guest.join("Cargo.toml"))?)?;
    let dependency = guest_manifest
        .get("dependencies")
        .and_then(Toml::as_table)
        .and_then(|dependencies| dependencies.get("telekio"))
        .cloned()
        .ok_or("generated Tokio has no Telekio dependency")?;
    let manifest = Toml::Table(toml::Table::from_iter([
        (
            "package".to_owned(),
            Toml::Table(toml::Table::from_iter([
                (
                    "name".to_owned(),
                    Toml::String("telekio-compiler-abi".to_owned()),
                ),
                ("version".to_owned(), Toml::String("0.0.0".to_owned())),
                ("edition".to_owned(), Toml::String("2024".to_owned())),
            ])),
        ),
        ("workspace".to_owned(), Toml::Table(toml::Table::new())),
        (
            "dependencies".to_owned(),
            Toml::Table(toml::Table::from_iter([("telekio".to_owned(), dependency)])),
        ),
    ]));
    fs::write(root.join("Cargo.toml"), toml::to_string(&manifest)?)?;
    fs::write(root.join("src/lib.rs"), "")?;

    ensure_lock(&root, options)?;

    let cargo = env::var_os("CARGO").ok_or("CARGO is unavailable")?;
    let mut flags = options.rustflags.to_os_string();
    if !flags.is_empty() {
        flags.push("\u{1f}");
    }
    flags.push("-C\u{1f}embed-bitcode=yes");
    let mut command = Command::new(cargo);
    command
        .current_dir(&root)
        .arg("build")
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target_dir)
        .arg("--target")
        .arg(target)
        .arg("--profile")
        .arg(if profile == "debug" { "dev" } else { "release" })
        .arg("--message-format=json-render-diagnostics")
        .args(options.configs)
        .args(options.resolution)
        .env("CARGO_ENCODED_RUSTFLAGS", flags)
        .env_remove("RUSTFLAGS");
    clear_clippy(&mut command);
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!(
            "Telekio ABI compilation failed with {}:\n{}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    artifact(&output.stdout, "telekio")
}

fn ensure_lock(root: &Path, options: Options<'_>) -> Result<(), Box<dyn Error>> {
    if root.join("Cargo.lock").is_file() {
        return Ok(());
    }
    let cargo = env::var_os("CARGO").ok_or("CARGO is unavailable")?;
    let mut command = Command::new(cargo);
    command
        .current_dir(root)
        .arg("generate-lockfile")
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .args(options.configs);
    if options
        .resolution
        .iter()
        .any(|option| matches!(option.to_str(), Some("--frozen" | "--offline")))
    {
        command.arg("--offline");
    }
    let status = command.status()?;
    if !status.success() {
        return Err(format!("compiler dependency lock generation failed with {status}").into());
    }
    Ok(())
}

fn clear_clippy(command: &mut Command) {
    if env::var_os("RUSTC_WORKSPACE_WRAPPER")
        .as_deref()
        .and_then(|wrapper| Path::new(wrapper).file_stem())
        .is_some_and(|name| name.to_string_lossy().starts_with("clippy-driver"))
    {
        command
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .env_remove("CLIPPY_ARGS");
    }
}

fn artifact(output: &[u8], name: &str) -> Result<Artifact, Box<dyn Error>> {
    let mut rlib = None;
    let mut rmeta = None;
    for line in output.split(|byte| *byte == b'\n') {
        let Ok(message) = serde_json::from_slice::<Json>(line) else {
            continue;
        };
        if message["reason"] != "compiler-artifact" || message["target"]["name"] != name {
            continue;
        }
        for filename in message["filenames"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .map(PathBuf::from)
        {
            match filename.extension().and_then(OsStr::to_str) {
                Some("rlib") => rlib = Some(filename),
                Some("rmeta") => rmeta = Some(filename),
                _ => {}
            }
        }
    }
    let rlib = rlib.ok_or_else(|| format!("canonical {name} rlib was not produced"))?;
    let rmeta = rmeta.ok_or_else(|| format!("canonical {name} metadata was not produced"))?;
    let dep = rmeta
        .file_name()
        .and_then(OsStr::to_str)
        .and_then(|name| name.strip_prefix("lib"))
        .and_then(|name| name.strip_suffix(".rmeta"))
        .map(|name| rmeta.with_file_name(format!("{name}.d")))
        .ok_or_else(|| format!("canonical {name} metadata has an invalid name"))?;
    if !dep.is_file() {
        return Err(format!(
            "canonical {name} dependency file is missing: {}",
            dep.display()
        )
        .into());
    }
    Ok(Artifact { rlib, rmeta, dep })
}

fn proc_macro(output: &[u8], name: &str) -> Result<ProcMacro, Box<dyn Error>> {
    for line in output.split(|byte| *byte == b'\n') {
        let Ok(message) = serde_json::from_slice::<Json>(line) else {
            continue;
        };
        if message["reason"] != "compiler-artifact" || message["target"]["name"] != name {
            continue;
        }
        if let Some(library) = message["filenames"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .map(PathBuf::from)
            .find(|path| {
                matches!(
                    path.extension().and_then(OsStr::to_str),
                    Some("dll" | "dylib" | "so")
                )
            })
        {
            return Ok(ProcMacro { library });
        }
    }
    Err(format!("canonical {name} proc macro was not produced").into())
}

fn source(tool: &OsStr, sources: &Sources<'_>, executable: &Path, documentation: bool) -> String {
    format!(
        r###"use std::{{env, ffi::OsString, fs, path::{{Path, PathBuf}}, process::{{self, Command}}}};

const TOOL: &str = {tool:?};
const GUEST: &str = {guest:?};
const MACROS: &str = {macros:?};
const ABI_RLIB: &str = {rlib:?};
const ABI_RMETA: &str = {rmeta:?};
const ABI_DEP: &str = {dep:?};
const CHECK_CFG: &str = {check_cfg:?};
const VERSION: &str = {version:?};
const COMPILER: &str = {executable:?};
const DOCUMENTATION: bool = {documentation};

fn main() {{
    let mut arguments = env::args_os().skip(1).collect::<Vec<_>>();
    let package = env::var("CARGO_PKG_NAME").unwrap_or_default();
    let crate_name = option(&arguments, "--crate-name").unwrap_or_default();
    if arguments.iter().any(|argument| argument == "--print") {{
        passthrough(arguments);
    }}
    if !arguments.iter().any(|argument| argument == "--target") {{
        passthrough(arguments);
    }}
    arguments.extend([
        OsString::from("-L"),
        OsString::from(format!("dependency={{}}", Path::new(MACROS).parent().unwrap().display())),
    ]);
    if package == "tokio" && crate_name == "tokio" {{
        replace_source(&mut arguments, GUEST);
        replace_extern(&mut arguments, "tokio_macros", MACROS);
        if DOCUMENTATION {{
            replace_option(&mut arguments, "--crate-version", VERSION);
        }}
        arguments.extend([
            OsString::from("--extern"),
            OsString::from(format!("telekio={{ABI_RLIB}}")),
            OsString::from("-L"),
            OsString::from(format!("dependency={{}}", Path::new(ABI_RLIB).parent().unwrap().display())),
            OsString::from("--check-cfg"),
            OsString::from(CHECK_CFG),
        ]);
        passthrough(arguments);
    }}
    if DOCUMENTATION || package != "telekio" || crate_name != "telekio" {{
        passthrough(arguments);
    }}
    let Some(out_dir) = option(&arguments, "--out-dir").map(PathBuf::from) else {{
        passthrough(arguments);
    }};
    let Some(extra) = codegen(&arguments, "extra-filename") else {{
        passthrough(arguments);
    }};
    let Some(emit) = option(&arguments, "--emit") else {{
        passthrough(arguments);
    }};
    let rlib = out_dir.join(format!("libtelekio{{extra}}.rlib"));
    let rmeta = out_dir.join(format!("libtelekio{{extra}}.rmeta"));
    let dep = out_dir.join(format!("telekio{{extra}}.d"));
    fs::create_dir_all(&out_dir).unwrap();
    let mut outputs = Vec::new();
    if emit.split(',').any(|emit| emit == "link") {{
        persist(Path::new(ABI_RLIB), &rlib);
        outputs.push(rlib);
    }}
    if emit.split(',').any(|emit| emit == "metadata") {{
        persist(Path::new(ABI_RMETA), &rmeta);
        outputs.push(rmeta);
    }}
    if emit.split(',').any(|emit| emit == "dep-info") {{
        let dependencies = fs::read_to_string(ABI_DEP).unwrap();
        let dependencies = dependencies.split_once(": ").map_or("", |(_, dependencies)| dependencies);
        fs::write(
            dep,
            format!("{{}}: {{}} {{}}\n", outputs.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(" "), COMPILER, dependencies.trim()),
        ).unwrap();
    }}
}}

fn replace_option(arguments: &mut [OsString], name: &str, replacement: &str) {{
    for index in 0..arguments.len() {{
        if arguments[index] == name {{
            if let Some(argument) = arguments.get_mut(index + 1) {{
                *argument = replacement.into();
                return;
            }}
        }} else if arguments[index]
            .to_string_lossy()
            .starts_with(&format!("{{name}}="))
        {{
            arguments[index] = format!("{{name}}={{replacement}}").into();
            return;
        }}
    }}
}}

fn replace_extern(arguments: &mut [OsString], name: &str, replacement: &str) {{
    for index in 0..arguments.len() {{
        if arguments[index] == "--extern" {{
            if let Some(argument) = arguments.get_mut(index + 1) {{
                if argument.to_string_lossy().starts_with(&format!("{{name}}=")) {{
                    *argument = format!("{{name}}={{replacement}}").into();
                    return;
                }}
            }}
        }} else if arguments[index]
            .to_string_lossy()
            .starts_with(&format!("--extern={{name}}="))
        {{
            arguments[index] = format!("--extern={{name}}={{replacement}}").into();
            return;
        }}
    }}
}}

fn replace_source(arguments: &mut [OsString], source: &str) {{
    let input = arguments
        .iter_mut()
        .find(|argument| Path::new(argument).extension() == Some("rs".as_ref()))
        .expect("tool invocation has no Rust source");
    *input = source.into();
}}

fn persist(source: &Path, destination: &Path) {{
    if destination.is_file() {{
        fs::remove_file(destination).unwrap();
    }}
    fs::hard_link(source, destination).unwrap();
}}

fn option(arguments: &[OsString], name: &str) -> Option<String> {{
    arguments.iter().enumerate().find_map(|(index, argument)| {{
        let argument = argument.to_string_lossy();
        if argument == name {{
            arguments.get(index + 1).map(|value| value.to_string_lossy().into_owned())
        }} else {{
            argument.strip_prefix(&format!("{{name}}=")).map(str::to_owned)
        }}
    }})
}}

fn codegen(arguments: &[OsString], name: &str) -> Option<String> {{
    arguments.iter().enumerate().find_map(|(index, argument)| {{
        let argument = argument.to_string_lossy();
        if argument == "-C" {{
            arguments.get(index + 1)?.to_string_lossy().strip_prefix(&format!("{{name}}=")).map(str::to_owned)
        }} else {{
            argument.strip_prefix(&format!("-C{{name}}=")).map(str::to_owned)
        }}
    }})
}}

fn passthrough(arguments: Vec<OsString>) -> ! {{
    let status = Command::new(TOOL).args(arguments).status().unwrap();
    process::exit(status.code().unwrap_or(1));
}}
"###,
        tool = tool.to_string_lossy(),
        guest = sources.guest.to_string_lossy(),
        macros = sources.macros.library.to_string_lossy(),
        rlib = sources.abi.rlib.to_string_lossy(),
        rmeta = sources.abi.rmeta.to_string_lossy(),
        dep = sources.abi.dep.to_string_lossy(),
        check_cfg = sources.check_cfg,
        version = sources.version,
        executable = executable.to_string_lossy(),
        documentation = documentation,
    )
}
