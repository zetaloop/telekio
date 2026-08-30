use std::{
    env,
    error::Error,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::Value as Json;
use sha2::{Digest, Sha256};
use toml::Value as Toml;

use crate::linker::Mapping;

struct Abi {
    source: PathBuf,
}

struct ProcMacro {
    library: PathBuf,
}

#[derive(Clone, Copy)]
pub(crate) struct Options<'a> {
    pub configs: &'a [OsString],
    pub resolution: &'a [OsString],
    pub mappings: &'a [Mapping],
    pub records: &'a Path,
}

pub(crate) struct Tools {
    pub rustc: PathBuf,
    pub rustdoc: PathBuf,
    pub fingerprint: String,
}

struct Sources<'a> {
    compiler: &'a OsStr,
    guest: &'a Path,
    macros: &'a ProcMacro,
    abi: &'a Abi,
    check_cfg: &'a str,
    version: &'a str,
    mappings: &'a [Mapping],
    records: &'a Path,
}

pub(crate) fn build(
    out: &Path,
    guest: &Path,
    macros: &Path,
    target_dir: &Path,
    profile: &str,
    options: Options<'_>,
) -> Result<Tools, Box<dyn Error>> {
    let abi = prepare_abi(out, guest, options)?;
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
        compiler: &rustc,
        guest: &guest.join("src/lib.rs"),
        macros: &macros,
        abi: &abi,
        check_cfg: &check_cfg,
        version,
        mappings: options.mappings,
        records: options.records,
    };
    let rustc_proxy = helper(out, "rustc", &rustc, &sources, false)?;
    let rustdoc_proxy = helper(out, "rustdoc", &rustdoc, &sources, true)?;
    let fingerprint = Sha256::digest(fs::read(out.join("rustc.rs"))?)[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(Tools {
        rustc: rustc_proxy,
        rustdoc: rustdoc_proxy,
        fingerprint,
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
    let host = env::var("HOST").map_err(|_| "HOST is unavailable")?;
    let host_rustflags = format!(
        "CARGO_TARGET_{}_RUSTFLAGS",
        host.replace('-', "_").to_ascii_uppercase()
    );
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
        .arg("--config")
        .arg("build.rustflags=[]")
        .arg("--config")
        .arg(format!("target.{host}.rustflags=[]"))
        .args(options.resolution)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .env_remove(host_rustflags);
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

fn prepare_abi(out: &Path, guest: &Path, options: Options<'_>) -> Result<Abi, Box<dyn Error>> {
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
    let output = Command::new(cargo)
        .current_dir(&root)
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .args(options.configs)
        .args(options.resolution)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "Telekio ABI metadata failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let metadata: Json = serde_json::from_slice(&output.stdout)?;
    let manifest = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|package| package["name"] == "telekio")
        .and_then(|package| package["manifest_path"].as_str())
        .map(PathBuf::from)
        .ok_or("Telekio ABI package is unavailable")?;
    let source = manifest
        .parent()
        .ok_or("Telekio ABI manifest has no parent")?
        .join("src/lib.rs");
    source
        .is_file()
        .then_some(Abi { source })
        .ok_or_else(|| "Telekio ABI source is unavailable".into())
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
    let mappings = sources
        .mappings
        .iter()
        .map(|mapping| format!("({:?}, {:?})", mapping.proxy, mapping.entry))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r###"use std::{{env, ffi::OsString, fs::{{self, OpenOptions}}, io::{{BufRead, BufReader, Read, Write}}, path::{{Path, PathBuf}}, process::{{self, Command, Stdio}}}};

const TOOL: &str = {tool:?};
const RUSTC: &str = {rustc:?};
const GUEST: &str = {guest:?};
const MACROS: &str = {macros:?};
const ABI_SOURCE: &str = {abi_source:?};
const CHECK_CFG: &str = {check_cfg:?};
const VERSION: &str = {version:?};
const COMPILER: &str = {executable:?};
const RECORDS: &str = {records:?};
const MAPPINGS: &[(&str, &str)] = &[{mappings}];
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
        let abi = abi(&arguments);
        if DOCUMENTATION {{
            replace_option(&mut arguments, "--crate-version", VERSION);
        }}
        arguments.extend([
            OsString::from("--extern"),
            OsString::from(format!("telekio={{}}", abi.rmeta.display())),
            OsString::from("-L"),
            OsString::from(format!("dependency={{}}", abi.rmeta.parent().unwrap().display())),
            OsString::from("--check-cfg"),
            OsString::from(CHECK_CFG),
        ]);
        passthrough(arguments);
    }}
    if !DOCUMENTATION
        && !arguments.iter().any(|argument| argument == "--test")
        && let Some((proxy, entry)) = artifact(&arguments)
    {{
        arguments.extend([
            OsString::from("--check-cfg"),
            OsString::from("cfg(telekio_static)"),
        ]);
        link(&arguments, proxy);
        staticlib(arguments, entry);
    }}
    if DOCUMENTATION || package != "telekio" || crate_name != "telekio" {{
        passthrough(arguments);
    }}
    let abi = abi(&arguments);
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
        persist(&abi.rlib, &rlib);
        outputs.push(rlib);
    }}
    if emit.split(',').any(|emit| emit == "metadata") {{
        persist(&abi.rmeta, &rmeta);
        outputs.push(rmeta);
    }}
    if emit.split(',').any(|emit| emit == "dep-info") {{
        let dependencies = fs::read_to_string(abi.dep).unwrap();
        let dependencies = dependencies.split_once(": ").map_or("", |(_, dependencies)| dependencies);
        fs::write(
            dep,
            format!("{{}}: {{}} {{}}\n", outputs.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(" "), COMPILER, dependencies.trim()),
        ).unwrap();
    }}
}}

struct Abi {{
    rlib: PathBuf,
    rmeta: PathBuf,
    dep: PathBuf,
}}

fn abi(arguments: &[OsString]) -> Abi {{
    let directory = Path::new(RECORDS).join("abi");
    fs::create_dir_all(&directory).unwrap();
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(directory.join("lock"))
        .unwrap();
    lock.lock().unwrap();
    let abi = Abi {{
        rlib: directory.join("libtelekio.rlib"),
        rmeta: directory.join("libtelekio.rmeta"),
        dep: directory.join("libtelekio.d"),
    }};
    if !abi.rlib.is_file() || !abi.rmeta.is_file() || !abi.dep.is_file() {{
        let mut arguments = arguments.to_vec();
        replace_source(&mut arguments, ABI_SOURCE);
        replace_option(&mut arguments, "--crate-name", "telekio");
        replace_option(&mut arguments, "--crate-type", "rlib");
        replace_option(&mut arguments, "--edition", "2024");
        replace_option(
            &mut arguments,
            "--emit",
            &format!(
                "dep-info={{}},metadata={{}},link={{}}",
                abi.dep.display(),
                abi.rmeta.display(),
                abi.rlib.display()
            ),
        );
        remove_options(
            &mut arguments,
            &[
                "-o",
                "--out-dir",
                "-l",
                "--cfg",
                "--check-cfg",
                "--crate-version",
            ],
        );
        arguments.retain(|argument| argument != "--test");
        for name in ["metadata", "extra-filename", "incremental", "linker", "link-arg", "link-args", "embed-bitcode"] {{
            remove_codegen(&mut arguments, name);
        }}
        arguments.extend([
            OsString::from("--cfg"),
            OsString::from("feature=\"guest\""),
            OsString::from("--check-cfg"),
            OsString::from("cfg(feature, values(\"guest\"))"),
            OsString::from("-C"),
            OsString::from("metadata=telekio"),
            OsString::from("-C"),
            OsString::from("embed-bitcode=yes"),
        ]);
        let status = compiler().args(arguments).status().unwrap();
        if !status.success() {{
            process::exit(status.code().unwrap_or(1));
        }}
        assert!(abi.rlib.is_file() && abi.rmeta.is_file() && abi.dep.is_file(), "rustc did not produce the Telekio ABI");
    }}
    abi
}}

fn remove_options(arguments: &mut Vec<OsString>, names: &[&str]) {{
    let mut index = 0;
    while index < arguments.len() {{
        let argument = arguments[index].to_string_lossy();
        let paired = names.iter().any(|name| argument == *name);
        let inline = names
            .iter()
            .any(|name| argument.starts_with(&format!("{{name}}=")));
        if paired || inline {{
            arguments.drain(index..index + usize::from(paired) + 1);
        }} else {{
            index += 1;
        }}
    }}
}}

fn artifact(arguments: &[OsString]) -> Option<&'static (&'static str, &'static str)> {{
    let source = arguments
        .iter()
        .find(|argument| Path::new(argument).extension() == Some("rs".as_ref()))?;
    let name = Path::new(source).file_name()?.to_str()?;
    MAPPINGS.iter().find(|(proxy, _)| *proxy == name)
}}

fn link(arguments: &[OsString], proxy: &str) {{
    let mut command = arguments.to_vec();
    command.extend([OsString::from("--print"), OsString::from("link-args")]);
    let mut child = Command::new(TOOL)
        .args(command)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = Vec::new();
    stdout.read_until(b'\n', &mut line).unwrap();
    if line.is_empty() {{
        let status = child.wait().unwrap();
        process::exit(status.code().unwrap_or(1));
    }}
    let mut command = parse_command(&line);
    persist_raw_dylibs(&mut command);
    write_record(&Path::new(RECORDS).join(format!("{{proxy}}.record")), &command);
    let mut remaining = Vec::new();
    stdout.read_to_end(&mut remaining).unwrap();
    std::io::stdout().write_all(&remaining).unwrap();
    let status = child.wait().unwrap();
    if !status.success() {{
        process::exit(status.code().unwrap_or(1));
    }}
}}

fn parse_command(line: &[u8]) -> Vec<OsString> {{
    let mut arguments = Vec::new();
    let mut index = line
        .iter()
        .enumerate()
        .find(|(index, byte)| {{
            **byte == b'"'
                && (*index == 0 || line[*index - 1].is_ascii_whitespace())
        }})
        .map(|(index, _)| index)
        .expect("rustc linker command has no quoted executable");
    while index < line.len() {{
        while line.get(index).is_some_and(u8::is_ascii_whitespace) {{
            index += 1;
        }}
        if index == line.len() {{
            break;
        }}
        arguments.push(os_string(parse_quoted(line, &mut index)));
    }}
    arguments
}}

fn parse_quoted(line: &[u8], index: &mut usize) -> Vec<u8> {{
    assert_eq!(line[*index], b'"', "linker argument is not quoted");
    *index += 1;
    let mut argument = Vec::new();
    while line[*index] != b'"' {{
        if line[*index] != b'\\' {{
            argument.push(line[*index]);
            *index += 1;
            continue;
        }}
        *index += 1;
        match line[*index] {{
            b'\\' | b'"' | b'\'' => argument.push(line[*index]),
            b'n' => argument.push(b'\n'),
            b'r' => argument.push(b'\r'),
            b't' => argument.push(b'\t'),
            b'0' => argument.push(0),
            b'x' => {{
                argument.push(hex(line[*index + 1]) * 16 + hex(line[*index + 2]));
                *index += 2;
            }}
            b'u' => {{
                assert_eq!(line[*index + 1], b'{{', "Unicode escape has no opening brace");
                let start = *index + 2;
                let end = line[start..]
                    .iter()
                    .position(|byte| *byte == b'}}')
                    .map(|offset| start + offset)
                    .expect("Unicode escape has no closing brace");
                let value = std::str::from_utf8(&line[start..end]).unwrap();
                encode(u32::from_str_radix(value, 16).unwrap(), &mut argument);
                *index = end;
            }}
            escape => panic!("unsupported linker argument escape: {{escape:#x}}"),
        }}
        *index += 1;
    }}
    *index += 1;
    argument
}}

fn hex(byte: u8) -> u8 {{
    match byte {{
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => panic!("invalid hexadecimal escape"),
    }}
}}

fn encode(value: u32, output: &mut Vec<u8>) {{
    match value {{
        0..=0x7f => output.push(value as u8),
        0x80..=0x7ff => {{
            output.push(0xc0 | (value >> 6) as u8);
            output.push(0x80 | (value & 0x3f) as u8);
        }}
        0x800..=0xffff => {{
            output.push(0xe0 | (value >> 12) as u8);
            output.push(0x80 | ((value >> 6) & 0x3f) as u8);
            output.push(0x80 | (value & 0x3f) as u8);
        }}
        0x10000..=0x10ffff => {{
            output.push(0xf0 | (value >> 18) as u8);
            output.push(0x80 | ((value >> 12) & 0x3f) as u8);
            output.push(0x80 | ((value >> 6) & 0x3f) as u8);
            output.push(0x80 | (value & 0x3f) as u8);
        }}
        _ => panic!("invalid Unicode escape"),
    }}
}}

fn os_string(bytes: Vec<u8>) -> OsString {{
    // Bytes are decoded from the standard library's lossless OsStr Debug representation.
    unsafe {{ OsString::from_encoded_bytes_unchecked(bytes) }}
}}

fn persist_raw_dylibs(arguments: &mut [OsString]) {{
    let destination = Path::new(RECORDS).join("raw-dylibs");
    let mut index = 0;
    while index < arguments.len() {{
        if arguments[index] == "-L"
            && let Some(source) = arguments.get(index + 1).map(PathBuf::from)
            && raw_dylibs(&source)
        {{
            persist_directory(&source, &destination);
            arguments[index + 1] = destination.clone().into_os_string();
            index += 2;
            continue;
        }}
        let argument = arguments[index].to_string_lossy();
        if argument.to_ascii_uppercase().starts_with("/LIBPATH:") {{
            let source = PathBuf::from(&argument[9..]);
            if raw_dylibs(&source) {{
                persist_directory(&source, &destination);
                arguments[index] = format!("/LIBPATH:{{}}", destination.display()).into();
            }}
        }}
        index += 1;
    }}
}}

fn raw_dylibs(path: &Path) -> bool {{
    path.file_name() == Some("raw-dylibs".as_ref())
        && path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name.to_string_lossy().starts_with("rustc"))
}}

fn persist_directory(source: &Path, destination: &Path) {{
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {{
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if let Err(error) = fs::hard_link(entry.path(), &target) {{
            if error.kind() != std::io::ErrorKind::AlreadyExists {{
                panic!("failed to preserve raw dylib: {{error}}");
            }}
            assert_eq!(
                fs::read(entry.path()).unwrap(),
                fs::read(&target).unwrap(),
                "raw dylib name collision"
            );
        }}
    }}
}}

fn write_record(path: &Path, arguments: &[OsString]) {{
    let mut record = Vec::new();
    for argument in arguments {{
        let bytes = argument.as_encoded_bytes();
        record.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        record.extend_from_slice(bytes);
    }}
    fs::write(path, record).unwrap();
}}

fn staticlib(mut arguments: Vec<OsString>, entry: &str) -> ! {{
    let archive = Path::new(RECORDS).join(format!("{{entry}}.telekio"));
    replace_option(&mut arguments, "--crate-type", "staticlib");
    replace_option(&mut arguments, "--emit", &format!("link={{}}", archive.display()));
    if codegen(&arguments, "incremental").is_some() {{
        remove_codegen(&mut arguments, "incremental");
        arguments.extend([
            OsString::from("-C"),
            OsString::from(format!("incremental={{}}", Path::new(RECORDS).join(format!("{{entry}}.incremental")).display())),
        ]);
    }}
    arguments.extend([
        OsString::from("--cfg"),
        OsString::from("telekio_static"),
        OsString::from("--check-cfg"),
        OsString::from("cfg(telekio_static)"),
    ]);
    let status = compiler().args(arguments).status().unwrap();
    if status.success() {{
        assert!(archive.is_file(), "rustc did not produce the Telekio artifact archive");
    }}
    process::exit(status.code().unwrap_or(1));
}}

fn compiler() -> Command {{
    if let Some(wrapper) = env::var_os("RUSTC_WRAPPER") {{
        let mut command = Command::new(wrapper);
        if let Some(workspace) = env::var_os("RUSTC_WORKSPACE_WRAPPER") {{
            command.arg(workspace);
        }}
        command.arg(RUSTC);
        command
    }} else if let Some(wrapper) = env::var_os("RUSTC_WORKSPACE_WRAPPER") {{
        let mut command = Command::new(wrapper);
        command.arg(RUSTC);
        command
    }} else {{
        Command::new(RUSTC)
    }}
}}

fn remove_codegen(arguments: &mut Vec<OsString>, name: &str) {{
    let mut index = 0;
    while index < arguments.len() {{
        let paired = arguments[index] == "-C"
            && arguments
                .get(index + 1)
                .is_some_and(|argument| argument.to_string_lossy().starts_with(&format!("{{name}}=")));
        let inline = arguments[index]
            .to_string_lossy()
            .starts_with(&format!("-C{{name}}="));
        if paired || inline {{
            arguments.drain(index..index + usize::from(paired) + 1);
        }} else {{
            index += 1;
        }}
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
        rustc = sources.compiler.to_string_lossy(),
        guest = sources.guest.to_string_lossy(),
        macros = sources.macros.library.to_string_lossy(),
        abi_source = sources.abi.source.to_string_lossy(),
        check_cfg = sources.check_cfg,
        version = sources.version,
        executable = executable.to_string_lossy(),
        records = sources.records.to_string_lossy(),
        documentation = documentation,
    )
}
